mod backup;
mod daemon;
mod db;
mod netlink;
mod output_capture;
mod server;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use backup::BackupManager;
use daemon::{
    daemonize, install_signal_handlers, is_shutdown_requested, log_message, read_running_pid,
    remove_pid_file, stop_daemon, write_pid_file,
};
use db::ErrorDatabase;
use netlink::{ProcEventMonitor, ProcessExitEvent};
use output_capture::OutputCollector;
use server::SocketIoServerHandle;

const DEFAULT_PID_FILE: &str = "/tmp/error_detection.pid";
const DEFAULT_LOG_FILE: &str = "/tmp/error_detection.log";
const DEFAULT_DB_FILE: &str = "/tmp/error_detection.db";
const DEFAULT_PORT: u16 = 8032;

#[derive(Debug, PartialEq, Eq)]
enum Action {
    StartDaemon,
    RunForeground,
    Stop,
    Status,
    Logs,
    Help,
}

struct Config {
    action: Action,
    pid_file: PathBuf,
    log_file: PathBuf,
    db_file: PathBuf,
    port: u16,
    lv_path: Option<String>,
    snapshot_size: Option<String>,
    ignored_comms: HashSet<String>,
}

impl Config {
    fn from_args() -> Result<Self, String> {
        let mut action = Action::StartDaemon;
        let mut pid_file = PathBuf::from(
            env::var("ERROR_DETECTION_PID_FILE").unwrap_or_else(|_| DEFAULT_PID_FILE.to_string()),
        );
        let mut log_file = PathBuf::from(
            env::var("ERROR_DETECTION_LOG_FILE").unwrap_or_else(|_| DEFAULT_LOG_FILE.to_string()),
        );
        let mut db_file = PathBuf::from(
            env::var("ERROR_DETECTION_DB_FILE").unwrap_or_else(|_| DEFAULT_DB_FILE.to_string()),
        );
        let mut port = env::var("ERROR_DETECTION_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let mut lv_path = env::var("ERROR_DETECTION_LV_PATH").ok();
        let mut snapshot_size = env::var("ERROR_DETECTION_SNAPSHOT_SIZE").ok();
        let mut ignored_comms: HashSet<String> =
            ["journalctl", "lvs", "lvcreate", "lvremove", "lvconvert"]
                .into_iter()
                .map(String::from)
                .collect();

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "start" | "daemon" => action = Action::StartDaemon,
                "run" | "foreground" | "-f" | "--foreground" => action = Action::RunForeground,
                "stop" => action = Action::Stop,
                "status" => action = Action::Status,
                "logs" => action = Action::Logs,
                "-h" | "--help" | "help" => action = Action::Help,
                "--pid-file" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--pid-file requires a path argument".to_string())?;
                    pid_file = PathBuf::from(val);
                }
                "--log-file" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--log-file requires a path argument".to_string())?;
                    log_file = PathBuf::from(val);
                }
                "--db-file" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--db-file requires a path argument".to_string())?;
                    db_file = PathBuf::from(val);
                }
                "--port" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--port requires a port number".to_string())?;
                    port = val
                        .parse::<u16>()
                        .map_err(|_| format!("Invalid port number: '{val}'"))?;
                }
                "--lv-path" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--lv-path requires a VG/LV path argument".to_string())?;
                    lv_path = Some(val);
                }
                "--snapshot-size" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--snapshot-size requires a size argument (e.g. 5G)".to_string())?;
                    snapshot_size = Some(val);
                }
                "--ignore" => {
                    let val = args
                        .next()
                        .ok_or_else(|| "--ignore requires a comma-separated list".to_string())?;
                    for item in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        ignored_comms.insert(item.to_string());
                    }
                }
                other => {
                    return Err(format!("Unknown argument: '{other}'. Use --help for usage."));
                }
            }
        }

        Ok(Self {
            action,
            pid_file,
            log_file,
            db_file,
            port,
            lv_path,
            snapshot_size,
            ignored_comms,
        })
    }
}

fn print_usage() {
    println!(
        "\
error_detection - Background daemon monitoring system-wide process exits

USAGE:
    error_detection [COMMAND] [OPTIONS]

COMMANDS:
    start           Start as a background daemon (default)
    run, -f         Run in the foreground
    stop            Stop the running background daemon
    status          Show daemon status and recent logs
    logs            Query logged errors (and stdout/stderr) from the SQLite database
    help, --help    Show this help message

OPTIONS:
    --pid-file <PATH>       Path to PID file (default: {DEFAULT_PID_FILE})
    --log-file <PATH>       Path to log file (default: {DEFAULT_LOG_FILE})
    --db-file <PATH>        Path to SQLite database (default: {DEFAULT_DB_FILE})
    --port <PORT>           Localhost Socket.IO server port (default: {DEFAULT_PORT})
    --lv-path <VG/LV>       LVM volume to snapshot for backups (auto-detected if omitted)
    --snapshot-size <SIZE>  LVM snapshot size for lvcreate (default: 5G)
    --ignore <COMM,...>     Comma-separated process names to ignore

SOCKET.IO EVENTS (http://127.0.0.1:{DEFAULT_PORT}):
    list_errors / get_errors / list  Returns list of all failed processes (id, process_name, error_code)
    get_error / get                  Given id (or ids), returns full stored details for that failed process
    run_command / run                Executes a command string and returns exit_code, stdout, and stderr
    create_backup / backup           Creates/overwrites the server-triggered LVM snapshot backup
    restore_backup / restore         Restores the most recent LVM snapshot backup (unless target specified)

PERMISSIONS:
    Monitoring all system-wide process exits via the Linux Netlink Proc Connector
    requires CAP_NET_ADMIN or root privileges:
      sudo ./target/debug/error_detection start
    Or grant the capabilities once to run as a normal user:
      sudo setcap cap_net_admin,cap_bpf,cap_perfmon,cap_dac_read_search+ep ./target/debug/error_detection"
    );
}

fn main() -> ExitCode {
    let config = match Config::from_args() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };

    match config.action {
        Action::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        Action::Status => cmd_status(&config.pid_file, &config.log_file, &config.db_file),
        Action::Logs => cmd_logs(&config.db_file),
        Action::Stop => match stop_daemon(&config.pid_file) {
            Ok(pid) => {
                println!("Stopped error_detection daemon (PID {pid}).");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("Failed to stop daemon: {err}");
                ExitCode::FAILURE
            }
        },
        Action::StartDaemon => {
            if let Some(existing_pid) = read_running_pid(&config.pid_file) {
                eprintln!("error_detection daemon is already running (PID {existing_pid}).");
                return ExitCode::FAILURE;
            }

            // Pre-flight check to ensure CAP_NET_ADMIN / Netlink Proc Connector access before forking
            if let Err(err) = preflight_netlink_check() {
                print_netlink_permission_error(&err);
                return ExitCode::FAILURE;
            }

            println!(
                "Starting error_detection daemon (PID file: {}, Log file: {}, DB: {})...",
                config.pid_file.display(),
                config.log_file.display(),
                config.db_file.display()
            );

            if let Err(err) = daemonize(&config.pid_file, &config.log_file) {
                eprintln!("Failed to daemonize: {err}");
                return ExitCode::FAILURE;
            }

            run_monitor_loop(&config, true)
        }
        Action::RunForeground => {
            if let Err(err) = write_pid_file(&config.pid_file) {
                eprintln!(
                    "Warning: could not write PID file {}: {err}",
                    config.pid_file.display()
                );
            }
            run_monitor_loop(&config, false)
        }
    }
}

fn preflight_netlink_check() -> io::Result<()> {
    let _monitor = ProcEventMonitor::new()?;
    Ok(())
}

fn print_netlink_permission_error(err: &io::Error) {
    eprintln!("Failed to initialize Linux Netlink Proc Connector: {err}");
    if err.kind() == io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(1) {
        eprintln!(
            "Hint: Monitoring system-wide process exits requires root or CAP_NET_ADMIN.\n\
             Run with `sudo` or grant capability with:\n  \
             sudo setcap cap_net_admin,cap_bpf,cap_perfmon,cap_dac_read_search+ep {}",
            env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "error_detection".to_string())
        );
    }
}

fn cmd_status(pid_path: &Path, log_path: &Path, db_path: &Path) -> ExitCode {
    match read_running_pid(pid_path) {
        Some(pid) => {
            println!("error_detection daemon is RUNNING (PID {pid})");
            println!("  PID file: {}", pid_path.display());
            println!("  Log file: {}", log_path.display());
            println!("  Database: {}", db_path.display());
            if let Ok(logs) = fs::read_to_string(log_path) {
                let tail: Vec<&str> = logs.lines().rev().take(15).collect();
                if !tail.is_empty() {
                    println!("\nRecent log entries:");
                    for line in tail.into_iter().rev() {
                        println!("  {line}");
                    }
                }
            }
            ExitCode::SUCCESS
        }
        None => {
            println!("error_detection daemon is STOPPED");
            ExitCode::FAILURE
        }
    }
}

fn cmd_logs(db_path: &Path) -> ExitCode {
    let db = match ErrorDatabase::open_query(db_path) {
        Ok(d) => d,
        Err(err) => {
            eprintln!("Failed to open database {}: {err}", db_path.display());
            return ExitCode::FAILURE;
        }
    };

    match db.recent_errors(50) {
        Ok(records) if records.is_empty() => {
            println!(
                "No process errors recorded since last power on in {}.",
                db_path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(records) => {
            for rec in records {
                println!(
                    "--------------------------------------------------------------------------------\n\
                     [ID #{}] ts={} boot={}\n\
                     Process: '{}' (pid={}, tgid={}, ppid={}) | exit_code={} signal={:?}\n\
                     Command: {}\n\
                     Summary: {}\n\
                     Details:\n{}\n\
                     --- STDOUT ---\n{}\n\
                     --- STDERR ---\n{}",
                    rec.id,
                    rec.timestamp,
                    rec.boot_id,
                    rec.comm,
                    rec.pid,
                    rec.tgid,
                    rec.parent_pid,
                    rec.exit_code,
                    rec.exit_signal,
                    rec.cmdline,
                    rec.summary,
                    rec.notification_body,
                    if rec.stdout.is_empty() {
                        "<empty>"
                    } else {
                        &rec.stdout
                    },
                    if rec.stderr.is_empty() {
                        "<empty>"
                    } else {
                        &rec.stderr
                    },
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("Failed to query error logs: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_monitor_loop(config: &Config, is_daemon: bool) -> ExitCode {
    install_signal_handlers();

    let db = match ErrorDatabase::open(&config.db_file) {
        Ok(db) => db,
        Err(err) => {
            log_message(&format!(
                "FATAL: Failed to open SQLite database {}: {err}",
                config.db_file.display()
            ));
            remove_pid_file(&config.pid_file);
            return ExitCode::FAILURE;
        }
    };

    let mut monitor = match ProcEventMonitor::new() {
        Ok(m) => m,
        Err(err) => {
            if is_daemon {
                log_message(&format!("FATAL: Failed to start ProcEventMonitor: {err}"));
            } else {
                print_netlink_permission_error(&err);
            }
            remove_pid_file(&config.pid_file);
            return ExitCode::FAILURE;
        }
    };

    let self_pid = std::process::id() as i32;
    let mut output_collector = OutputCollector::new(self_pid as u32);

    let backup_manager = Arc::new(BackupManager::new(
        config.lv_path.clone(),
        config.snapshot_size.clone(),
    ));
    // Always create the startup backup on program startup, replacing any startup backup from prior runs
    let _ = backup_manager.create_startup_backup();

    let _socketio_server = match SocketIoServerHandle::start(
        config.port,
        config.db_file.clone(),
        Arc::clone(&backup_manager),
    ) {
        Ok(srv) => {
            log_message(&format!(
                "Socket.IO server listening on http://{} (localhost only)",
                srv.local_addr
            ));
            Some(srv)
        }
        Err(err) => {
            log_message(&format!(
                "Warning: failed to bind Socket.IO server on 127.0.0.1:{}: {err}",
                config.port
            ));
            None
        }
    };

    log_message(&format!(
        "error_detection started (pid={self_pid}, db={}, port={}, ebpf_capture={})",
        config.db_file.display(),
        config.port,
        output_collector.has_ebpf()
    ));

    while !is_shutdown_requested() {
        output_collector.poll_ebpf();

        match monitor.poll_events() {
            Ok(events) => {
                for ev in events {
                    if should_record_error(&ev, self_pid, &config.ignored_comms) {
                        let (summary, body) = format_exit_details(&ev);
                        let (stdout, stderr) = output_collector.collect_for_exit(&ev);

                        log_message(&format!(
                            "ERROR: {summary} | {} | raw=0x{:x}\n  stdout: {}\n  stderr: {}",
                            body.replace('\n', ", "),
                            ev.raw_status,
                            if stdout.is_empty() { "<empty>" } else { &stdout },
                            if stderr.is_empty() { "<empty>" } else { &stderr }
                        ));

                        if let Err(err) = db.insert_error(&ev, &summary, &body, &stdout, &stderr) {
                            log_message(&format!(
                                "Warning: failed to insert error into database for pid {}: {err}",
                                ev.tgid
                            ));
                        }
                    } else {
                        output_collector.discard_process(ev.tgid);
                    }
                }
            }
            Err(err) => {
                log_message(&format!("Error polling netlink events: {err}"));
            }
        }
    }

    log_message("Shutting down error_detection daemon.");
    remove_pid_file(&config.pid_file);
    ExitCode::SUCCESS
}

fn should_record_error(
    ev: &ProcessExitEvent,
    daemon_pid: i32,
    ignored_comms: &HashSet<String>,
) -> bool {
    if ev.exit_code == 0 {
        return false;
    }
    // Ignore the daemon itself or direct helper children spawned by the daemon (e.g. journalctl)
    if ev.tgid == daemon_pid || ev.parent_pid == daemon_pid {
        return false;
    }
    if ignored_comms.contains(&ev.comm) {
        return false;
    }
    true
}

fn format_exit_details(ev: &ProcessExitEvent) -> (String, String) {
    let summary = match ev.exit_signal {
        Some(sig) => format!(
            "Process '{}' (PID {}) killed by signal {}",
            ev.comm, ev.tgid, sig
        ),
        None => format!(
            "Process '{}' (PID {}) exited with code {}",
            ev.comm, ev.tgid, ev.exit_code
        ),
    };

    let body = if ev.cmdline.is_empty() {
        format!("PID: {}\nExit Code: {}", ev.tgid, ev.exit_code)
    } else {
        format!(
            "Command: {}\nPID: {}\nExit Code: {}",
            ev.cmdline, ev.tgid, ev.exit_code
        )
    };

    (summary, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> ProcessExitEvent {
        ProcessExitEvent {
            pid: 1234,
            tgid: 1234,
            parent_pid: 1,
            exit_code: 0,
            exit_signal: None,
            raw_status: 0,
            comm: "my_worker".to_string(),
            cmdline: "my_worker --job 1".to_string(),
            stdout_path: None,
            stderr_path: None,
            systemd_unit: None,
        }
    }

    #[test]
    fn test_should_record_error_filters_zero_and_daemon_children() {
        let ignored: HashSet<String> = ["journalctl"].into_iter().map(String::from).collect();

        let ok_ev = sample_event();
        assert!(!should_record_error(&ok_ev, 999, &ignored));

        let err_ev = ProcessExitEvent {
            exit_code: 1,
            raw_status: 1 << 8,
            ..ok_ev.clone()
        };
        assert!(should_record_error(&err_ev, 999, &ignored));

        let daemon_child = ProcessExitEvent {
            parent_pid: 999,
            ..err_ev.clone()
        };
        assert!(!should_record_error(&daemon_child, 999, &ignored));

        let ignored_proc = ProcessExitEvent {
            comm: "journalctl".to_string(),
            ..err_ev
        };
        assert!(!should_record_error(&ignored_proc, 999, &ignored));
    }

    #[test]
    fn test_format_exit_details() {
        let ev = ProcessExitEvent {
            pid: 4242,
            tgid: 4242,
            parent_pid: 1,
            exit_code: 2,
            exit_signal: None,
            raw_status: 2 << 8,
            comm: "backup-job".to_string(),
            cmdline: "/usr/local/bin/backup-job --full".to_string(),
            stdout_path: None,
            stderr_path: None,
            systemd_unit: None,
        };
        let (summary, body) = format_exit_details(&ev);
        assert_eq!(summary, "Process 'backup-job' (PID 4242) exited with code 2");
        assert!(body.contains("/usr/local/bin/backup-job --full"));
    }

    #[test]
    fn test_sqlite_database_insert_and_query() {
        let db_path = PathBuf::from(format!(
            "/tmp/test_error_detection_{}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&db_path);

        let db = ErrorDatabase::open(&db_path).expect("should open sqlite db");
        let ev = ProcessExitEvent {
            pid: 7777,
            tgid: 7777,
            parent_pid: 1,
            exit_code: 13,
            exit_signal: None,
            raw_status: 13 << 8,
            comm: "crashy".to_string(),
            cmdline: "crashy --now".to_string(),
            stdout_path: None,
            stderr_path: None,
            systemd_unit: None,
        };
        let (summary, body) = format_exit_details(&ev);
        db.insert_error(&ev, &summary, &body, "hello stdout\nline 2", "fatal stderr\nboom")
            .expect("should insert error record");

        let rows = db.recent_errors(10).expect("should query recent errors");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tgid, 7777);
        assert_eq!(rows[0].exit_code, 13);
        assert_eq!(rows[0].summary, summary);
        assert_eq!(rows[0].notification_body, body);
        assert_eq!(rows[0].stdout, "hello stdout\nline 2");
        assert_eq!(rows[0].stderr, "fatal stderr\nboom");

        // Inserting another error for the same process ('crashy') and same exit_code (13)
        // should overwrite with the most recent occurrence instead of adding a duplicate row
        let newer_ev = ProcessExitEvent {
            pid: 8888,
            tgid: 8888,
            cmdline: "crashy --again".to_string(),
            ..ev.clone()
        };
        let (newer_summary, newer_body) = format_exit_details(&newer_ev);
        db.insert_error(&newer_ev, &newer_summary, &newer_body, "newer stdout", "newer stderr")
            .expect("should upsert most recent error record");

        let rows_after_upsert = db.recent_errors(10).expect("should query upserted errors");
        assert_eq!(rows_after_upsert.len(), 1);
        assert_eq!(rows_after_upsert[0].tgid, 8888);
        assert_eq!(rows_after_upsert[0].cmdline, "crashy --again");
        assert_eq!(rows_after_upsert[0].stdout, "newer stdout");
        assert_eq!(rows_after_upsert[0].stderr, "newer stderr");

        // A different exit_code for the same process name is stored as its own entry
        let diff_code_ev = ProcessExitEvent {
            exit_code: 2,
            ..newer_ev
        };
        let (diff_summary, diff_body) = format_exit_details(&diff_code_ev);
        db.insert_error(&diff_code_ev, &diff_summary, &diff_body, "code 2 out", "code 2 err")
            .expect("should insert distinct exit_code");
        assert_eq!(db.recent_errors(10).unwrap().len(), 2);

        drop(db);

        // Re-opening via ErrorDatabase::open (as on program start) should clear and reinitialize
        let db_reinit = ErrorDatabase::open(&db_path).expect("should reinit sqlite db");
        let rows_after_reinit = db_reinit
            .recent_errors(10)
            .expect("should query empty reinitialized db");
        assert!(rows_after_reinit.is_empty());

        drop(db_reinit);
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
