use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use axum::Router;
use serde::Serialize;
use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef, TryData};
use tower_http::cors::CorsLayer;

use crate::backup::BackupManager;
use crate::daemon::{is_shutdown_requested, log_message};
use crate::db::{ErrorDatabase, ErrorLogRecord};

#[derive(Debug, Clone, Serialize)]
pub struct ErrorSummaryItem {
    pub id: i64,
    pub process_name: String,
    pub comm: String,
    pub error_code: i32,
    pub exit_code: i32,
}

impl From<&ErrorLogRecord> for ErrorSummaryItem {
    fn from(r: &ErrorLogRecord) -> Self {
        Self {
            id: r.id,
            process_name: r.comm.clone(),
            comm: r.comm.clone(),
            error_code: r.exit_code,
            exit_code: r.exit_code,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetailItem {
    pub id: i64,
    pub timestamp: i64,
    pub boot_id: String,
    pub pid: i32,
    pub tgid: i32,
    pub parent_pid: i32,
    pub process_name: String,
    pub comm: String,
    pub cmdline: String,
    pub error_code: i32,
    pub exit_code: i32,
    pub exit_signal: Option<i32>,
    pub summary: String,
    pub notification_body: String,
    pub stdout: String,
    pub stderr: String,
}

impl From<ErrorLogRecord> for ErrorDetailItem {
    fn from(r: ErrorLogRecord) -> Self {
        Self {
            id: r.id,
            timestamp: r.timestamp,
            boot_id: r.boot_id,
            pid: r.pid,
            tgid: r.tgid,
            parent_pid: r.parent_pid,
            process_name: r.comm.clone(),
            comm: r.comm,
            cmdline: r.cmdline,
            error_code: r.exit_code,
            exit_code: r.exit_code,
            exit_signal: r.exit_signal,
            summary: r.summary,
            notification_body: r.notification_body,
            stdout: r.stdout,
            stderr: r.stderr,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandExecutionResult {
    pub command: String,
    pub exit_code: i32,
    pub error_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct SocketIoServerHandle {
    pub local_addr: SocketAddr,
    join_handle: Option<JoinHandle<()>>,
}

impl SocketIoServerHandle {
    pub fn start(
        port: u16,
        db_path: PathBuf,
        backup_manager: Arc<BackupManager>,
    ) -> io::Result<Self> {
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let std_listener = std::net::TcpListener::bind(bind_addr)?;
        std_listener.set_nonblocking(true)?;
        let local_addr = std_listener.local_addr()?;

        let join_handle = thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    log_message(&format!("Failed to build Tokio runtime for Socket.IO: {err}"));
                    return;
                }
            };

            rt.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(std_listener) else {
                    log_message("Failed to convert std TcpListener to tokio TcpListener");
                    return;
                };

                let app = build_socket_io_router(Arc::new(db_path), backup_manager);

                let server = axum::serve(listener, app).with_graceful_shutdown(async {
                    while !is_shutdown_requested() {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                });

                if let Err(err) = server.await {
                    log_message(&format!("Socket.IO server error: {err}"));
                }
            });
        });

        Ok(Self {
            local_addr,
            join_handle: Some(join_handle),
        })
    }
}

impl Drop for SocketIoServerHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn build_socket_io_router(
    db_path: Arc<PathBuf>,
    backup_manager: Arc<BackupManager>,
) -> Router {
    let (layer, io) = SocketIo::new_layer();

    io.ns("/", move |socket: SocketRef| {
        // 1. List all failed processes (only when prompted by the client)
        for event in ["list_errors", "get_errors", "list"] {
            let db_path = Arc::clone(&db_path);
            socket.on(event, move |s: SocketRef, ack: AckSender| {
                let db_path = Arc::clone(&db_path);
                async move {
                    let payload = query_error_list(&db_path);
                    let _ = ack.send(&payload);
                    let _ = s.emit("errors_list", &payload);
                }
            });
        }

        // 2. Get stored information for a specific failed process ID (or list of IDs)
        for event in ["get_error", "get"] {
            let db_path = Arc::clone(&db_path);
            socket.on(
                event,
                move |s: SocketRef, Data::<Value>(data), ack: AckSender| {
                    let db_path = Arc::clone(&db_path);
                    async move {
                        let payload = query_error_details(&db_path, &data);
                        let _ = ack.send(&payload);
                        let _ = s.emit("error_details", &payload);
                    }
                },
            );
        }

        // 3. Execute a command string on the computer and return exit_code, stdout, and stderr
        for event in ["run_command", "run"] {
            socket.on(
                event,
                |s: SocketRef, Data::<Value>(data), ack: AckSender| async move {
                    let payload = handle_run_command_payload(&data).await;
                    let _ = ack.send(&payload);
                    let _ = s.emit("command_result", &payload);
                },
            );
        }

        // 4. Create an rsync backup (overwriting the previous server-triggered backup)
        for event in ["create_backup", "backup"] {
            let bm = Arc::clone(&backup_manager);
            socket.on(event, move |s: SocketRef, ack: AckSender| {
                let bm = Arc::clone(&bm);
                async move {
                    let payload = handle_create_backup(bm).await;
                    let _ = ack.send(&payload);
                    let _ = s.emit("backup_result", &payload);
                }
            });
        }

        // 5. Restore an rsync backup (defaults to most recent unless specified)
        for event in ["restore_backup", "restore"] {
            let bm = Arc::clone(&backup_manager);
            socket.on(
                event,
                move |s: SocketRef, TryData::<Value>(maybe_data), ack: AckSender| {
                    let bm = Arc::clone(&bm);
                    async move {
                        let target = maybe_data.ok().and_then(|v| extract_restore_target(&v));
                        let payload = handle_restore_backup(bm, target).await;
                        let _ = ack.send(&payload);
                        let _ = s.emit("restore_result", &payload);
                    }
                },
            );
        }
    });

    Router::new().layer(layer).layer(CorsLayer::permissive())
}

async fn handle_create_backup(bm: Arc<BackupManager>) -> Value {
    match tokio::task::spawn_blocking(move || bm.create_manual_backup()).await {
        Ok(res) => serde_json::to_value(res).unwrap_or(Value::Null),
        Err(err) => json!({
            "success": false,
            "action": "create_backup",
            "stderr": format!("Task join error: {err}")
        }),
    }
}

async fn handle_restore_backup(bm: Arc<BackupManager>, target: Option<String>) -> Value {
    match tokio::task::spawn_blocking(move || bm.restore_backup(target.as_deref())).await {
        Ok(res) => serde_json::to_value(res).unwrap_or(Value::Null),
        Err(err) => json!({
            "success": false,
            "action": "restore_backup",
            "stderr": format!("Task join error: {err}")
        }),
    }
}

fn extract_restore_target(val: &Value) -> Option<String> {
    match val {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Object(map) => {
            for key in ["target", "slot", "backup", "type", "name"] {
                if let Some(Value::String(s)) = map.get(key)
                    && !s.trim().is_empty() {
                        return Some(s.trim().to_string());
                    }
            }
            None
        }
        _ => None,
    }
}

fn query_error_list(db_path: &Path) -> Value {
    let db = match ErrorDatabase::open_query(db_path) {
        Ok(db) => db,
        Err(err) => return json!({ "error": format!("Database error: {err}") }),
    };

    match db.all_errors() {
        Ok(records) => {
            let items: Vec<ErrorSummaryItem> = records.iter().map(ErrorSummaryItem::from).collect();
            serde_json::to_value(items).unwrap_or_else(|_| json!([]))
        }
        Err(err) => json!({ "error": format!("Query failed: {err}") }),
    }
}

fn query_error_details(db_path: &Path, data: &Value) -> Value {
    let db = match ErrorDatabase::open_query(db_path) {
        Ok(db) => db,
        Err(err) => return json!({ "error": format!("Database error: {err}") }),
    };

    if let Some(ids) = extract_ids_from_value(data) {
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            match db.get_error_by_id(id) {
                Ok(Some(rec)) => {
                    results.push(
                        serde_json::to_value(ErrorDetailItem::from(rec)).unwrap_or(Value::Null),
                    );
                }
                Ok(None) => {
                    results.push(json!({
                        "error": format!("No failed process found with id {id}"),
                        "id": id
                    }));
                }
                Err(err) => {
                    return json!({ "error": format!("Query failed: {err}") });
                }
            }
        }
        return Value::Array(results);
    }

    let Some(id) = extract_single_id_from_value(data) else {
        return json!({ "error": "Invalid or missing process error ID" });
    };

    match db.get_error_by_id(id) {
        Ok(Some(rec)) => serde_json::to_value(ErrorDetailItem::from(rec)).unwrap_or(Value::Null),
        Ok(None) => json!({
            "error": format!("No failed process found with id {id}"),
            "id": id
        }),
        Err(err) => json!({ "error": format!("Query failed: {err}") }),
    }
}

async fn handle_run_command_payload(data: &Value) -> Value {
    let Some(cmd_str) = extract_command_from_value(data) else {
        return json!({
            "command": "",
            "exit_code": -1,
            "error_code": -1,
            "stdout": "",
            "stderr": "Missing or empty command string"
        });
    };

    let res = tokio::task::spawn_blocking(move || execute_command(&cmd_str)).await;
    match res {
        Ok(exec_res) => serde_json::to_value(exec_res).unwrap_or(Value::Null),
        Err(err) => json!({
            "command": "",
            "exit_code": -1,
            "error_code": -1,
            "stdout": "",
            "stderr": format!("Task join error: {err}")
        }),
    }
}

fn execute_command(cmd_str: &str) -> CommandExecutionResult {
    log_message(&format!("Executing Socket.IO command: {cmd_str}"));
    match Command::new("sh")
        .arg("-c")
        .arg(cmd_str)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            CommandExecutionResult {
                command: cmd_str.to_string(),
                exit_code,
                error_code: exit_code,
                stdout,
                stderr,
            }
        }
        Err(err) => CommandExecutionResult {
            command: cmd_str.to_string(),
            exit_code: -1,
            error_code: -1,
            stdout: String::new(),
            stderr: format!("Failed to execute command: {err}"),
        },
    }
}

fn extract_single_id_from_value(val: &Value) -> Option<i64> {
    match val {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Object(map) => map.get("id").and_then(extract_single_id_from_value),
        _ => None,
    }
}

fn extract_ids_from_value(val: &Value) -> Option<Vec<i64>> {
    match val {
        Value::Array(arr) => {
            let mut ids = Vec::with_capacity(arr.len());
            for item in arr {
                ids.push(extract_single_id_from_value(item)?);
            }
            Some(ids)
        }
        Value::Object(map) => map.get("ids").and_then(extract_ids_from_value),
        _ => None,
    }
}

fn extract_command_from_value(val: &Value) -> Option<String> {
    match val {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Object(map) => {
            for key in ["command", "cmd", "run", "exec"] {
                if let Some(Value::String(s)) = map.get(key)
                    && !s.trim().is_empty() {
                        return Some(s.clone());
                    }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlink::ProcessExitEvent;
    use std::fs;

    #[test]
    fn test_socketio_handlers_list_and_detail() {
        let db_path = PathBuf::from(format!(
            "/tmp/test_socketio_error_detection_{}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&db_path);

        let db = ErrorDatabase::open(&db_path).expect("open db");
        let ev = ProcessExitEvent {
            pid: 9001,
            tgid: 9001,
            parent_pid: 1,
            exit_code: 127,
            exit_signal: None,
            raw_status: 127 << 8,
            comm: "broken_worker".to_string(),
            cmdline: "broken_worker --retry".to_string(),
            stdout_path: None,
            stderr_path: None,
            systemd_unit: None,
        };
        db.insert_error(
            &ev,
            "Process 'broken_worker' (PID 9001) exited with code 127",
            "Command: broken_worker --retry\nPID: 9001\nExit Code: 127",
            "starting worker...\n",
            "command not found\n",
        )
        .expect("insert error");

        let list_val = query_error_list(&db_path);
        let arr = list_val.as_array().expect("should return JSON array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["process_name"], "broken_worker");
        assert_eq!(arr[0]["error_code"], 127);

        let detail_val = query_error_details(&db_path, &json!(1));
        assert_eq!(detail_val["id"], 1);
        assert_eq!(detail_val["cmdline"], "broken_worker --retry");
        assert_eq!(detail_val["stdout"], "starting worker...\n");
        assert_eq!(detail_val["stderr"], "command not found\n");

        let detail_obj_val = query_error_details(&db_path, &json!({ "id": 1 }));
        assert_eq!(detail_obj_val["id"], 1);

        let batch_val = query_error_details(&db_path, &json!([1, 999]));
        let batch_arr = batch_val.as_array().expect("batch array");
        assert_eq!(batch_arr.len(), 2);
        assert_eq!(batch_arr[0]["id"], 1);
        assert!(batch_arr[1]["error"].is_string());

        drop(db);
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[test]
    fn test_execute_command() {
        let res = execute_command("printf 'out_msg'; printf 'err_msg' >&2; exit 3");
        assert_eq!(res.exit_code, 3);
        assert_eq!(res.error_code, 3);
        assert_eq!(res.stdout, "out_msg");
        assert_eq!(res.stderr, "err_msg");
    }

    #[test]
    fn test_extract_restore_target() {
        assert_eq!(
            extract_restore_target(&json!("startup")),
            Some("startup".to_string())
        );
        assert_eq!(
            extract_restore_target(&json!({"target": "manual"})),
            Some("manual".to_string())
        );
        assert_eq!(
            extract_restore_target(&json!({"slot": "startup"})),
            Some("startup".to_string())
        );
        assert_eq!(extract_restore_target(&json!({})), None);
    }
}
