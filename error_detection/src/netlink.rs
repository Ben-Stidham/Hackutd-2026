use std::collections::HashMap;
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
const SO_RCVTIMEO: i32 = 20;

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

#[derive(Clone, Debug)]
pub struct ProcessMetadata {
    pub comm: String,
    pub cmdline: String,
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
}

pub struct ProcEventMonitor {
    sock_fd: i32,
    cache: HashMap<i32, ProcessMetadata>,
}

impl ProcEventMonitor {
    pub fn new() -> io::Result<Self> {
        let fd = unsafe { socket(AF_NETLINK as i32, SOCK_DGRAM | SOCK_CLOEXEC, NETLINK_CONNECTOR) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // Set a 1-second receive timeout so the daemon loop can check shutdown flags periodically
        let tv = Timeval {
            tv_sec: 1,
            tv_usec: 0,
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
            if let Ok(pid) = name_str.parse::<i32>() {
                if let Some(meta) = read_proc_metadata(pid) {
                    self.cache.insert(pid, meta);
                }
            }
        }
    }

    /// Reads the next batch of netlink messages and returns any process exit events.
    pub fn poll_events(&mut self) -> io::Result<Vec<ProcessExitEvent>> {
        let mut buf = [0u8; 8192];
        let n = unsafe { recv(self.sock_fd, buf.as_mut_ptr(), buf.len(), 0) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock
                || err.kind() == io::ErrorKind::TimedOut
                || err.kind() == io::ErrorKind::Interrupted
            {
                return Ok(Vec::new());
            }
            return Err(err);
        }

        let total_len = n as usize;
        let mut offset = 0usize;
        let mut exits = Vec::new();

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
                    if err_code != 0 {
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
                let cn_len = u16::from_ne_bytes([buf[cn_offset + 16], buf[cn_offset + 17]]) as usize;

                let ev_offset = cn_offset + size_of::<CnMsgHdr>();
                if idx == CN_IDX_PROC
                    && val == CN_VAL_PROC
                    && ev_offset + cn_len <= offset + nlmsg_len
                    && cn_len >= 16
                {
                    if let Some(exit_ev) =
                        self.handle_proc_event(&buf[ev_offset..ev_offset + cn_len])
                    {
                        exits.push(exit_ev);
                    }
                }
            }

            offset += nlmsg_align(nlmsg_len);
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
                    if let Some(meta) = read_proc_metadata(child_tgid) {
                        self.cache.insert(child_tgid, meta);
                    } else if let Some(parent_meta) = self.cache.get(&parent_tgid).cloned() {
                        self.cache.insert(child_tgid, parent_meta);
                    }
                }
                None
            }
            PROC_EVENT_EXEC if payload.len() >= 8 => {
                let process_tgid =
                    i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                if let Some(meta) = read_proc_metadata(process_tgid) {
                    self.cache.insert(process_tgid, meta);
                }
                None
            }
            PROC_EVENT_COMM if payload.len() >= 24 => {
                let process_tgid =
                    i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
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
                        cmdline: String::new(),
                    });
                entry.comm = comm;
                if entry.cmdline.is_empty() {
                    if let Some(fresh) = read_proc_metadata(process_tgid) {
                        entry.cmdline = fresh.cmdline;
                    }
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

                let cached = self
                    .cache
                    .remove(&process_tgid)
                    .or_else(|| read_proc_metadata(process_tgid));

                let (comm, cmdline) = match cached {
                    Some(meta) => (meta.comm, meta.cmdline),
                    None => (format!("pid-{process_tgid}"), String::new()),
                };

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
                    comm,
                    cmdline,
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

pub fn read_proc_metadata(pid: i32) -> Option<ProcessMetadata> {
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

    Some(ProcessMetadata { comm, cmdline })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_proc_event_comm_and_exit() {
        let mut monitor = ProcEventMonitor {
            sock_fd: -1,
            cache: HashMap::new(),
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
}

