use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::mem::size_of;
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::atomic::{ Ordering, compiler_fence};

use crate::netlink::ProcessExitEvent;

const MAX_STREAM_BYTES: usize = 64 * 1024; // 64 KiB per stream per process

// x86_64 syscall number for bpf()
const SYS_BPF: i64 = 321;

const BPF_MAP_CREATE: i32 = 0;
const BPF_PROG_LOAD: i32 = 5;
const BPF_RAW_TRACEPOINT_OPEN: i32 = 17;

const BPF_MAP_TYPE_RINGBUF: u32 = 27;
const BPF_PROG_TYPE_RAW_TRACEPOINT: u32 = 17;

const BPF_RINGBUF_BUSY_BIT: u32 = 1 << 31;
const BPF_RINGBUF_DISCARD_BIT: u32 = 1 << 30;
const BPF_RINGBUF_HDR_SZ: usize = 8;

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

unsafe extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn mmap(
        addr: *mut u8,
        length: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut u8;
    fn munmap(addr: *mut u8, length: usize) -> i32;
    fn close(fd: i32) -> i32;
}

#[repr(C)]
struct BpfAttrMapCreate {
    map_type: u32,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    map_flags: u32,
    inner_map_fd: u32,
    numa_node: u32,
    map_name: [u8; 16],
}

#[repr(C)]
struct BpfAttrProgLoad {
    prog_type: u32,
    insn_cnt: u32,
    insns: u64,
    license: u64,
    log_level: u32,
    log_size: u32,
    log_buf: u64,
    kern_version: u32,
    prog_flags: u32,
    prog_name: [u8; 16],
}

#[repr(C)]
struct BpfAttrRawTracepointOpen {
    name: u64,
    prog_fd: u32,
    _pad: u32,
}

#[derive(Default, Clone, Debug)]
struct ProcessStreams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessStreams {
    fn append(&mut self, fd: u32, data: &[u8]) {
        let target = if fd == 1 {
            &mut self.stdout
        } else {
            &mut self.stderr
        };
        if target.len() + data.len() > MAX_STREAM_BYTES {
            let overflow = (target.len() + data.len()) - MAX_STREAM_BYTES;
            if overflow >= target.len() {
                target.clear();
            } else {
                target.drain(..overflow);
            }
        }
        target.extend_from_slice(data);
    }
}

pub struct OutputCollector {
    ebpf: Option<EbpfWriteTracer>,
    streams: HashMap<i32, ProcessStreams>,
}

impl OutputCollector {
    pub fn new(daemon_pid: u32) -> Self {
        let ebpf = EbpfWriteTracer::new(daemon_pid).ok();
        Self {
            ebpf,
            streams: HashMap::new(),
        }
    }

    pub fn has_ebpf(&self) -> bool {
        self.ebpf.is_some()
    }

    /// Drains any pending write(1|2) events from the kernel eBPF ringbuffer.
    pub fn poll_ebpf(&mut self) {
        let Some(ref mut tracer) = self.ebpf else {
            return;
        };
        let streams = &mut self.streams;
        tracer.consume(|pid, fd, bytes| {
            streams.entry(pid).or_default().append(fd, bytes);
        });
    }

    /// Discards buffered output for a process that exited cleanly (exit code 0).
    pub fn discard_process(&mut self, tgid: i32) {
        self.streams.remove(&tgid);
    }

    /// Collects all available `stdout` and `stderr` for a failed process since the last power on:
    /// 1. Real-time eBPF `sys_enter_write` capture (for terminal/pipe/background processes)
    /// 2. Regular files linked by `/proc/<pid>/fd/1` and `/proc/<pid>/fd/2`
    /// 3. `systemd-journald` (`journalctl --boot=0`) entries for the PID and/or systemd service unit
    pub fn collect_for_exit(&mut self, ev: &ProcessExitEvent) -> (String, String) {
        self.poll_ebpf();

        let mut stdout_parts = Vec::new();
        let mut stderr_parts = Vec::new();

        // 1. Check journalctl since current boot (--boot=0)
        let (j_out, j_err) = query_journalctl_since_boot(ev.tgid, ev.systemd_unit.as_deref());
        if !j_out.is_empty() {
            stdout_parts.push(j_out);
        }
        if !j_err.is_empty() {
            stderr_parts.push(j_err);
        }

        // 2. Check file-backed stdout / stderr paths discovered in /proc/<pid>/fd/{1,2}
        if let Some(ref out_path) = ev.stdout_path {
            if let Some(content) = read_regular_file_tail(out_path) {
                if !content.is_empty() && !stdout_parts.iter().any(|p| p == &content) {
                    stdout_parts.push(content);
                }
            }
        }
        if let Some(ref err_path) = ev.stderr_path {
            if let Some(content) = read_regular_file_tail(err_path) {
                if !content.is_empty() && !stderr_parts.iter().any(|p| p == &content) {
                    stderr_parts.push(content);
                }
            }
        }

        // 3. Check real-time eBPF write(1) / write(2) buffers
        if let Some(captured) = self.streams.remove(&ev.tgid) {
            let ebpf_out = String::from_utf8_lossy(&captured.stdout).trim().to_string();
            let ebpf_err = String::from_utf8_lossy(&captured.stderr).trim().to_string();

            if !ebpf_out.is_empty() && !stdout_parts.iter().any(|p| p.contains(&ebpf_out)) {
                stdout_parts.push(ebpf_out);
            }
            if !ebpf_err.is_empty() && !stderr_parts.iter().any(|p| p.contains(&ebpf_err)) {
                stderr_parts.push(ebpf_err);
            }
        }

        (stdout_parts.join("\n"), stderr_parts.join("\n"))
    }
}

struct EbpfWriteTracer {
    map_fd: i32,
    prog_fd: i32,
    tp_fd: i32,
    consumer_ptr: *mut u8,
    producer_ptr: *mut u8,
    ring_size: usize,
    page_size: usize,
}

impl EbpfWriteTracer {
    fn new(self_pid: u32) -> io::Result<Self> {
        let page_size = 4096usize;
        let ring_size = 256 * 1024usize; // 256 KiB ringbuf

        let map_attr = BpfAttrMapCreate {
            map_type: BPF_MAP_TYPE_RINGBUF,
            key_size: 0,
            value_size: 0,
            max_entries: ring_size as u32,
            map_flags: 0,
            inner_map_fd: 0,
            numa_node: 0,
            map_name: *b"err_det_rb\0\0\0\0\0\0",
        };

        let map_fd = unsafe {
            syscall(
                SYS_BPF,
                BPF_MAP_CREATE,
                &map_attr as *const BpfAttrMapCreate,
                size_of::<BpfAttrMapCreate>(),
            )
        } as i32;
        if map_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // eBPF bytecode for raw_tracepoint/sys_enter capturing write(1|2, buf, count)
        let mut insns: [u64; 62] = [
            0x0000000000081279, // 0: r2 = *(u64 *)(r1 + 8) [syscall_id]
            0x00000001003a0255, // 1: if r2 != 1 (__NR_write) goto exit
            0x0000000000001679, // 2: r6 = *(u64 *)(r1 + 0) [pt_regs]
            0x00000000000001b7, // 3: r1 = 0
            0x00000000fff81a7b, // 4: *(u64 *)(r10 - 8) = r1
            0x00000000000063bf, // 5: r3 = r6
            0x0000007000000307, // 6: r3 += 0x70 (&regs->rdi)
            0x000000000000a1bf, // 7: r1 = r10
            0xfffffff800000107, // 8: r1 += -8
            0x00000008000002b4, // 9: w2 = 8
            0x0000007100000085, // 10: call bpf_probe_read_kernel
            0x00000000fff8a179, // 11: r1 = *(u64 *)(r10 - 8) [fd]
            0xfffffffd00000107, // 12: r1 += -3
            0xfffffffe002e01a5, // 13: if r1 < -2 goto exit (only fd 1 or 2)
            0x0000000e00000085, // 14: call bpf_get_current_pid_tgid
            0x0000002000000077, // 15: r0 >>= 32
            0x00000000002b0015 | ((self_pid as u64) << 32), // 16: if r0 == self_pid goto exit
            0x00000000000007bf, // 17: r7 = r0
            0x00000000000008b7, // 18: r8 = 0
            0x00000000fff08a7b, // 19: *(u64 *)(r10 - 16) = r8
            0x00000000000063bf, // 20: r3 = r6
            0x0000006000000307, // 21: r3 += 0x60 (&regs->rdx = count)
            0x000000000000a1bf, // 22: r1 = r10
            0xfffffff000000107, // 23: r1 += -16
            0x00000008000002b4, // 24: w2 = 8
            0x0000007100000085, // 25: call bpf_probe_read_kernel
            0x00000000fff0a179, // 26: r1 = *(u64 *)(r10 - 16) [count]
            0x0000000000200115, // 27: if r1 == 0 goto exit
            0x00000000ffe88a7b, // 28: *(u64 *)(r10 - 24) = r8
            0x0000006800000607, // 29: r6 += 0x68 (&regs->rsi = buf)
            0x000000000000a1bf, // 30: r1 = r10
            0xffffffe800000107, // 31: r1 += -24
            0x00000008000002b4, // 32: w2 = 8
            0x00000000000063bf, // 33: r3 = r6
            0x0000007100000085, // 34: call bpf_probe_read_kernel
            0x00000000ffe8a179, // 35: r1 = *(u64 *)(r10 - 24) [buf]
            0x0000000000170115, // 36: if r1 == 0 goto exit
            0x0000000000001118 | ((map_fd as u64) << 32), // 37: ld_pseudo_map_fd r1, map_fd
            0x0000000000000000, // 38: (second half of lddw)
            0x0000010c000002b7, // 39: r2 = 268 (sizeof(struct event))
            0x00000000000003b7, // 40: r3 = 0
            0x0000008300000085, // 41: call bpf_ringbuf_reserve
            0x0000000000110015, // 42: if r0 == 0 goto exit
            0x0000000000007063, // 43: *(u32 *)(r0 + 0) = w7 [pid]
            0x00000000fff8a179, // 44: r1 = *(u64 *)(r10 - 8) [fd]
            0x0000000000041063, // 45: *(u32 *)(r0 + 4) = w1 [fd]
            0x00000000fff0a179, // 46: r1 = *(u64 *)(r10 - 16) [count]
            0x000000ff000002b4, // 47: w2 = 255
            0x000000ff00010126, // 48: if w1 > 255 goto +1
            0x00000000000012bc, // 49: w2 = w1
            0x0000000000082063, // 50: *(u32 *)(r0 + 8) = w2 [len]
            0x000000ff00000254, // 51: w2 &= 0xff
            0x00000000ffe8a379, // 52: r3 = *(u64 *)(r10 - 24) [buf]
            0x00000000000001bf, // 53: r1 = r0
            0x0000000c00000107, // 54: r1 += 12 (&e->data)
            0x00000000000006bf, // 55: r6 = r0
            0x0000007000000085, // 56: call bpf_probe_read_user
            0x00000000000061bf, // 57: r1 = r6
            0x00000000000002b7, // 58: r2 = 0
            0x0000008400000085, // 59: call bpf_ringbuf_submit
            0x00000000000000b4, // 60: w0 = 0
            0x0000000000000095, // 61: exit
        ];

        let license = b"GPL\0";
        let prog_attr = BpfAttrProgLoad {
            prog_type: BPF_PROG_TYPE_RAW_TRACEPOINT,
            insn_cnt: insns.len() as u32,
            insns: insns.as_mut_ptr() as u64,
            license: license.as_ptr() as u64,
            log_level: 0,
            log_size: 0,
            log_buf: 0,
            kern_version: 0,
            prog_flags: 0,
            prog_name: *b"err_det_write\0\0\0",
        };

        let prog_fd = unsafe {
            syscall(
                SYS_BPF,
                BPF_PROG_LOAD,
                &prog_attr as *const BpfAttrProgLoad,
                size_of::<BpfAttrProgLoad>(),
            )
        } as i32;
        if prog_fd < 0 {
            let err = io::Error::last_os_error();
            unsafe { close(map_fd) };
            return Err(err);
        }

        let tp_name = b"sys_enter\0";
        let tp_attr = BpfAttrRawTracepointOpen {
            name: tp_name.as_ptr() as u64,
            prog_fd: prog_fd as u32,
            _pad: 0,
        };

        let tp_fd = unsafe {
            syscall(
                SYS_BPF,
                BPF_RAW_TRACEPOINT_OPEN,
                &tp_attr as *const BpfAttrRawTracepointOpen,
                size_of::<BpfAttrRawTracepointOpen>(),
            )
        } as i32;
        if tp_fd < 0 {
            let err = io::Error::last_os_error();
            unsafe {
                close(prog_fd);
                close(map_fd);
            }
            return Err(err);
        }

        let consumer_ptr = unsafe {
            mmap(
                ptr::null_mut(),
                page_size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                map_fd,
                0,
            )
        };
        if consumer_ptr == MAP_FAILED {
            let err = io::Error::last_os_error();
            unsafe {
                close(tp_fd);
                close(prog_fd);
                close(map_fd);
            }
            return Err(err);
        }

        let producer_map_len = page_size + 2 * ring_size;
        let producer_ptr = unsafe {
            mmap(
                ptr::null_mut(),
                producer_map_len,
                PROT_READ,
                MAP_SHARED,
                map_fd,
                page_size as i64,
            )
        };
        if producer_ptr == MAP_FAILED {
            let err = io::Error::last_os_error();
            unsafe {
                munmap(consumer_ptr, page_size);
                close(tp_fd);
                close(prog_fd);
                close(map_fd);
            }
            return Err(err);
        }

        Ok(Self {
            map_fd,
            prog_fd,
            tp_fd,
            consumer_ptr,
            producer_ptr,
            ring_size,
            page_size,
        })
    }

    fn consume<F>(&mut self, mut callback: F)
    where
        F: FnMut(i32, u32, &[u8]),
    {
        let mask = (self.ring_size - 1) as u64;
        let cons_pos_ptr = self.consumer_ptr.cast::<u64>();
        let prod_pos_ptr = self.producer_ptr.cast::<u64>();
        let data_base = unsafe { self.producer_ptr.add(self.page_size) };

        let mut cons_pos = unsafe { ptr::read_volatile(cons_pos_ptr) };
        let prod_pos = unsafe { ptr::read_volatile(prod_pos_ptr) };
        compiler_fence(Ordering::Acquire);

        while cons_pos < prod_pos {
            let hdr_ptr = unsafe { data_base.add((cons_pos & mask) as usize).cast::<u32>() };
            let len_word = unsafe { ptr::read_volatile(hdr_ptr) };
            compiler_fence(Ordering::Acquire);

            if (len_word & BPF_RINGBUF_BUSY_BIT) != 0 {
                break;
            }

            let raw_len = (len_word & !(BPF_RINGBUF_BUSY_BIT | BPF_RINGBUF_DISCARD_BIT)) as usize;
            let step = ((raw_len + BPF_RINGBUF_HDR_SZ + 7) & !7) as u64;

            if (len_word & BPF_RINGBUF_DISCARD_BIT) == 0 && raw_len >= 12 {
                let sample_ptr =
                    unsafe { data_base.add(((cons_pos + BPF_RINGBUF_HDR_SZ as u64) & mask) as usize) };
                let pid = unsafe { ptr::read_unaligned(sample_ptr.cast::<i32>()) };
                let fd = unsafe { ptr::read_unaligned(sample_ptr.add(4).cast::<u32>()) };
                let data_len =
                    unsafe { ptr::read_unaligned(sample_ptr.add(8).cast::<u32>()) } as usize;
                let safe_len = data_len.min(255).min(raw_len.saturating_sub(12));
                if safe_len > 0 {
                    let bytes =
                        unsafe { std::slice::from_raw_parts(sample_ptr.add(12), safe_len) };
                    callback(pid, fd, bytes);
                }
            }

            cons_pos += step;
            compiler_fence(Ordering::Release);
            unsafe { ptr::write_volatile(cons_pos_ptr, cons_pos) };
        }
    }
}

impl Drop for EbpfWriteTracer {
    fn drop(&mut self) {
        unsafe {
            munmap(self.producer_ptr, self.page_size + 2 * self.ring_size);
            munmap(self.consumer_ptr, self.page_size);
            close(self.tp_fd);
            close(self.prog_fd);
            close(self.map_fd);
        }
    }
}

fn read_regular_file_tail(path: &str) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let len = meta.len();
    if len > MAX_STREAM_BYTES as u64 {
        let _ = file.seek(SeekFrom::End(-(MAX_STREAM_BYTES as i64)));
    }
    let mut buf = Vec::new();
    file.take(MAX_STREAM_BYTES as u64).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).trim().to_string())
}

fn query_journalctl_since_boot(pid: i32, systemd_unit: Option<&str>) -> (String, String) {
    let mut stdout_lines = Vec::new();
    let mut stderr_lines = Vec::new();

    // Query by _PID=<pid> since the last power on (--boot=0)
    collect_journal_json_lines(
        &[
            "--boot=0",
            "--no-pager",
            "--output=json",
            &format!("_PID={pid}"),
        ],
        &mut stdout_lines,
        &mut stderr_lines,
    );

    // If no entries were logged directly under _PID (e.g. short-lived worker in a service unit)
    // and we have a specific service unit, query the service unit's logs since boot
    if stdout_lines.is_empty() && stderr_lines.is_empty() {
        if let Some(unit) = systemd_unit {
            if is_specific_service_unit(unit) {
                collect_journal_json_lines(
                    &["--boot=0", "--no-pager", "--output=json", "-u", unit],
                    &mut stdout_lines,
                    &mut stderr_lines,
                );
            }
        }
    }

    (stdout_lines.join("\n"), stderr_lines.join("\n"))
}

fn is_specific_service_unit(unit: &str) -> bool {
    unit.ends_with(".service")
        && !unit.starts_with("user@")
        && unit != "init.scope"
        && !unit.starts_with("dbus")
}

fn collect_journal_json_lines(
    args: &[&str],
    stdout_lines: &mut Vec<String>,
    stderr_lines: &mut Vec<String>,
) {
    let Ok(output) = Command::new("journalctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };

    if !output.status.success() || output.stdout.is_empty() {
        return;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Some(msg) = extract_json_string_field(line, "MESSAGE") else {
            continue;
        };
        if msg.is_empty() {
            continue;
        }
        let syslog_fd = extract_json_string_field(line, "SYSLOG_FD");
        let priority = extract_json_string_field(line, "PRIORITY")
            .and_then(|p| p.parse::<u8>().ok())
            .unwrap_or(6);

        if syslog_fd.as_deref() == Some("2") || priority <= 3 {
            stderr_lines.push(msg);
        } else {
            stdout_lines.push(msg);
        }
    }
}

/// Lightweight extractor for string fields in single-line `journalctl -o json` records.
pub fn extract_json_string_field(json_line: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\":\"");
    let start = json_line.find(&pattern)? + pattern.len();
    let rest = &json_line[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_string_field() {
        let line = r#"{"_PID":"1234","PRIORITY":"3","SYSLOG_FD":"2","MESSAGE":"Failed to bind port: \"8080\"\nExiting."}"#;
        assert_eq!(
            extract_json_string_field(line, "_PID").as_deref(),
            Some("1234")
        );
        assert_eq!(
            extract_json_string_field(line, "PRIORITY").as_deref(),
            Some("3")
        );
        assert_eq!(
            extract_json_string_field(line, "SYSLOG_FD").as_deref(),
            Some("2")
        );
        assert_eq!(
            extract_json_string_field(line, "MESSAGE").as_deref(),
            Some("Failed to bind port: \"8080\"\nExiting.")
        );
    }
}

