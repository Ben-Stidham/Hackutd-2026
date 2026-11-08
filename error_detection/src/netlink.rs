use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::mem::size_of;

const AF_NETLINK: u16 = 16;
const SOCK_DGRAM: i32 = 2;
const SOCK_CLOEXEC: i32 = 0o2000000;
const NETLINK_CONNECTOR: i32 = 11;

const NLMSG_NOOP: u16 = 0x1;
const NLMSG_ERROR: u16 = 0x2;
const NLMSG_DONE: u16 = 0x3;
const NLMSG_OVERRUN: u16 = 0x4;

const CN_IDX_PROC: u32 = 0x1;
const CN_VAL_PROC: u32 = 0x1;

const PROC_CN_MCAST_LISTEN: u32 = 1;
const PROC_CN_MCAST_IGNORE: u32 = 2;

const PROC_EVENT_FORK: u32 = 0x0000_0001;
const PROC_EVENT_EXEC: u32 = 0x0000_0002;
const PROC_EVENT_COMM: u32 = 0x0000_0200;
const PROC_EVENT_EXIT: u32 = 0x8000_0000;

unsafe extern "C" {
    fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
    fn bind(sockfd: i32, addr: *const SockaddrNl, addrlen: u32) -> i32;
    fn send(sockfd: i32, buf: *const u8, len: usize, flags: i32) -> isize;
    fn recv(sockfd: i32, buf: *mut u8, len: usize, flags: i32) -> isize;
    fn close(fd: i32) -> i32;
    fn setsockopt(
        sockfd: i32,
        level: i32,
        optname: i32,
        optval: *const u8,
        optlen: u32,
    ) -> i32;
}

const SOL_SOCKET: i32 = 1;
const SO_RCVBUF: i32 = 8;
const SO_RCVTIMEO: i32 = 20;
const SO_RCVBUFFORCE: i32 = 33;
const MSG_DONTWAIT: i32 = 0x40;
const ENOBUFS: i32 = 105;
const NETLINK_RCVBUF_BYTES: i32 = 16 * 1024 * 1024;
const MAX_BATCH_DATAGRAMS: usize = 512;

#[repr(C)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i64,
}

#[repr(C)]
struct SockaddrNl {
    nl_family: u16,
    nl_pad: u16,
    nl_pid: u32,
    nl_groups: u32,
}

#[repr(C, packed)]
struct NlMsgHdr {
    nlmsg_len: u32,
    nlmsg_type: u16,
    nlmsg_flags: u16,
    nlmsg_seq: u32,
    nlmsg_pid: u32,
}

#[repr(C, packed)]
struct CbId {
    idx: u32,
    val: u32,
}

#[repr(C, packed)]
struct CnMsgHdr {
    id: CbId,
    seq: u32,
    ack: u32,
    len: u16,
    flags: u16,
}

#[repr(C, packed)]
struct SubscriptionRequest {
    nlhdr: NlMsgHdr,
    cnhdr: CnMsgHdr,
    mcast_op: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessMetadata {
    pub comm: String,
    pub cmdline: String,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub systemd_unit: Option<String>,
    pub inherited_from_parent: bool,
}

#[derive(Debug, Clone)]
pub struct ProcessExitEvent {
    pub pid: i32,
    pub tgid: i32,
    pub parent_pid: i32,
    pub exit_code: i32,
    pub exit_signal: Option<i32>,
    pub raw_status: u32,
    pub comm: String,
    pub cmdline: String,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub systemd_unit: Option<String>,
    /// True when the process was (transitively) spawned by the daemon itself
    pub spawned_by_daemon: bool,
}

pub struct ProcEventMonitor {
    sock_fd: i32,
    cache: HashMap<i32, ProcessMetadata>,
    /// TGID of the daemon itself
    self_tgid: i32,
    /// Every live process descended from the daemon (journalctl, rsync and its workers,
    /// `sh -c` from `run_command` and everything those spawn).
    daemon_descendants: HashSet<i32>,
    /// Number of times the kernel reported that events were dropped (ENOBUFS)
    pub dropped_batches: u64,
}

impl ProcEventMonitor {
    pub fn new() -> io::Result<Self> {
        let fd = unsafe { socket(AF_NETLINK as i32, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_CONNECTOR) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set a 25ms receive timeout so the daemon loop stays responsive to eBPF ringbuffers and shutdown
        let tv = Timeval {
            tv_sec: 0,
            tv_usec: 25_000,
        };
        unsafe {
            setsockopt(
                fd,
                SOL_SOCKET,
                SO_RCVTIMEO,
                (&tv as *const Timeval).cast::<u8>(),
                size_of::<Timeval>() as u32,
            );
        }

        // Every fork/exec/exit/thread on the system produces an event. The default (~208 KiB)
        // receive buffer overflows within milliseconds while the main loop is busy collecting
        // output for a failed process, silently dropping exit events. Enlarge it (FORCE needs
        // CAP_NET_ADMIN, which we already require; fall back to the capped SO_RCVBUF).
        let rcvbuf: i32 = NETLINK_RCVBUF_BYTES;
        unsafe {
            let p = (&rcvbuf as *const i32).cast::<u8>();
            if setsockopt(fd, SOL_SOCKET, SO_RCVBUFFORCE, p, size_of::<i32>() as u32) < 0 {
                setsockopt(fd, SOL_SOCKET, SO_RCVBUF, p, size_of::<i32>() as u32);
            }
        }

        let addr = SockaddrNl {
            nl_family: AF_NETLINK,
            nl_pad: 0,
            nl_pid: std::process::id(),
            nl_groups: CN_IDX_PROC,
        };

        let bind_res = unsafe { bind(fd, &addr, size_of::<SockaddrNl>() as u32) };
        if bind_res < 0 {
            let err = io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(err);
        }

        let mut monitor = Self {
            sock_fd: fd,
            cache: HashMap::new(),
            self_tgid: std::process::id() as i32,
            daemon_descendants: HashSet::new(),
            dropped_batches: 0,
        };

        monitor.set_multicast_listen(true)?;
        monitor.seed_proc_cache();

        Ok(monitor)
    }

    fn set_multicast_listen(&self, enable: bool) -> io::Result<()> {
        let req = SubscriptionRequest {
            nlhdr: NlMsgHdr {
                nlmsg_len: size_of::<SubscriptionRequest>() as u32,
                nlmsg_type: NLMSG_DONE,
                nlmsg_flags: 0,
                nlmsg_seq: 0,
                nlmsg_pid: std::process::id(),
            },
            cnhdr: CnMsgHdr {
                id: CbId {
                    idx: CN_IDX_PROC,
                    val: CN_VAL_PROC,
                },
                seq: 0,
                ack: 0,
                len: size_of::<u32>() as u16,
                flags: 0,
            },
            mcast_op: if enable {
                PROC_CN_MCAST_LISTEN
            } else {
                PROC_CN_MCAST_IGNORE
            },
        };

        let sent = unsafe {
            send(
                self.sock_fd,
                (&req as *const SubscriptionRequest).cast::<u8>(),
                size_of::<SubscriptionRequest>(),
                0,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn seed_proc_cache(&mut self) {
        let Ok(entries) = fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if let Ok(pid) = name_str.parse::<i32>()
                && let Some(meta) = read_proc_metadata(pid) {
                    self.cache.insert(pid, meta);
                }
        }
    }

    /// Reads all currently queued netlink datagrams (up to `MAX_BATCH_DATAGRAMS`) and returns any process exit events.
    pub fn poll_events(&mut self) -> io::Result<Vec<ProcessExitEvent>> {
        let mut buf = [0u8; 8192];
        let mut exits = Vec::new();

        for pkt_idx in 0..MAX_BATCH_DATAGRAMS {
            let flags = if pkt_idx == 0 { 0 } else { MSG_DONTWAIT };
            let n = unsafe { recv(self.sock_fd, buf.as_mut_ptr(), buf.len(), flags) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::TimedOut
                    || err.kind() == io::ErrorKind::Interrupted
                {
                    break;
                }
                if err.raw_os_error() == Some(ENOBUFS) {
                    // Kernel dropped events because we fell behind; keep draining remaining packets
                    self.dropped_batches += 1;
                    continue;
                }
                if pkt_idx > 0 {
                    break;
                }
                return Err(err);
            }

            let total_len = n as usize;
            let mut offset = 0usize;

            while offset + size_of::<NlMsgHdr>() <= total_len {
                let nlmsg_len = u32::from_ne_bytes([
                    buf[offset],
                    buf[offset + 1],
                    buf[offset + 2],
                    buf[offset + 3],
                ]) as usize;
                let nlmsg_type = u16::from_ne_bytes([buf[offset + 4], buf[offset + 5]]);

                if nlmsg_len < size_of::<NlMsgHdr>() || offset + nlmsg_len > total_len {
                    break;
                }

                if nlmsg_type == NLMSG_NOOP || nlmsg_type == NLMSG_OVERRUN {
                    offset += nlmsg_align(nlmsg_len);
                    continue;
                }
                if nlmsg_type == NLMSG_ERROR {
                    if offset + size_of::<NlMsgHdr>() + 4 <= total_len {
                        let err_code = i32::from_ne_bytes([
                            buf[offset + 16],
                            buf[offset + 17],
                            buf[offset + 18],
                            buf[offset + 19],
                        ]);
                        if err_code != 0 && -err_code != ENOBUFS {
                            return Err(io::Error::from_raw_os_error(-err_code));
                        }
                    }
                    offset += nlmsg_align(nlmsg_len);
                    continue;
                }

                let cn_offset = offset + size_of::<NlMsgHdr>();
                if cn_offset + size_of::<CnMsgHdr>() <= offset + nlmsg_len {
                    let idx = u32::from_ne_bytes([
                        buf[cn_offset],
                        buf[cn_offset + 1],
                        buf[cn_offset + 2],
                        buf[cn_offset + 3],
                    ]);
                    let val = u32::from_ne_bytes([
                        buf[cn_offset + 4],
                        buf[cn_offset + 5],
                        buf[cn_offset + 6],
                        buf[cn_offset + 7],
                    ]);
                    let cn_len =
                        u16::from_ne_bytes([buf[cn_offset + 16], buf[cn_offset + 17]]) as usize;

                    let ev_offset = cn_offset + size_of::<CnMsgHdr>();
                    if idx == CN_IDX_PROC
                        && val == CN_VAL_PROC
                        && ev_offset + cn_len <= offset + nlmsg_len
                        && cn_len >= 16
                        && let Some(exit_ev) =
                            self.handle_proc_event(&buf[ev_offset..ev_offset + cn_len])
                    {
                        exits.push(exit_ev);
                    }
                }

                offset += nlmsg_align(nlmsg_len);
            }
        }

        Ok(exits)
    }

    fn handle_proc_event(&mut self, data: &[u8]) -> Option<ProcessExitEvent> {
        // struct proc_event header:
        // 0..4: what (u32)
        // 4..8: cpu (u32)
        // 8..16: timestamp_ns (u64)
        // 16..: event_data union
        let what = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        let payload = &data[16..];

        match what {
            PROC_EVENT_FORK if payload.len() >= 16 => {
                let parent_tgid = i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let child_pid = i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                let child_tgid =
                    i32::from_ne_bytes([payload[12], payload[13], payload[14], payload[15]]);

                if child_pid == child_tgid {
                    let parent_meta = self.cache.get(&parent_tgid).cloned().or_else(|| {
                        let m = read_proc_metadata(parent_tgid)?;
                        self.cache.insert(parent_tgid, m.clone());
                        Some(m)
                    });
                    if let Some(mut meta) = parent_meta {
                        meta.inherited_from_parent = true;
                        self.cache.insert(child_tgid, meta);
                    } else if let Some(meta) = read_proc_metadata(child_tgid) {
                        self.cache.insert(child_tgid, meta);
                    }
                    if parent_tgid == self.self_tgid
                        || self.daemon_descendants.contains(&parent_tgid)
                    {
                        self.daemon_descendants.insert(child_tgid);
                    }
                }
                None
            }
            PROC_EVENT_EXEC if payload.len() >= 8 => {
                let process_tgid =
                    i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                if let Some(meta) = read_proc_metadata(process_tgid) {
                    self.cache.insert(process_tgid, meta);
                } else if let Some(existing) = self.cache.get_mut(&process_tgid)
                    && existing.inherited_from_parent
                {
                    // The child called execve() and exited before /proc/<pid> could be read.
                    // If the parent was a wrapper like `sudo pacman --dbpath ...`, unwrap the
                    // target command; otherwise clear the inherited parent comm so eBPF can fill it
                    // and so the child never collides with its parent's comm in SQLite.
                    if let Some((wrapped_comm, wrapped_cmdline)) =
                        unwrap_wrapper_cmdline(&existing.comm, &existing.cmdline)
                    {
                        existing.comm = wrapped_comm;
                        existing.cmdline = wrapped_cmdline;
                    } else {
                        existing.comm = format!("pid-{process_tgid}");
                        existing.cmdline.clear();
                    }
                    existing.inherited_from_parent = false;
                }
                None
            }
            PROC_EVENT_COMM if payload.len() >= 24 => {
                let process_pid =
                    i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let process_tgid =
                    i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                // Ignore thread renames (e.g. worker threads) so they don't overwrite the process name
                if process_pid != process_tgid {
                    return None;
                }
                let comm_bytes = &payload[8..24];
                let nul_pos = comm_bytes
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(comm_bytes.len());
                let comm = String::from_utf8_lossy(&comm_bytes[..nul_pos]).into_owned();
                let entry = self
                    .cache
                    .entry(process_tgid)
                    .or_insert_with(|| ProcessMetadata {
                        comm: comm.clone(),
                        ..Default::default()
                    });
                entry.comm = comm;
                entry.inherited_from_parent = false;
                if entry.cmdline.is_empty()
                    && let Some(fresh) = read_proc_metadata(process_tgid) {
                        entry.cmdline = fresh.cmdline;
                        entry.stdout_path = fresh.stdout_path;
                        entry.stderr_path = fresh.stderr_path;
                        entry.systemd_unit = fresh.systemd_unit;
                    }
                None
            }
            PROC_EVENT_EXIT if payload.len() >= 16 => {
                let process_pid =
                    i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let process_tgid =
                    i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let raw_exit_code =
                    u32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                let parent_pid = if payload.len() >= 24 {
                    i32::from_ne_bytes([payload[20], payload[21], payload[22], payload[23]])
                } else {
                    0
                };

                // Only report process exits (thread group leader), not individual sub-thread exits
                if process_pid != process_tgid {
                    return None;
                }

                let spawned_by_daemon =
                    self.daemon_descendants.remove(&process_tgid) || parent_pid == self.self_tgid;

                let cached = self
                    .cache
                    .remove(&process_tgid)
                    .or_else(|| read_proc_metadata(process_tgid));

                let mut meta = cached.unwrap_or_else(|| ProcessMetadata {
                    comm: format!("pid-{process_tgid}"),
                    ..Default::default()
                });

                // If this process still has inherited metadata from a wrapper parent (e.g. `sudo`),
                // unwrap the target command so the exit event is recorded under the actual command (`pacman`).
                if meta.inherited_from_parent {
                    if let Some((wrapped_comm, wrapped_cmdline)) =
                        unwrap_wrapper_cmdline(&meta.comm, &meta.cmdline)
                    {
                        meta.comm = wrapped_comm;
                        meta.cmdline = wrapped_cmdline;
                    }
                    meta.inherited_from_parent = false;
                }

                // Decode wait(2) status encoded in task->exit_code
                let low_7 = (raw_exit_code & 0x7f) as i32;
                let (exit_code, exit_signal) = if low_7 == 0 {
                    (((raw_exit_code >> 8) & 0xff) as i32, None)
                } else {
                    (128 + low_7, Some(low_7))
                };

                Some(ProcessExitEvent {
                    pid: process_pid,
                    tgid: process_tgid,
                    parent_pid,
                    exit_code,
                    exit_signal,
                    raw_status: raw_exit_code,
                    comm: meta.comm,
                    cmdline: meta.cmdline,
                    stdout_path: meta.stdout_path,
                    stderr_path: meta.stderr_path,
                    systemd_unit: meta.systemd_unit,
                    spawned_by_daemon,
                })
            }
            _ => None,
        }
    }
}

impl Drop for ProcEventMonitor {
    fn drop(&mut self) {
        if self.sock_fd >= 0 {
            let _ = self.set_multicast_listen(false);
            unsafe {
                close(self.sock_fd);
            }
        }
    }
}

fn nlmsg_align(len: usize) -> usize {
    (len + 3) & !3
}

fn read_proc_metadata(pid: i32) -> Option<ProcessMetadata> {
    let comm_path = format!("/proc/{pid}/comm");
    let comm = fs::read_to_string(comm_path).ok()?.trim().to_string();
    if comm.is_empty() {
        return None;
    }

    let cmdline_path = format!("/proc/{pid}/cmdline");
    let cmdline = match fs::read(cmdline_path) {
        Ok(bytes) if !bytes.is_empty() => {
            let parts: Vec<String> = bytes
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            parts.join(" ")
        }
        _ => String::new(),
    };

    let stdout_path = read_regular_fd_target(pid, 1);
    let stderr_path = read_regular_fd_target(pid, 2);
    let systemd_unit = read_systemd_unit(pid);

    Some(ProcessMetadata {
        comm,
        cmdline,
        stdout_path,
        stderr_path,
        systemd_unit,
        inherited_from_parent: false,
    })
}

/// If a parent wrapper (`sudo`, `doas`, `pkexec`, `env`, `nice`, `nohup`, `setsid`, `time`)
/// forked a child that called `execve` and exited before `/proc/<pid>` could be inspected,
/// extract the wrapped command's `comm` and `cmdline` from the wrapper's `cmdline`.
fn unwrap_wrapper_cmdline(parent_comm: &str, parent_cmdline: &str) -> Option<(String, String)> {
    if !matches!(
        parent_comm,
        "sudo" | "doas" | "pkexec" | "env" | "nice" | "nohup" | "setsid" | "time"
    ) {
        return None;
    }
    let tokens: Vec<&str> = parent_cmdline.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    let mut idx = 1;
    while idx < tokens.len() {
        let tok = tokens[idx];
        if tok == "--" {
            idx += 1;
            break;
        }
        if tok.starts_with('-') {
            // Flags on sudo/doas/pkexec/nice that consume a following argument
            if matches!(
                tok,
                "-u" | "--user"
                    | "-g"
                    | "--group"
                    | "-C"
                    | "-D"
                    | "-R"
                    | "-T"
                    | "-h"
                    | "-p"
                    | "-r"
                    | "-t"
                    | "-n"
            ) {
                idx += 2;
            } else {
                idx += 1;
            }
            continue;
        }
        if parent_comm == "env" && tok.contains('=') {
            idx += 1;
            continue;
        }
        break;
    }
    if idx >= tokens.len() {
        return None;
    }
    let cmd_token = tokens[idx];
    let comm = cmd_token
        .rsplit('/')
        .next()
        .unwrap_or(cmd_token)
        .chars()
        .take(15)
        .collect::<String>();
    if comm.is_empty() {
        return None;
    }
    let cmdline = tokens[idx..].join(" ");
    Some((comm, cmdline))
}

fn read_regular_fd_target(pid: i32, fd: i32) -> Option<String> {
    let link = fs::read_link(format!("/proc/{pid}/fd/{fd}")).ok()?;
    let path_str = link.to_str()?;
    let clean = path_str.strip_suffix(" (deleted)").unwrap_or(path_str);
    if clean.starts_with('/')
        && !clean.starts_with("/dev/")
        && !clean.starts_with("/proc/")
        && !clean.starts_with("/sys/")
    {
        Some(clean.to_string())
    } else {
        None
    }
}

fn read_systemd_unit(pid: i32) -> Option<String> {
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    for line in cgroup.lines() {
        if let Some(path) = line.split(':').nth(2)
            && let Some(unit) = path.rsplit('/').next()
                && !unit.is_empty() && (unit.ends_with(".service") || unit.ends_with(".scope")) {
                    return Some(unit.to_string());
                }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_proc_event_comm_and_exit() {
        let mut monitor = ProcEventMonitor {
            sock_fd: -1,
            cache: HashMap::new(),
            self_tgid: 999,
            daemon_descendants: HashSet::new(),
            dropped_batches: 0,
        };

        // Construct a PROC_EVENT_COMM payload (16-byte header + 24-byte comm_proc_event)
        let mut comm_data = vec![0u8; 40];
        comm_data[0..4].copy_from_slice(&PROC_EVENT_COMM.to_ne_bytes());
        comm_data[16..20].copy_from_slice(&5555i32.to_ne_bytes()); // process_pid
        comm_data[20..24].copy_from_slice(&5555i32.to_ne_bytes()); // process_tgid
        comm_data[24..35].copy_from_slice(b"failing_app");

        assert!(monitor.handle_proc_event(&comm_data).is_none());
        assert_eq!(
            monitor.cache.get(&5555).map(|m| m.comm.as_str()),
            Some("failing_app")
        );

        // A PROC_EVENT_COMM for a non-leader thread (pid != tgid) must not rename the process
        let mut thread_comm = vec![0u8; 40];
        thread_comm[0..4].copy_from_slice(&PROC_EVENT_COMM.to_ne_bytes());
        thread_comm[16..20].copy_from_slice(&5556i32.to_ne_bytes()); // thread pid
        thread_comm[20..24].copy_from_slice(&5555i32.to_ne_bytes()); // process_tgid
        thread_comm[24..35].copy_from_slice(b"worker-thrd");
        assert!(monitor.handle_proc_event(&thread_comm).is_none());
        assert_eq!(
            monitor.cache.get(&5555).map(|m| m.comm.as_str()),
            Some("failing_app")
        );

        // Construct a PROC_EVENT_EXIT payload (16-byte header + 24-byte exit_proc_event)
        let mut exit_data = vec![0u8; 40];
        exit_data[0..4].copy_from_slice(&PROC_EVENT_EXIT.to_ne_bytes());
        exit_data[16..20].copy_from_slice(&5555i32.to_ne_bytes()); // process_pid
        exit_data[20..24].copy_from_slice(&5555i32.to_ne_bytes()); // process_tgid
        let raw_exit: u32 = 42 << 8; // exit(42) in wait(2) status format
        exit_data[24..28].copy_from_slice(&raw_exit.to_ne_bytes());
        exit_data[36..40].copy_from_slice(&1000i32.to_ne_bytes()); // parent_tgid

        let ev = monitor
            .handle_proc_event(&exit_data)
            .expect("should produce exit event");
        assert_eq!(ev.tgid, 5555);
        assert_eq!(ev.exit_code, 42);
        assert_eq!(ev.exit_signal, None);
        assert_eq!(ev.comm, "failing_app");
        assert_eq!(ev.parent_pid, 1000);
    }

    #[test]
    fn test_unwrap_wrapper_cmdline() {
        assert_eq!(
            unwrap_wrapper_cmdline(
                "sudo",
                "sudo pacman --dbpath /tmp/pacman-troubleshooting-lab/db -Sy"
            ),
            Some((
                "pacman".to_string(),
                "pacman --dbpath /tmp/pacman-troubleshooting-lab/db -Sy".to_string()
            ))
        );
        assert_eq!(
            unwrap_wrapper_cmdline("sudo", "sudo -u root /usr/bin/pacman -Sy"),
            Some(("pacman".to_string(), "/usr/bin/pacman -Sy".to_string()))
        );
        assert_eq!(
            unwrap_wrapper_cmdline("sudo", "sudo -- pacman -Sy"),
            Some(("pacman".to_string(), "pacman -Sy".to_string()))
        );
        assert_eq!(
            unwrap_wrapper_cmdline("env", "env FOO=BAR /usr/bin/pacman -Sy"),
            Some(("pacman".to_string(), "/usr/bin/pacman -Sy".to_string()))
        );
    }
}

