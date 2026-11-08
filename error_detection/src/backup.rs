use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::daemon::log_message;

pub const DEFAULT_BACKUP_ROOT: &str = "/var/backups/error_detection";
pub const DEFAULT_SOURCE_ROOT: &str = "/";
pub const STARTUP_SLOT_DIR: &str = "startup";
pub const MANUAL_SLOT_DIR: &str = "manual";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupSlot {
    Startup,
    Manual,
}

impl BackupSlot {
    pub fn dir_name(self) -> &'static str {
        match self {
            BackupSlot::Startup => STARTUP_SLOT_DIR,
            BackupSlot::Manual => MANUAL_SLOT_DIR,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BackupSlot::Startup => "startup",
            BackupSlot::Manual => "manual",
        }
    }

    pub fn other(self) -> Self {
        match self {
            BackupSlot::Startup => BackupSlot::Manual,
            BackupSlot::Manual => BackupSlot::Startup,
        }
    }

    fn timestamp_filename(self) -> &'static str {
        match self {
            BackupSlot::Startup => ".startup_timestamp",
            BackupSlot::Manual => ".manual_timestamp",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupOperationResult {
    pub success: bool,
    pub action: String,
    pub slot: String,
    pub backup_path: String,
    pub source_path: String,
    pub timestamp: u64,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Default)]
struct BackupTimestamps {
    startup_ts: Option<u64>,
    manual_ts: Option<u64>,
}

pub struct BackupManager {
    backup_root: PathBuf,
    source_root: PathBuf,
    state: Mutex<BackupTimestamps>,
    op_lock: Mutex<()>,
}

impl BackupManager {
    pub fn new(backup_root: Option<PathBuf>, source_root: Option<PathBuf>) -> Self {
        let root = backup_root
            .or_else(|| env::var("ERROR_DETECTION_BACKUP_DIR").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_BACKUP_ROOT));

        let src = source_root
            .or_else(|| env::var("ERROR_DETECTION_BACKUP_SOURCE").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SOURCE_ROOT));

        let startup_ts = read_slot_timestamp(&root, BackupSlot::Startup);
        let manual_ts = read_slot_timestamp(&root, BackupSlot::Manual);

        Self {
            backup_root: root,
            source_root: src,
            state: Mutex::new(BackupTimestamps {
                startup_ts,
                manual_ts,
            }),
            op_lock: Mutex::new(()),
        }
    }

    /// Creates the startup backup (`/var/backups/error_detection/startup`),
    /// replacing any startup backup from the previous startup.
    pub fn create_startup_backup(&self) -> BackupOperationResult {
        self.create_rsync_backup(BackupSlot::Startup)
    }

    /// Creates the server-triggered backup (`/var/backups/error_detection/manual`),
    /// overwriting the backup from the previous time the server was called.
    pub fn create_manual_backup(&self) -> BackupOperationResult {
        self.create_rsync_backup(BackupSlot::Manual)
    }

    fn create_rsync_backup(&self, slot: BackupSlot) -> BackupOperationResult {
        let _guard = self.op_lock.lock().ok();
        let now = current_unix_ts();
        let slot_dir = self.slot_path(slot);
        let other_dir = self.slot_path(slot.other());

        if let Err(err) = fs::create_dir_all(&slot_dir) {
            let msg = format!(
                "Failed to create backup directory {}: {err}",
                slot_dir.display()
            );
            log_message(&msg);
            return BackupOperationResult {
                success: false,
                action: "create_backup".to_string(),
                slot: slot.label().to_string(),
                backup_path: slot_dir.display().to_string(),
                source_path: self.source_root.display().to_string(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: msg,
            };
        }

        // Enforce at most 2 backups by removing any unexpected directories in backup_root
        self.prune_extra_backup_dirs();

        let src_arg = ensure_trailing_slash(&self.source_root);
        let dst_arg = ensure_trailing_slash(&slot_dir);

        let mut cmd = Command::new("rsync");
        cmd.args(["-aAX", "--delete", "--numeric-ids"]);
        if self.source_root != Path::new("/") {
            cmd.arg("--checksum");
        }

        for pattern in Self::exclude_patterns(&self.source_root, &self.backup_root) {
            cmd.arg(format!("--exclude={pattern}"));
        }

        // If the other backup slot already exists, use --link-dest so unchanged files are hard-linked
        if other_dir.is_dir() && slot_exists(&self.backup_root, slot.other()) {
            cmd.arg(format!("--link-dest={}", other_dir.display()));
        }

        cmd.arg(&src_arg)
            .arg(&dst_arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        log_message(&format!(
            "Creating {} rsync backup from '{}' to '{}'...",
            slot.label(),
            src_arg,
            dst_arg
        ));

        match cmd.output() {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                // rsync exit code 0 = success, 24 = partial transfer due to vanished source files (normal on live /)
                let success = exit_code == 0 || exit_code == 24;

                if success {
                    write_slot_timestamp(&self.backup_root, slot, now);
                    if let Ok(mut st) = self.state.lock() {
                        match slot {
                            BackupSlot::Startup => st.startup_ts = Some(now),
                            BackupSlot::Manual => st.manual_ts = Some(now),
                        }
                    }
                    log_message(&format!(
                        "Successfully created {} rsync backup at '{}'",
                        slot.label(),
                        slot_dir.display()
                    ));
                } else {
                    log_message(&format!(
                        "Failed to create {} rsync backup (exit={exit_code}): {stderr}",
                        slot.label()
                    ));
                }

                BackupOperationResult {
                    success,
                    action: "create_backup".to_string(),
                    slot: slot.label().to_string(),
                    backup_path: slot_dir.display().to_string(),
                    source_path: self.source_root.display().to_string(),
                    timestamp: now,
                    exit_code,
                    stdout,
                    stderr,
                }
            }
            Err(err) => BackupOperationResult {
                success: false,
                action: "create_backup".to_string(),
                slot: slot.label().to_string(),
                backup_path: slot_dir.display().to_string(),
                source_path: self.source_root.display().to_string(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Failed to execute rsync: {err}"),
            },
        }
    }

    /// Restores a backup using `rsync` back to the source root (`/`).
    /// Defaults to restoring the most recent backup unless `requested_target` specifies `"startup"` or `"manual"`.
    pub fn restore_backup(&self, requested_target: Option<&str>) -> BackupOperationResult {
        let _guard = self.op_lock.lock().ok();
        let now = current_unix_ts();

        let Some(slot) = self.select_restore_slot(requested_target) else {
            return BackupOperationResult {
                success: false,
                action: "restore_backup".to_string(),
                slot: "none".to_string(),
                backup_path: String::new(),
                source_path: self.source_root.display().to_string(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: "No matching rsync backup found to restore".to_string(),
            };
        };

        let slot_dir = self.slot_path(slot);
        let src_arg = ensure_trailing_slash(&slot_dir);
        let dst_arg = ensure_trailing_slash(&self.source_root);

        let mut cmd = Command::new("rsync");
        cmd.args(["-aAX", "--delete", "--numeric-ids"]);
        if self.source_root != Path::new("/") {
            cmd.arg("--checksum");
        }

        for pattern in Self::exclude_patterns(&self.source_root, &self.backup_root) {
            cmd.arg(format!("--exclude={pattern}"));
        }

        cmd.arg(&src_arg)
            .arg(&dst_arg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        log_message(&format!(
            "Restoring {} rsync backup from '{}' to '{}'...",
            slot.label(),
            src_arg,
            dst_arg
        ));

        match cmd.output() {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let success = exit_code == 0 || exit_code == 24;

                if success {
                    log_message(&format!(
                        "Successfully restored {} backup from '{}'",
                        slot.label(),
                        slot_dir.display()
                    ));
                } else {
                    log_message(&format!(
                        "Failed to restore {} backup from '{}' (exit={exit_code}): {stderr}",
                        slot.label(),
                        slot_dir.display()
                    ));
                }

                BackupOperationResult {
                    success,
                    action: "restore_backup".to_string(),
                    slot: slot.label().to_string(),
                    backup_path: slot_dir.display().to_string(),
                    source_path: self.source_root.display().to_string(),
                    timestamp: now,
                    exit_code,
                    stdout,
                    stderr,
                }
            }
            Err(err) => BackupOperationResult {
                success: false,
                action: "restore_backup".to_string(),
                slot: slot.label().to_string(),
                backup_path: slot_dir.display().to_string(),
                source_path: self.source_root.display().to_string(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Failed to execute rsync: {err}"),
            },
        }
    }

    pub fn select_restore_slot(&self, requested_target: Option<&str>) -> Option<BackupSlot> {
        let startup_exists = slot_exists(&self.backup_root, BackupSlot::Startup);
        let manual_exists = slot_exists(&self.backup_root, BackupSlot::Manual);

        let guard = self.state.lock().ok()?;
        let startup_ts = guard
            .startup_ts
            .or_else(|| read_slot_timestamp(&self.backup_root, BackupSlot::Startup));
        let manual_ts = guard
            .manual_ts
            .or_else(|| read_slot_timestamp(&self.backup_root, BackupSlot::Manual));

        choose_slot_with_state(
            requested_target,
            startup_ts,
            manual_ts,
            startup_exists,
            manual_exists,
        )
    }

    fn slot_path(&self, slot: BackupSlot) -> PathBuf {
        self.backup_root.join(slot.dir_name())
    }

    fn exclude_patterns(source_root: &Path, backup_root: &Path) -> Vec<String> {
        if source_root != Path::new("/") {
            if let Ok(rel) = backup_root.strip_prefix(source_root) {
                return vec![format!("/{}/**", rel.display())];
            }
            return Vec::new();
        }

        let mut patterns = vec![
            "/dev/*".to_string(),
            "/proc/*".to_string(),
            "/sys/*".to_string(),
            "/tmp/*".to_string(),
            "/run/*".to_string(),
            "/mnt/*".to_string(),
            "/media/*".to_string(),
            "/lost+found".to_string(),
            "/home/*".to_string(),
            "/var/backups/error_detection/**".to_string(),
            "/var/cache/**".to_string(),
            "/var/tmp/**".to_string(),
            "/var/lib/libvirt/**".to_string(),
            "/var/lib/docker/**".to_string(),
            "/var/lib/containers/**".to_string(),
            "/swapfile".to_string(),
        ];
        let custom_root = format!("{}/**", backup_root.display());
        if !patterns.contains(&custom_root) {
            patterns.push(custom_root);
        }
        patterns
    }

    fn prune_extra_backup_dirs(&self) {
        let Ok(entries) = fs::read_dir(&self.backup_root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str != STARTUP_SLOT_DIR
                && name_str != MANUAL_SLOT_DIR
                && !name_str.starts_with('.')
            {
                let path = entry.path();
                if path.is_dir() {
                    let _ = fs::remove_dir_all(path);
                } else {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

pub fn choose_slot_with_state(
    requested_target: Option<&str>,
    startup_ts: Option<u64>,
    manual_ts: Option<u64>,
    startup_exists: bool,
    manual_exists: bool,
) -> Option<BackupSlot> {
    if let Some(raw) = requested_target.map(str::trim).filter(|s| !s.is_empty()) {
        match raw.to_ascii_lowercase().as_str() {
            "startup" | "boot" => {
                return if startup_exists {
                    Some(BackupSlot::Startup)
                } else {
                    None
                };
            }
            "manual" | "server" | "user" => {
                return if manual_exists {
                    Some(BackupSlot::Manual)
                } else {
                    None
                };
            }
            "latest" | "recent" | "most_recent" | "default" => {}
            _ => return None,
        }
    }

    let eff_manual = if manual_exists {
        Some(manual_ts.unwrap_or(1))
    } else {
        None
    };
    let eff_startup = if startup_exists {
        Some(startup_ts.unwrap_or(0))
    } else {
        None
    };

    match (eff_manual, eff_startup) {
        (Some(m_ts), Some(s_ts)) => {
            if m_ts >= s_ts {
                Some(BackupSlot::Manual)
            } else {
                Some(BackupSlot::Startup)
            }
        }
        (Some(_), None) => Some(BackupSlot::Manual),
        (None, Some(_)) => Some(BackupSlot::Startup),
        (None, None) => None,
    }
}

fn slot_exists(backup_root: &Path, slot: BackupSlot) -> bool {
    let dir = backup_root.join(slot.dir_name());
    dir.is_dir() && read_slot_timestamp(backup_root, slot).is_some()
}

fn write_slot_timestamp(backup_root: &Path, slot: BackupSlot, ts: u64) {
    let ts_path = backup_root.join(slot.timestamp_filename());
    let _ = fs::write(ts_path, format!("{ts}\n"));
}

fn read_slot_timestamp(backup_root: &Path, slot: BackupSlot) -> Option<u64> {
    let ts_path = backup_root.join(slot.timestamp_filename());
    fs::read_to_string(ts_path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn ensure_trailing_slash(path: &Path) -> String {
    let s = path.display().to_string();
    if s.ends_with('/') {
        s
    } else {
        format!("{s}/")
    }
}

fn current_unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_slot_with_state_defaults_to_most_recent() {
        assert_eq!(
            choose_slot_with_state(None, Some(100), None, true, false),
            Some(BackupSlot::Startup)
        );
        assert_eq!(
            choose_slot_with_state(None, Some(100), Some(200), true, true),
            Some(BackupSlot::Manual)
        );
        assert_eq!(
            choose_slot_with_state(Some("startup"), Some(100), Some(200), true, true),
            Some(BackupSlot::Startup)
        );
        assert_eq!(
            choose_slot_with_state(Some("manual"), Some(300), Some(200), true, true),
            Some(BackupSlot::Manual)
        );
    }

    #[test]
    fn test_rsync_backup_and_restore_end_to_end() {
        let base = PathBuf::from(format!("/tmp/test_rsync_backup_{}", std::process::id()));
        let src_dir = base.join("src");
        let backup_root = base.join("backups");
        let _ = fs::remove_dir_all(&base);

        fs::create_dir_all(&src_dir).expect("create src");
        fs::write(src_dir.join("config.txt"), "v1-startup").expect("write v1");

        let bm = BackupManager::new(Some(backup_root.clone()), Some(src_dir.clone()));

        // 1. Create startup backup (v1-startup)
        let res_startup = bm.create_startup_backup();
        assert!(res_startup.success);
        assert_eq!(res_startup.slot, "startup");

        // 2. Modify source and create manual backup (v2-manual)
        fs::write(src_dir.join("config.txt"), "v2-manual").expect("write v2");
        let res_manual = bm.create_manual_backup();
        assert!(res_manual.success);
        assert_eq!(res_manual.slot, "manual");

        // 3. Corrupt source (v3-broken) and restore default (most recent -> manual -> v2-manual)
        fs::write(src_dir.join("config.txt"), "v3-broken").expect("write v3");
        let res_restore_latest = bm.restore_backup(None);
        assert!(res_restore_latest.success);
        assert_eq!(res_restore_latest.slot, "manual");
        assert_eq!(
            fs::read_to_string(src_dir.join("config.txt")).unwrap(),
            "v2-manual"
        );

        // 4. Explicitly restore startup backup (v1-startup)
        let res_restore_startup = bm.restore_backup(Some("startup"));
        assert!(res_restore_startup.success);
        assert_eq!(res_restore_startup.slot, "startup");
        assert_eq!(
            fs::read_to_string(src_dir.join("config.txt")).unwrap(),
            "v1-startup"
        );

        let _ = fs::remove_dir_all(&base);
    }
}
