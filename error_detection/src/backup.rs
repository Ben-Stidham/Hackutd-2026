use std::env;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::daemon::log_message;

pub const STARTUP_BACKUP_NAME: &str = "error_detection_startup_backup";
pub const MANUAL_BACKUP_NAME: &str = "error_detection_manual_backup";
pub const DEFAULT_SNAPSHOT_SIZE: &str = "5G";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupSlot {
    Startup,
    Manual,
}

impl BackupSlot {
    pub fn snapshot_name(self) -> &'static str {
        match self {
            BackupSlot::Startup => STARTUP_BACKUP_NAME,
            BackupSlot::Manual => MANUAL_BACKUP_NAME,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BackupSlot::Startup => "startup",
            BackupSlot::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupOperationResult {
    pub success: bool,
    pub action: String,
    pub slot: String,
    pub snapshot_name: String,
    pub origin_lv: String,
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
    configured_lv: Option<String>,
    snapshot_size: String,
    state: Mutex<BackupTimestamps>,
}

impl BackupManager {
    pub fn new(configured_lv: Option<String>, snapshot_size: Option<String>) -> Self {
        let lv = configured_lv.or_else(|| env::var("ERROR_DETECTION_LV_PATH").ok());
        let size = snapshot_size
            .or_else(|| env::var("ERROR_DETECTION_SNAPSHOT_SIZE").ok())
            .unwrap_or_else(|| DEFAULT_SNAPSHOT_SIZE.to_string());

        Self {
            configured_lv: lv,
            snapshot_size: size,
            state: Mutex::new(BackupTimestamps::default()),
        }
    }

    /// Creates the startup backup (`error_detection_startup_backup`), replacing any previous startup backup.
    pub fn create_startup_backup(&self) -> BackupOperationResult {
        self.create_snapshot(BackupSlot::Startup)
    }

    /// Creates the server-triggered backup (`error_detection_manual_backup`), overwriting any previous server-triggered backup.
    pub fn create_manual_backup(&self) -> BackupOperationResult {
        self.create_snapshot(BackupSlot::Manual)
    }

    fn create_snapshot(&self, slot: BackupSlot) -> BackupOperationResult {
        let now = current_unix_ts();
        let snap_name = slot.snapshot_name();

        let Some((vg_name, lv_name)) = self.resolve_origin_lv() else {
            let msg = "Could not detect an active LVM logical volume. Set ERROR_DETECTION_LV_PATH=<vg>/<lv> or --lv-path.";
            log_message(&format!("Backup ({}) skipped/failed: {msg}", slot.label()));
            return BackupOperationResult {
                success: false,
                action: "create_backup".to_string(),
                slot: slot.label().to_string(),
                snapshot_name: snap_name.to_string(),
                origin_lv: String::new(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: msg.to_string(),
            };
        };

        let origin_path = format!("{vg_name}/{lv_name}");
        let snap_path = format!("{vg_name}/{snap_name}");

        // Remove previous snapshot in this slot if it exists so we overwrite it cleanly
        if snapshot_exists(&vg_name, snap_name) {
            log_message(&format!("Removing previous {} snapshot {snap_path}...", slot.label()));
            let _ = Command::new("lvremove")
                .args(["-f", &snap_path])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        // Also enforce at most 2 backups by removing any stale snapshots beyond the 2 managed slots
        self.prune_extra_snapshots(&vg_name);

        log_message(&format!(
            "Creating {} LVM snapshot '{snap_name}' (size {}) from '{origin_path}'...",
            slot.label(),
            self.snapshot_size
        ));

        match Command::new("lvcreate")
            .args([
                "--snapshot",
                "--name",
                snap_name,
                "--size",
                &self.snapshot_size,
                &origin_path,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let success = output.status.success();

                if success {
                    if let Ok(mut guard) = self.state.lock() {
                        match slot {
                            BackupSlot::Startup => guard.startup_ts = Some(now),
                            BackupSlot::Manual => guard.manual_ts = Some(now),
                        }
                    }
                    log_message(&format!(
                        "Successfully created {} backup '{snap_path}'",
                        slot.label()
                    ));
                } else {
                    log_message(&format!(
                        "Failed to create {} backup '{snap_path}' (exit={exit_code}): {stderr}",
                        slot.label()
                    ));
                }

                BackupOperationResult {
                    success,
                    action: "create_backup".to_string(),
                    slot: slot.label().to_string(),
                    snapshot_name: snap_name.to_string(),
                    origin_lv: origin_path,
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
                snapshot_name: snap_name.to_string(),
                origin_lv: origin_path,
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Failed to execute lvcreate: {err}"),
            },
        }
    }

    /// Restores a backup using `lvconvert --merge`.
    /// Defaults to restoring the most recent backup unless `requested_target` specifies `"startup"` or `"manual"`.
    pub fn restore_backup(&self, requested_target: Option<&str>) -> BackupOperationResult {
        let now = current_unix_ts();

        let Some((vg_name, lv_name)) = self.resolve_origin_lv() else {
            let msg = "Could not detect an active LVM logical volume to restore.";
            return BackupOperationResult {
                success: false,
                action: "restore_backup".to_string(),
                slot: "none".to_string(),
                snapshot_name: String::new(),
                origin_lv: String::new(),
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: msg.to_string(),
            };
        };

        let origin_path = format!("{vg_name}/{lv_name}");
        let slot = match self.select_restore_slot(&vg_name, requested_target) {
            Some(s) => s,
            None => {
                return BackupOperationResult {
                    success: false,
                    action: "restore_backup".to_string(),
                    slot: "none".to_string(),
                    snapshot_name: String::new(),
                    origin_lv: origin_path,
                    timestamp: now,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "No matching LVM backup snapshot found to restore".to_string(),
                };
            }
        };

        let snap_name = slot.snapshot_name();
        let snap_path = format!("{vg_name}/{snap_name}");

        log_message(&format!(
            "Restoring {} LVM snapshot '{snap_path}' into '{origin_path}'...",
            slot.label()
        ));

        match Command::new("lvconvert")
            .args(["--merge", &snap_path])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let success = output.status.success();

                if success {
                    // Merging consumes the snapshot LV
                    if let Ok(mut guard) = self.state.lock() {
                        match slot {
                            BackupSlot::Startup => guard.startup_ts = None,
                            BackupSlot::Manual => guard.manual_ts = None,
                        }
                    }
                    log_message(&format!(
                        "Successfully scheduled/completed merge of '{snap_path}'"
                    ));
                } else {
                    log_message(&format!(
                        "Failed to restore '{snap_path}' (exit={exit_code}): {stderr}"
                    ));
                }

                BackupOperationResult {
                    success,
                    action: "restore_backup".to_string(),
                    slot: slot.label().to_string(),
                    snapshot_name: snap_name.to_string(),
                    origin_lv: origin_path,
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
                snapshot_name: snap_name.to_string(),
                origin_lv: origin_path,
                timestamp: now,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Failed to execute lvconvert: {err}"),
            },
        }
    }

    pub fn select_restore_slot(
        &self,
        vg_name: &str,
        requested_target: Option<&str>,
    ) -> Option<BackupSlot> {
        let startup_exists = snapshot_exists(vg_name, STARTUP_BACKUP_NAME);
        let manual_exists = snapshot_exists(vg_name, MANUAL_BACKUP_NAME);

        let guard = self.state.lock().ok()?;
        choose_slot_with_state(
            requested_target,
            guard.startup_ts,
            guard.manual_ts,
            startup_exists,
            manual_exists,
        )
    }

    fn resolve_origin_lv(&self) -> Option<(String, String)> {
        if let Some(ref cfg) = self.configured_lv {
            if let Some(pair) = parse_vg_lv(cfg) {
                return Some(pair);
            }
        }

        // Try discovering via `lvs --noheadings -o vg_name,lv_name,lv_attr`
        // selecting the first non-snapshot origin/standard volume
        let output = Command::new("lvs")
            .args(["--noheadings", "--separator", ":", "-o", "vg_name,lv_name,lv_attr"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.trim().split(':').collect();
            if parts.len() >= 3 {
                let vg = parts[0].trim();
                let lv = parts[1].trim();
                let attr = parts[2].trim();
                // Skip snapshot volumes (attr starts with 's' or 'S') and our own backup names
                if !attr.starts_with('s')
                    && !attr.starts_with('S')
                    && lv != STARTUP_BACKUP_NAME
                    && lv != MANUAL_BACKUP_NAME
                {
                    return Some((vg.to_string(), lv.to_string()));
                }
            }
        }
        None
    }

    fn prune_extra_snapshots(&self, vg_name: &str) {
        let Ok(output) = Command::new("lvs")
            .args(["--noheadings", "--separator", ":", "-o", "vg_name,lv_name,lv_attr"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.trim().split(':').collect();
            if parts.len() >= 3 {
                let vg = parts[0].trim();
                let lv = parts[1].trim();
                if vg == vg_name
                    && lv.starts_with("error_detection_")
                    && lv != STARTUP_BACKUP_NAME
                    && lv != MANUAL_BACKUP_NAME
                {
                    let _ = Command::new("lvremove")
                        .args(["-f", &format!("{vg}/{lv}")])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
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
            "startup" | "boot" | STARTUP_BACKUP_NAME => {
                return if startup_exists || startup_ts.is_some() {
                    Some(BackupSlot::Startup)
                } else {
                    None
                };
            }
            "manual" | "server" | "user" | MANUAL_BACKUP_NAME => {
                return if manual_exists || manual_ts.is_some() {
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
        manual_ts
    };
    let eff_startup = if startup_exists {
        Some(startup_ts.unwrap_or(0))
    } else {
        startup_ts
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

pub fn parse_vg_lv(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    if let Some(mapper) = trimmed.strip_prefix("/dev/mapper/") {
        let (vg, lv) = mapper.split_once('-')?;
        if !vg.is_empty() && !lv.is_empty() {
            return Some((vg.to_string(), lv.to_string()));
        }
    }
    let clean = trimmed.strip_prefix("/dev/").unwrap_or(trimmed);
    let (vg, lv) = clean.split_once('/')?;
    if !vg.is_empty() && !lv.is_empty() && !lv.contains('/') {
        Some((vg.to_string(), lv.to_string()))
    } else {
        None
    }
}

fn snapshot_exists(vg_name: &str, snap_name: &str) -> bool {
    let Ok(status) = Command::new("lvs")
        .arg(format!("{vg_name}/{snap_name}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    else {
        return false;
    };
    status.success()
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
    fn test_parse_vg_lv() {
        assert_eq!(
            parse_vg_lv("vg0/root"),
            Some(("vg0".to_string(), "root".to_string()))
        );
        assert_eq!(
            parse_vg_lv("/dev/vg0/root"),
            Some(("vg0".to_string(), "root".to_string()))
        );
        assert_eq!(
            parse_vg_lv("/dev/mapper/vg0-root"),
            Some(("vg0".to_string(), "root".to_string()))
        );
    }

    #[test]
    fn test_choose_slot_with_state_defaults_to_most_recent() {
        // Only startup exists
        assert_eq!(
            choose_slot_with_state(None, Some(100), None, true, false),
            Some(BackupSlot::Startup)
        );
        // Both exist, manual is newer (created after startup)
        assert_eq!(
            choose_slot_with_state(None, Some(100), Some(200), true, true),
            Some(BackupSlot::Manual)
        );
        // Both exist, but caller explicitly requests "startup"
        assert_eq!(
            choose_slot_with_state(Some("startup"), Some(100), Some(200), true, true),
            Some(BackupSlot::Startup)
        );
        // Both exist, caller explicitly requests "manual"
        assert_eq!(
            choose_slot_with_state(Some("manual"), Some(300), Some(200), true, true),
            Some(BackupSlot::Manual)
        );
    }
}

