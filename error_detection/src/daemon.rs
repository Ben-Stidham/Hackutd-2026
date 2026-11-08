use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::io::IntoRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

unsafe extern "C" {
    fn fork() -> i32;
    fn setsid() -> i32;
    fn umask(mask: u32) -> u32;
    fn chdir(path: *const u8) -> i32;
    fn dup2(oldfd: i32, newfd: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(signum: i32, handler: usize) -> usize;
    fn _exit(status: i32) -> !;
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_shutdown_signal(_sig: i32) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn install_signal_handlers() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    unsafe {
        signal(SIGINT, handle_shutdown_signal as *const () as usize);
        signal(SIGTERM, handle_shutdown_signal as *const () as usize);
    }
}

pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Performs a POSIX double-fork to detach the daemon into the background,
/// redirects stdin to `/dev/null` and stdout/stderr to `log_path`, and writes the daemon PID to `pid_path`.
pub fn daemonize(pid_path: &Path, log_path: &Path) -> io::Result<()> {
    if let Some(existing_pid) = read_running_pid(pid_path) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Daemon is already running with PID {existing_pid}"),
        ));
    }

    if let Some(parent) = pid_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let dev_null = OpenOptions::new().read(true).write(true).open("/dev/null")?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    // First fork
    let pid1 = unsafe { fork() };
    if pid1 < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid1 > 0 {
        // Parent exits cleanly
        unsafe { _exit(0) };
    }

    // Become session leader
    if unsafe { setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }

    // Second fork so the daemon cannot re-acquire a controlling terminal
    let pid2 = unsafe { fork() };
    if pid2 < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid2 > 0 {
        unsafe { _exit(0) };
    }

    unsafe {
        umask(0o022);
        let root = b"/\0";
        let _ = chdir(root.as_ptr());
    }

    let null_fd = dev_null.into_raw_fd();
    let log_fd = log_file.into_raw_fd();

    unsafe {
        dup2(null_fd, 0);
        dup2(log_fd, 1);
        dup2(log_fd, 2);
        if null_fd > 2 {
            close(null_fd);
        }
        if log_fd > 2 {
            close(log_fd);
        }
    }

    write_pid_file(pid_path)?;
    Ok(())
}

pub fn write_pid_file(pid_path: &Path) -> io::Result<()> {
    if let Some(parent) = pid_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let pid = std::process::id();
    fs::write(pid_path, format!("{pid}\n"))
}

pub fn remove_pid_file(pid_path: &Path) {
    let _ = fs::remove_file(pid_path);
}

pub fn read_running_pid(pid_path: &Path) -> Option<i32> {
    let content = fs::read_to_string(pid_path).ok()?;
    let pid = content.trim().parse::<i32>().ok()?;
    if pid > 0 && is_process_alive(pid) {
        Some(pid)
    } else {
        None
    }
}

fn is_process_alive(pid: i32) -> bool {
    let res = unsafe { kill(pid, 0) };
    if res == 0 {
        return true;
    }
    let err = io::Error::last_os_error();
    err.raw_os_error() == Some(1) // EPERM means process exists
}

pub fn stop_daemon(pid_path: &Path) -> io::Result<i32> {
    let Some(pid) = read_running_pid(pid_path) else {
        let _ = fs::remove_file(pid_path);
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Daemon is not currently running",
        ));
    };

    let res = unsafe { kill(pid, SIGTERM) };
    if res < 0 {
        return Err(io::Error::last_os_error());
    }

    for _ in 0..30 {
        if !is_process_alive(pid) {
            let _ = fs::remove_file(pid_path);
            return Ok(pid);
        }
        thread::sleep(Duration::from_millis(100));
    }

    Ok(pid)
}

pub fn log_message(msg: &str) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "[ts={secs}] {msg}");
    let _ = out.flush();
}

