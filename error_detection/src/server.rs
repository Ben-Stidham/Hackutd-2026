use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::daemon::{is_shutdown_requested, log_message};
use crate::db::{ErrorDatabase, ErrorLogRecord};

pub struct HttpServerHandle {
    pub local_addr: SocketAddr,
    join_handle: Option<JoinHandle<()>>,
}

impl HttpServerHandle {
    pub fn start(port: u16, db_path: PathBuf) -> io::Result<Self> {
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let listener = TcpListener::bind(bind_addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;

        let join_handle = thread::spawn(move || {
            run_http_server(listener, &db_path);
        });

        Ok(Self {
            local_addr,
            join_handle: Some(join_handle),
        })
    }
}

impl Drop for HttpServerHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_http_server(listener: TcpListener, db_path: &Path) {
    while !is_shutdown_requested() {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                if !peer_addr.ip().is_loopback() {
                    let _ = write_http_response(
                        stream,
                        403,
                        "Forbidden",
                        r#"{"error":"Only requests from localhost are allowed"}"#,
                    );
                    continue;
                }
                let _ = handle_client(stream, db_path);
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                log_message(&format!("HTTP accept error: {err}"));
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_client(stream: TcpStream, db_path: &Path) -> io::Result<()> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    // Drain remaining request headers until blank line
    loop {
        let mut header_line = String::new();
        let n = reader.read_line(&mut header_line)?;
        if n == 0 || header_line == "\r\n" || header_line == "\n" {
            break;
        }
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return write_http_response(
            stream,
            400,
            "Bad Request",
            r#"{"error":"Malformed HTTP request line"}"#,
        );
    }

    let method = parts[0];
    let raw_target = parts[1];

    if method == "OPTIONS" {
        return write_http_response(stream, 204, "No Content", "");
    }

    if method != "GET" {
        return write_http_response(
            stream,
            405,
            "Method Not Allowed",
            r#"{"error":"Only GET requests are supported"}"#,
        );
    }

    let (status_code, status_text, body) = route_get_request(raw_target, db_path);
    write_http_response(stream, status_code, status_text, &body)
}

#[derive(Debug, PartialEq, Eq)]
pub enum RouteMatch {
    ListAll,
    GetById(i64),
    InvalidId(String),
    NotFound,
}

pub fn parse_route(raw_target: &str) -> RouteMatch {
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (raw_target, None),
    };

    let trimmed_path = path.trim_end_matches('/');
    let normalized = if trimmed_path.is_empty() {
        "/"
    } else {
        trimmed_path
    };

    // Check if ?id=<id> was provided in the query string on a valid endpoint
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(val) = pair.strip_prefix("id=") {
                if matches!(normalized, "/" | "/errors" | "/api/errors" | "/processes") {
                    return match val.parse::<i64>() {
                        Ok(id) => RouteMatch::GetById(id),
                        Err(_) => RouteMatch::InvalidId(val.to_string()),
                    };
                }
            }
        }
    }

    match normalized {
        "/" | "/errors" | "/api/errors" | "/processes" => RouteMatch::ListAll,
        _ => {
            for prefix in ["/errors/", "/api/errors/", "/processes/"] {
                if let Some(rest) = normalized.strip_prefix(prefix) {
                    if !rest.contains('/') {
                        return match rest.parse::<i64>() {
                            Ok(id) => RouteMatch::GetById(id),
                            Err(_) => RouteMatch::InvalidId(rest.to_string()),
                        };
                    }
                }
            }
            RouteMatch::NotFound
        }
    }
}

pub fn route_get_request(raw_target: &str, db_path: &Path) -> (u16, &'static str, String) {
    let route = parse_route(raw_target);
    match route {
        RouteMatch::NotFound => (
            404,
            "Not Found",
            r#"{"error":"Endpoint not found. Use GET /errors or GET /errors/<id>"}"#.to_string(),
        ),
        RouteMatch::InvalidId(bad) => (
            400,
            "Bad Request",
            format!(r#"{{"error":"Invalid error ID: {}"}}"#, json_escape(&bad)),
        ),
        RouteMatch::ListAll => {
            let db = match ErrorDatabase::open_query(db_path) {
                Ok(db) => db,
                Err(err) => {
                    return (
                        500,
                        "Internal Server Error",
                        format!(r#"{{"error":"Database error: {}"}}"#, json_escape(&err.to_string())),
                    );
                }
            };
            match db.all_errors() {
                Ok(records) => (200, "OK", format_error_list_json(&records)),
                Err(err) => (
                    500,
                    "Internal Server Error",
                    format!(r#"{{"error":"Query failed: {}"}}"#, json_escape(&err.to_string())),
                ),
            }
        }
        RouteMatch::GetById(id) => {
            let db = match ErrorDatabase::open_query(db_path) {
                Ok(db) => db,
                Err(err) => {
                    return (
                        500,
                        "Internal Server Error",
                        format!(r#"{{"error":"Database error: {}"}}"#, json_escape(&err.to_string())),
                    );
                }
            };
            match db.get_error_by_id(id) {
                Ok(Some(rec)) => (200, "OK", format_error_detail_json(&rec)),
                Ok(None) => (
                    404,
                    "Not Found",
                    format!(r#"{{"error":"No failed process found with id {id}"}}"#),
                ),
                Err(err) => (
                    500,
                    "Internal Server Error",
                    format!(r#"{{"error":"Query failed: {}"}}"#, json_escape(&err.to_string())),
                ),
            }
        }
    }
}

pub fn format_error_list_json(records: &[ErrorLogRecord]) -> String {
    let items: Vec<String> = records
        .iter()
        .map(|r| {
            let comm = json_escape(&r.comm);
            format!(
                r#"{{"id":{},"process_name":"{}","comm":"{}","error_code":{},"exit_code":{}}}"#,
                r.id, comm, comm, r.exit_code, r.exit_code
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

pub fn format_error_detail_json(r: &ErrorLogRecord) -> String {
    let signal_json = match r.exit_signal {
        Some(sig) => sig.to_string(),
        None => "null".to_string(),
    };
    let comm = json_escape(&r.comm);
    format!(
        r#"{{"id":{},"timestamp":{},"boot_id":"{}","pid":{},"tgid":{},"parent_pid":{},"process_name":"{}","comm":"{}","cmdline":"{}","error_code":{},"exit_code":{},"exit_signal":{},"summary":"{}","notification_body":"{}","stdout":"{}","stderr":"{}"}}"#,
        r.id,
        r.timestamp,
        json_escape(&r.boot_id),
        r.pid,
        r.tgid,
        r.parent_pid,
        comm,
        comm,
        json_escape(&r.cmdline),
        r.exit_code,
        r.exit_code,
        signal_json,
        json_escape(&r.summary),
        json_escape(&r.notification_body),
        json_escape(&r.stdout),
        json_escape(&r.stderr),
    )
}

pub fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn write_http_response(
    mut stream: TcpStream,
    status_code: u16,
    status_text: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status_code} {status_text}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlink::ProcessExitEvent;
    use std::fs;

    #[test]
    fn test_parse_routes() {
        assert_eq!(parse_route("/errors"), RouteMatch::ListAll);
        assert_eq!(parse_route("/errors/"), RouteMatch::ListAll);
        assert_eq!(parse_route("/api/errors"), RouteMatch::ListAll);
        assert_eq!(parse_route("/processes"), RouteMatch::ListAll);
        assert_eq!(parse_route("/errors/42"), RouteMatch::GetById(42));
        assert_eq!(parse_route("/errors?id=42"), RouteMatch::GetById(42));
        assert_eq!(parse_route("/api/errors/7"), RouteMatch::GetById(7));
        assert_eq!(
            parse_route("/errors/notanumber"),
            RouteMatch::InvalidId("notanumber".to_string())
        );
        assert_eq!(parse_route("/unknown/path"), RouteMatch::NotFound);
    }

    #[test]
    fn test_route_get_list_and_detail() {
        let db_path = PathBuf::from(format!(
            "/tmp/test_http_error_detection_{}.db",
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

        let (code, _, list_json) = route_get_request("/errors", &db_path);
        assert_eq!(code, 200);
        assert!(list_json.contains(r#""id":1"#));
        assert!(list_json.contains(r#""process_name":"broken_worker""#));
        assert!(list_json.contains(r#""error_code":127"#));

        let (code, _, detail_json) = route_get_request("/errors/1", &db_path);
        assert_eq!(code, 200);
        assert!(detail_json.contains(r#""id":1"#));
        assert!(detail_json.contains(r#""cmdline":"broken_worker --retry""#));
        assert!(detail_json.contains(r#""stdout":"starting worker...\n""#));
        assert!(detail_json.contains(r#""stderr":"command not found\n""#));

        let (code404, _, _) = route_get_request("/errors/999", &db_path);
        assert_eq!(code404, 404);

        drop(db);
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
