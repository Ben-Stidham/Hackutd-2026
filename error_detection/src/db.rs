use std::ffi::{CStr, CString, c_char, c_void};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::netlink::ProcessExitEvent;

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _private: [u8; 0],
}

const SQLITE_OK: i32 = 0;
const SQLITE_ROW: i32 = 100;
const SQLITE_DONE: i32 = 101;
const SQLITE_TRANSIENT: isize = -1;

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut Sqlite3) -> i32;
    fn sqlite3_close(db: *mut Sqlite3) -> i32;
    fn sqlite3_exec(
        db: *mut Sqlite3,
        sql: *const c_char,
        callback: usize,
        arg: usize,
        errmsg: *mut *mut c_char,
    ) -> i32;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_errmsg(db: *mut Sqlite3) -> *const c_char;
    fn sqlite3_prepare_v2(
        db: *mut Sqlite3,
        z_sql: *const c_char,
        n_byte: i32,
        pp_stmt: *mut *mut Sqlite3Stmt,
        pz_tail: *mut *const c_char,
    ) -> i32;
    fn sqlite3_bind_int(stmt: *mut Sqlite3Stmt, idx: i32, val: i32) -> i32;
    fn sqlite3_bind_int64(stmt: *mut Sqlite3Stmt, idx: i32, val: i64) -> i32;
    fn sqlite3_bind_null(stmt: *mut Sqlite3Stmt, idx: i32) -> i32;
    fn sqlite3_bind_text(
        stmt: *mut Sqlite3Stmt,
        idx: i32,
        val: *const c_char,
        n: i32,
        destructor: isize,
    ) -> i32;
    fn sqlite3_step(stmt: *mut Sqlite3Stmt) -> i32;
    fn sqlite3_column_int(stmt: *mut Sqlite3Stmt, i_col: i32) -> i32;
    fn sqlite3_column_int64(stmt: *mut Sqlite3Stmt, i_col: i32) -> i64;
    fn sqlite3_column_text(stmt: *mut Sqlite3Stmt, i_col: i32) -> *const u8;
    fn sqlite3_finalize(stmt: *mut Sqlite3Stmt) -> i32;
}

#[derive(Debug, Clone)]
pub struct ErrorLogRecord {
    pub id: i64,
    pub timestamp: i64,
    pub boot_id: String,
    pub pid: i32,
    pub tgid: i32,
    pub parent_pid: i32,
    pub comm: String,
    pub cmdline: String,
    pub exit_code: i32,
    pub exit_signal: Option<i32>,
    pub summary: String,
    pub notification_body: String,
    pub stdout: String,
    pub stderr: String,
}

pub struct ErrorDatabase {
    db: *mut Sqlite3,
    boot_id: String,
}

impl ErrorDatabase {
    /// Clears any existing SQLite database files (`db_path`, `-wal`, `-shm`, `-journal`)
    /// and reinitializes a fresh database and schema at `db_path`.
    pub fn open(db_path: &Path) -> io::Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        clear_database_files(db_path);

        let path_str = db_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Database path is not valid UTF-8")
        })?;
        let c_path = CString::new(path_str)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let mut db: *mut Sqlite3 = ptr::null_mut();
        let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut db) };
        if rc != SQLITE_OK || db.is_null() {
            let msg = if !db.is_null() {
                let err = unsafe { sqlite_error_string(db) };
                unsafe { sqlite3_close(db) };
                err
            } else {
                format!("sqlite3_open failed with code {rc}")
            };
            return Err(io::Error::other(msg));
        }

        let boot_id = current_boot_id();
        let instance = Self { db, boot_id };
        instance.init_schema()?;

        // Ensure world-readable permissions so any user/UI can inspect the database
        if let Ok(meta) = fs::metadata(db_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o644);
            let _ = fs::set_permissions(db_path, perms);
        }

        Ok(instance)
    }

    /// Opens an existing database connection for fast read queries without clearing or reinitializing.
    pub fn open_query(db_path: &Path) -> io::Result<Self> {
        let path_str = db_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Database path is not valid UTF-8")
        })?;
        let c_path = CString::new(path_str)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let mut db: *mut Sqlite3 = ptr::null_mut();
        let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut db) };
        if rc != SQLITE_OK || db.is_null() {
            let msg = if !db.is_null() {
                let err = unsafe { sqlite_error_string(db) };
                unsafe { sqlite3_close(db) };
                err
            } else {
                format!("sqlite3_open failed with code {rc}")
            };
            return Err(io::Error::other(msg));
        }

        let instance = Self {
            db,
            boot_id: String::new(),
        };
        let _ = instance.exec_sql("PRAGMA busy_timeout=5000;");
        Ok(instance)
    }

    fn init_schema(&self) -> io::Result<()> {
        let sql = "\
            PRAGMA journal_mode=WAL;\
            PRAGMA synchronous=NORMAL;\
            PRAGMA busy_timeout=5000;\
            DROP TABLE IF EXISTS process_errors;\
            CREATE TABLE process_errors (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                timestamp INTEGER NOT NULL,\
                boot_id TEXT NOT NULL,\
                pid INTEGER NOT NULL,\
                tgid INTEGER NOT NULL,\
                parent_pid INTEGER NOT NULL,\
                comm TEXT NOT NULL,\
                cmdline TEXT NOT NULL,\
                exit_code INTEGER NOT NULL,\
                exit_signal INTEGER,\
                summary TEXT NOT NULL,\
                notification_body TEXT NOT NULL,\
                stdout TEXT NOT NULL,\
                stderr TEXT NOT NULL,\
                UNIQUE(pid, exit_code)\
            );\
            CREATE INDEX idx_process_errors_ts ON process_errors(timestamp);\
            CREATE INDEX idx_process_errors_pid ON process_errors(tgid);\
        ";
        self.exec_sql(sql)
    }

    pub fn insert_error(
        &self,
        ev: &ProcessExitEvent,
        summary: &str,
        notification_body: &str,
        stdout: &str,
        stderr: &str,
    ) -> io::Result<()> {
        let sql = CString::new(
            "INSERT INTO process_errors \
             (timestamp, boot_id, pid, tgid, parent_pid, comm, cmdline, exit_code, exit_signal, summary, notification_body, stdout, stderr) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
             ON CONFLICT(pid, exit_code) DO UPDATE SET \
                 timestamp = excluded.timestamp, \
                 boot_id = excluded.boot_id, \
                 tgid = excluded.tgid, \
                 parent_pid = excluded.parent_pid, \
                 comm = excluded.comm, \
                 cmdline = excluded.cmdline, \
                 exit_signal = excluded.exit_signal, \
                 summary = excluded.summary, \
                 notification_body = excluded.notification_body, \
                 stdout = excluded.stdout, \
                 stderr = excluded.stderr;",
        )
        .unwrap();

        let mut stmt: *mut Sqlite3Stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(self.db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != SQLITE_OK {
            return Err(io::Error::other(unsafe { sqlite_error_string(self.db) }));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        unsafe {
            sqlite3_bind_int64(stmt, 1, now);
            bind_str(stmt, 2, &self.boot_id);
            sqlite3_bind_int(stmt, 3, ev.pid);
            sqlite3_bind_int(stmt, 4, ev.tgid);
            sqlite3_bind_int(stmt, 5, ev.parent_pid);
            bind_str(stmt, 6, &ev.comm);
            bind_str(stmt, 7, &ev.cmdline);
            sqlite3_bind_int(stmt, 8, ev.exit_code);
            match ev.exit_signal {
                Some(sig) => sqlite3_bind_int(stmt, 9, sig),
                None => sqlite3_bind_null(stmt, 9),
            };
            bind_str(stmt, 10, summary);
            bind_str(stmt, 11, notification_body);
            bind_str(stmt, 12, stdout);
            bind_str(stmt, 13, stderr);

            let step_rc = sqlite3_step(stmt);
            sqlite3_finalize(stmt);
            if step_rc != SQLITE_DONE {
                return Err(io::Error::other(sqlite_error_string(self.db)));
            }
        }

        Ok(())
    }

    pub fn recent_errors(&self, limit: i32) -> io::Result<Vec<ErrorLogRecord>> {
        let sql = CString::new(
            "SELECT id, timestamp, boot_id, pid, tgid, parent_pid, comm, cmdline, exit_code, exit_signal, summary, notification_body, stdout, stderr \
             FROM process_errors ORDER BY timestamp DESC, id DESC LIMIT ?1;",
        )
        .unwrap();

        let mut stmt: *mut Sqlite3Stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(self.db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != SQLITE_OK {
            return Err(io::Error::other(unsafe { sqlite_error_string(self.db) }));
        }

        let mut rows = Vec::new();
        unsafe {
            sqlite3_bind_int(stmt, 1, limit);
            while sqlite3_step(stmt) == SQLITE_ROW {
                rows.push(read_row(stmt));
            }
            sqlite3_finalize(stmt);
        }
        rows.reverse();
        Ok(rows)
    }

    pub fn all_errors(&self) -> io::Result<Vec<ErrorLogRecord>> {
        let sql = CString::new(
            "SELECT id, timestamp, boot_id, pid, tgid, parent_pid, comm, cmdline, exit_code, exit_signal, summary, notification_body, stdout, stderr \
             FROM process_errors ORDER BY id ASC;",
        )
        .unwrap();

        let mut stmt: *mut Sqlite3Stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(self.db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != SQLITE_OK {
            return Err(io::Error::other(unsafe { sqlite_error_string(self.db) }));
        }

        let mut rows = Vec::new();
        unsafe {
            while sqlite3_step(stmt) == SQLITE_ROW {
                rows.push(read_row(stmt));
            }
            sqlite3_finalize(stmt);
        }
        Ok(rows)
    }

    pub fn get_error_by_id(&self, id: i64) -> io::Result<Option<ErrorLogRecord>> {
        let sql = CString::new(
            "SELECT id, timestamp, boot_id, pid, tgid, parent_pid, comm, cmdline, exit_code, exit_signal, summary, notification_body, stdout, stderr \
             FROM process_errors WHERE id = ?1 LIMIT 1;",
        )
        .unwrap();

        let mut stmt: *mut Sqlite3Stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(self.db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != SQLITE_OK {
            return Err(io::Error::other(unsafe { sqlite_error_string(self.db) }));
        }

        let record = unsafe {
            sqlite3_bind_int64(stmt, 1, id);
            let res = if sqlite3_step(stmt) == SQLITE_ROW {
                Some(read_row(stmt))
            } else {
                None
            };
            sqlite3_finalize(stmt);
            res
        };
        Ok(record)
    }

    fn exec_sql(&self, sql: &str) -> io::Result<()> {
        let c_sql = CString::new(sql)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        let rc = unsafe { sqlite3_exec(self.db, c_sql.as_ptr(), 0, 0, &mut errmsg) };
        if rc != SQLITE_OK {
            let msg = if !errmsg.is_null() {
                let s = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
                unsafe { sqlite3_free(errmsg.cast::<c_void>()) };
                s
            } else {
                unsafe { sqlite_error_string(self.db) }
            };
            return Err(io::Error::other(msg));
        }
        Ok(())
    }
}

impl Drop for ErrorDatabase {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                sqlite3_close(self.db);
            }
        }
    }
}

unsafe fn read_row(stmt: *mut Sqlite3Stmt) -> ErrorLogRecord {
    unsafe {
        let sig_val = sqlite3_column_int(stmt, 9);
        let exit_signal = if sig_val > 0 { Some(sig_val) } else { None };
        ErrorLogRecord {
            id: sqlite3_column_int64(stmt, 0),
            timestamp: sqlite3_column_int64(stmt, 1),
            boot_id: col_string(stmt, 2),
            pid: sqlite3_column_int(stmt, 3),
            tgid: sqlite3_column_int(stmt, 4),
            parent_pid: sqlite3_column_int(stmt, 5),
            comm: col_string(stmt, 6),
            cmdline: col_string(stmt, 7),
            exit_code: sqlite3_column_int(stmt, 8),
            exit_signal,
            summary: col_string(stmt, 10),
            notification_body: col_string(stmt, 11),
            stdout: col_string(stmt, 12),
            stderr: col_string(stmt, 13),
        }
    }
}

unsafe fn bind_str(stmt: *mut Sqlite3Stmt, idx: i32, s: &str) {
    unsafe {
        sqlite3_bind_text(
            stmt,
            idx,
            s.as_ptr().cast::<c_char>(),
            s.len() as i32,
            SQLITE_TRANSIENT,
        );
    }
}

unsafe fn col_string(stmt: *mut Sqlite3Stmt, idx: i32) -> String {
    let ptr = unsafe { sqlite3_column_text(stmt, idx) };
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr.cast::<c_char>()).to_string_lossy().into_owned() }
    }
}

unsafe fn sqlite_error_string(db: *mut Sqlite3) -> String {
    let ptr = unsafe { sqlite3_errmsg(db) };
    if ptr.is_null() {
        "Unknown SQLite error".to_string()
    } else {
        unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    }
}

fn clear_database_files(db_path: &Path) {
    let _ = fs::remove_file(db_path);
    let base = db_path.display();
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(format!("{base}{suffix}"));
    }
}

fn current_boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown-boot".to_string())
}

