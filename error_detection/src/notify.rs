use std::env;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

unsafe extern "C" {
    fn geteuid() -> u32;
    fn setgid(gid: u32) -> i32;
    fn setuid(uid: u32) -> i32;
}

#[derive(Clone, Debug)]
pub struct DesktopSession {
    pub target_uid: Option<u32>,
    pub target_gid: Option<u32>,
    pub target_user: Option<String>,
    pub dbus_address: Option<String>,
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub xdg_runtime_dir: Option<String>,
}

impl DesktopSession {
    /// Captures the current user's desktop notification environment (handling `sudo` or root invocation).
    pub fn capture() -> Self {
        let euid = unsafe { geteuid() };
        let mut target_uid = env::var("SUDO_UID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        let mut target_gid = env::var("SUDO_GID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        let mut target_user = env::var("SUDO_USER").ok();

        if euid == 0 && target_uid.is_none() {
            if let Some((uid, gid, username)) = discover_active_session_user() {
                target_uid = Some(uid);
                target_gid = Some(gid);
                target_user = Some(username);
            }
        }

        let xdg_runtime_dir = env::var("XDG_RUNTIME_DIR").ok().or_else(|| {
            target_uid.map(|uid| format!("/run/user/{uid}"))
        });

        let dbus_address = env::var("DBUS_SESSION_BUS_ADDRESS").ok().or_else(|| {
            xdg_runtime_dir.as_ref().and_then(|dir| {
                let bus_path = format!("{dir}/bus");
                if fs::metadata(&bus_path).is_ok() {
                    Some(format!("unix:path={bus_path}"))
                } else {
                    None
                }
            })
        });

        let display = env::var("DISPLAY").ok().or_else(|| Some(":0".to_string()));
        let wayland_display = env::var("WAYLAND_DISPLAY").ok().or_else(|| {
            xdg_runtime_dir.as_ref().and_then(|dir| {
                if fs::metadata(format!("{dir}/wayland-0")).is_ok() {
                    Some("wayland-0".to_string())
                } else {
                    None
                }
            })
        });

        Self {
            target_uid,
            target_gid,
            target_user,
            dbus_address,
            display,
            wayland_display,
            xdg_runtime_dir,
        }
    }

    /// Sends a desktop notification using the device's notification service (`notify-send` / `gdbus`).
    pub fn send_notification(&self, summary: &str, body: &str) -> io::Result<()> {
        let effective = if self.dbus_address.is_none() {
            Self::capture()
        } else {
            self.clone()
        };
        if effective.try_notify_send(summary, body).is_ok() {
            return Ok(());
        }
        effective.try_gdbus_notify(summary, body)
    }

    fn apply_session_env(&self, cmd: &mut Command) {
        if let Some(ref dbus) = self.dbus_address {
            cmd.env("DBUS_SESSION_BUS_ADDRESS", dbus);
        }
        if let Some(ref disp) = self.display {
            cmd.env("DISPLAY", disp);
        }
        if let Some(ref wdisp) = self.wayland_display {
            cmd.env("WAYLAND_DISPLAY", wdisp);
        }
        if let Some(ref xdg) = self.xdg_runtime_dir {
            cmd.env("XDG_RUNTIME_DIR", xdg);
        }
        if let Some(ref user) = self.target_user {
            cmd.env("USER", user);
            cmd.env("LOGNAME", user);
        }

        let euid = unsafe { geteuid() };
        if euid == 0 {
            let uid = self.target_uid;
            let gid = self.target_gid.or(uid);
            if let Some(u) = uid {
                if u != 0 {
                    unsafe {
                        cmd.pre_exec(move || {
                            if let Some(g) = gid {
                                let _ = setgid(g);
                            }
                            if setuid(u) != 0 {
                                return Err(io::Error::last_os_error());
                            }
                            Ok(())
                        });
                    }
                }
            }
        }
    }

    fn try_notify_send(&self, summary: &str, body: &str) -> io::Result<()> {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--app-name=error_detection")
            .arg("--urgency=critical")
            .arg("--icon=dialog-error")
            .arg(summary)
            .arg(body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        self.apply_session_env(&mut cmd);

        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "notify-send exited with status {status}"
            )))
        }
    }

    fn try_gdbus_notify(&self, summary: &str, body: &str) -> io::Result<()> {
        let mut cmd = Command::new("gdbus");
        cmd.args([
            "call",
            "--session",
            "--dest=org.freedesktop.Notifications",
            "--object-path=/org/freedesktop/Notifications",
            "--method=org.freedesktop.Notifications.Notify",
            "error_detection",
            "0",
            "dialog-error",
            summary,
            body,
            "[]",
            "{\"urgency\": <byte 2>}",
            "5000",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        self.apply_session_env(&mut cmd);

        let status = cmd.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "gdbus exited with status {status}"
            )))
        }
    }
}

fn discover_active_session_user() -> Option<(u32, u32, String)> {
    let entries = fs::read_dir("/run/user").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let uid_str = name.to_str()?;
        let Ok(uid) = uid_str.parse::<u32>() else {
            continue;
        };
        if uid >= 1000 && entry.path().join("bus").exists() {
            if let Some((gid, username)) = lookup_user_by_uid(uid) {
                return Some((uid, gid, username));
            }
            return Some((uid, uid, uid.to_string()));
        }
    }
    None
}

fn lookup_user_by_uid(target_uid: u32) -> Option<(u32, String)> {
    let content = fs::read_to_string("/etc/passwd").ok()?;
    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 4 {
            if let Ok(uid) = parts[2].parse::<u32>() {
                if uid == target_uid {
                    let gid = parts[3].parse::<u32>().unwrap_or(target_uid);
                    return Some((gid, parts[0].to_string()));
                }
            }
        }
    }
    None
}

