//! SQLite WAL persistence layer (`JobStore`) for `radivine-worker` (`JOB_SYSTEM_DESIGN.md` §4).

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use radivine_jobs::{
    ItemState, JobId, JobItemReport, JobProgressCounts, JobState, JobSummary, MAX_ACTIVE_JOBS,
    MAX_ITEMS_PER_JOB, MAX_PENDING_ITEMS_TOTAL, Origin, OrphanedStageEntry, RequestId, RetryMode,
    WirePath, WorkerErrorCode, compute_terminal_job_state,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use thiserror::Error;

use crate::fault::{CrashPoint, maybe_crash_at};

/// Current supported SQLite schema version (`1`).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Errors returned by [`JobStore`].
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "database schema_version {found} is newer than supported version {supported}; refusing to open"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("worker error [{code:?}]: {message}")]
    Protocol {
        code: WorkerErrorCode,
        message: String,
    },
}

/// Claimed item record returned to a worker execution thread.
#[derive(Debug, Clone)]
pub struct ClaimedItem {
    pub item_id: i64,
    pub job_id: JobId,
    pub item_index: u32,
    pub source_path: PathBuf,
    pub attempt_count: u32,
}

/// Raw item row loaded during startup crash recovery (`C0a`–`C6`).
#[derive(Debug, Clone)]
pub struct RecoveryItemRow {
    pub item_id: i64,
    pub job_id: JobId,
    pub item_index: u32,
    pub state: ItemState,
    pub source_path: PathBuf,
    pub output_path: Option<PathBuf>,
    pub temp_path: Option<PathBuf>,
    pub temp_dev: Option<u64>,
    pub temp_ino: Option<u64>,
    pub stage_dir_path: Option<PathBuf>,
    pub stage_dev: Option<u64>,
    pub stage_ino: Option<u64>,
    pub warnings_json: String,
}

/// Outcome parameters when marking an item `Succeeded`.
#[derive(Debug, Clone)]
pub struct SucceededOutcome<'a> {
    pub output_path: &'a Path,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub duration_ms: u64,
    pub warnings_json: &'a str,
}

/// Outcome parameters when marking an item `Failed`.
#[derive(Debug, Clone)]
pub struct FailedOutcome<'a> {
    pub error_code: &'a str,
    pub error_class: &'a str,
    pub error_message: &'a str,
    pub retryable: bool,
    pub duration_ms: u64,
    pub warnings_json: &'a str,
}

/// Thread-safe single-writer SQLite WAL store for durable background jobs (`ADR-032`).
pub struct JobStore {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl JobStore {
    /// Opens or initializes the SQLite WAL job database at `db_path`, enforcing `0700` on parent
    /// directory and `0600` on `.sqlite3`, `-wal`, and `-shm` files (`SECURITY_AND_FILE_SAFETY.md` §7.1).
    pub fn open(db_path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        let conn = Connection::open(db_path)?;
        Self::enforce_db_file_permissions(db_path);

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )?;
        Self::enforce_db_file_permissions(db_path);

        Self::init_schema(&conn)?;

        Ok(Self {
            db_path: db_path.to_path_buf(),
            conn: Mutex::new(conn),
        })
    }

    /// Reads the `meta.schema_version` of an existing SQLite database without modifying it (`AC-C4.6`).
    pub fn inspect_db_schema_version(db_path: &Path) -> Result<Option<u32>, StoreError> {
        if !db_path.is_file() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let _ = conn.busy_timeout(std::time::Duration::from_millis(2000));
        let ver_str: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(ver_str.and_then(|s| s.parse::<u32>().ok()))
    }

    /// Opens an existing SQLite database in read-only mode regardless of `schema_version` (`AC-C4.6`).
    pub fn open_read_only_any_schema(db_path: &Path) -> Result<(Self, u32), StoreError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let _ = conn.busy_timeout(std::time::Duration::from_millis(2000));
        let ver_str: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let found = ver_str
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(CURRENT_SCHEMA_VERSION);
        Ok((
            Self {
                db_path: db_path.to_path_buf(),
                conn: Mutex::new(conn),
            },
            found,
        ))
    }

    fn enforce_db_file_permissions(db_path: &Path) {
        let _ = fs::set_permissions(db_path, fs::Permissions::from_mode(0o600));
        let mut wal_os = db_path.as_os_str().to_os_string();
        wal_os.push("-wal");
        let wal_path = PathBuf::from(wal_os);
        if wal_path.exists() {
            let _ = fs::set_permissions(&wal_path, fs::Permissions::from_mode(0o600));
        }
        let mut shm_os = db_path.as_os_str().to_os_string();
        shm_os.push("-shm");
        let shm_path = PathBuf::from(shm_os);
        if shm_path.exists() {
            let _ = fs::set_permissions(&shm_path, fs::Permissions::from_mode(0o600));
        }
    }

    fn init_schema(conn: &Connection) -> Result<(), StoreError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) STRICT;",
        )?;

        let existing_ver: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(v_str) = existing_ver {
            let found = v_str.parse::<u32>().unwrap_or(0);
            if found > CURRENT_SCHEMA_VERSION {
                return Err(StoreError::UnsupportedSchemaVersion {
                    found,
                    supported: CURRENT_SCHEMA_VERSION,
                });
            }
        } else {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![CURRENT_SCHEMA_VERSION.to_string()],
            )?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id       TEXT NOT NULL UNIQUE,
                origin           TEXT NOT NULL,
                retry_of         INTEGER REFERENCES jobs(id) ON DELETE SET NULL,
                kind             TEXT NOT NULL,
                state            TEXT NOT NULL,
                created_at_ms    INTEGER NOT NULL,
                started_at_ms    INTEGER,
                finished_at_ms   INTEGER,
                updated_at_ms    INTEGER NOT NULL,
                total_items      INTEGER NOT NULL,
                pending_items    INTEGER NOT NULL,
                succeeded_items  INTEGER NOT NULL DEFAULT 0,
                skipped_items    INTEGER NOT NULL DEFAULT 0,
                failed_items     INTEGER NOT NULL DEFAULT 0,
                cancelled_items  INTEGER NOT NULL DEFAULT 0,
                options          TEXT NOT NULL,
                resolved         TEXT,
                summary_text     TEXT
            ) STRICT;

            CREATE TABLE IF NOT EXISTS job_items (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id           INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                item_index       INTEGER NOT NULL,
                source_path      BLOB NOT NULL,
                source_dev       INTEGER,
                source_ino       INTEGER,
                source_size      INTEGER,
                source_mtime_ns  INTEGER,
                output_path      BLOB,
                temp_path        BLOB,
                temp_dev         INTEGER,
                temp_ino         INTEGER,
                stage_dir_path   BLOB,
                stage_dev        INTEGER,
                stage_ino        INTEGER,
                state            TEXT NOT NULL,
                stage            TEXT,
                attempt_count    INTEGER NOT NULL DEFAULT 0,
                started_at_ms    INTEGER,
                finished_at_ms   INTEGER,
                duration_ms      INTEGER,
                output_bytes     INTEGER,
                error_code       TEXT,
                error_class      TEXT,
                error_message    TEXT,
                retryable        INTEGER NOT NULL DEFAULT 0,
                warnings         TEXT NOT NULL DEFAULT '[]',
                UNIQUE(job_id, item_index)
            ) STRICT;

            CREATE INDEX IF NOT EXISTS idx_jobs_state_created ON jobs(state, created_at_ms);
            CREATE INDEX IF NOT EXISTS idx_job_items_job_state ON job_items(job_id, state);
            CREATE INDEX IF NOT EXISTS idx_job_items_state ON job_items(state);",
        )?;

        // Ensure forward compatibility with any v1 table created before `retry_of` column was added
        let has_retry_of: bool = {
            let mut stmt = conn.prepare("PRAGMA table_info(jobs)")?;
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            cols.iter().any(|c| c == "retry_of")
        };
        if !has_retry_of {
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN retry_of INTEGER REFERENCES jobs(id) ON DELETE SET NULL;",
            )?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_jobs_retry_of ON jobs(retry_of) WHERE retry_of IS NOT NULL;",
        )?;

        Ok(())
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Executes `f` under `PRAGMA synchronous = FULL` for crash-safe durability (`JOB_SYSTEM_DESIGN.md` §4.4).
    fn with_sync_full<T, F>(&self, conn: &Connection, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError>,
    {
        conn.execute_batch("PRAGMA synchronous = FULL;")?;
        let res = f(conn);
        let _ = conn.execute_batch("PRAGMA synchronous = NORMAL;");
        res
    }

    /// Reads `(clean_shutdown, last_boot_id)` from `meta` and marks `clean_shutdown = '0'` and
    /// `last_boot_id = current_boot_id` under `PRAGMA synchronous = FULL`.
    pub fn begin_worker_session(
        &self,
        current_boot_id: &str,
    ) -> Result<(bool, Option<String>), StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            let clean: Option<String> = c
                .query_row(
                    "SELECT value FROM meta WHERE key = 'clean_shutdown'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let prev_boot: Option<String> = c
                .query_row(
                    "SELECT value FROM meta WHERE key = 'last_boot_id'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;

            c.execute(
                "INSERT INTO meta (key, value) VALUES ('clean_shutdown', '0')
                 ON CONFLICT(key) DO UPDATE SET value = '0'",
                [],
            )?;
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('last_boot_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![current_boot_id],
            )?;
            Ok((clean.as_deref() == Some("1"), prev_boot))
        })
    }

    /// Marks `clean_shutdown = '1'` and runs `PRAGMA wal_checkpoint(TRUNCATE)` on clean exit.
    pub fn finish_clean_shutdown(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "INSERT INTO meta (key, value) VALUES ('clean_shutdown', '1')
                 ON CONFLICT(key) DO UPDATE SET value = '1'",
                [],
            )?;
            Ok(())
        })?;
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        Ok(())
    }

    /// Runs `PRAGMA wal_checkpoint(TRUNCATE)`.
    pub fn wal_checkpoint_truncate(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Submits a new job or returns the existing `(JobId, JobState, true)` if `request_id` was
    /// already submitted (`AC-C2.1` idempotency).
    pub fn submit_or_deduplicate(
        &self,
        request_id: &RequestId,
        origin: Origin,
        kind: &str,
        items: &[PathBuf],
        options: &serde_json::Value,
    ) -> Result<(JobId, JobState, bool), StoreError> {
        if let Err(msg) = request_id.validate() {
            return Err(StoreError::Protocol {
                code: WorkerErrorCode::InvalidRequestId,
                message: msg,
            });
        }
        if items.is_empty() || items.len() > MAX_ITEMS_PER_JOB {
            return Err(StoreError::Protocol {
                code: WorkerErrorCode::TooManyItems,
                message: format!(
                    "submit items count {} must be between 1 and {MAX_ITEMS_PER_JOB}",
                    items.len()
                ),
            });
        }

        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute_batch("BEGIN IMMEDIATE;")?;
            let result = (|| -> Result<(JobId, JobState, bool), StoreError> {
                if let Some((existing_id, state_str)) = c
                    .query_row(
                        "SELECT id, state FROM jobs WHERE request_id = ?1",
                        params![request_id.as_str()],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?
                {
                    let st = state_str.parse::<JobState>().unwrap_or(JobState::Queued);
                    return Ok((JobId(existing_id as u64), st, true));
                }

                // Enforce queue capacity caps (256 active jobs, 50,000 pending items)
                let active_jobs: i64 = c.query_row(
                    "SELECT COUNT(*) FROM jobs WHERE state IN ('queued', 'preparing', 'running')",
                    [],
                    |r| r.get(0),
                )?;
                if active_jobs as usize >= MAX_ACTIVE_JOBS {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::QueueFull,
                        message: format!(
                            "active job limit ({MAX_ACTIVE_JOBS}) reached; wait for running jobs to finish"
                        ),
                    });
                }

                let pending_total: i64 = c.query_row(
                    "SELECT COUNT(*) FROM job_items WHERE state IN ('pending', 'inspecting', 'running', 'committing')",
                    [],
                    |r| r.get(0),
                )?;
                if (pending_total as usize).saturating_add(items.len()) > MAX_PENDING_ITEMS_TOTAL {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::QueueFull,
                        message: format!(
                            "total pending item limit ({MAX_PENDING_ITEMS_TOTAL}) exceeded"
                        ),
                    });
                }

                let now = now_unix_ms();
                let options_str = serde_json::to_string(options).unwrap_or_else(|_| "{}".to_string());
                let total_items = items.len() as i64;

                c.execute(
                    "INSERT INTO jobs (
                        request_id, origin, kind, state, created_at_ms, updated_at_ms,
                        total_items, pending_items, options
                     ) VALUES (?1, ?2, ?3, 'queued', ?4, ?4, ?5, ?5, ?6)",
                    params![
                        request_id.as_str(),
                        origin.as_str(),
                        kind,
                        now,
                        total_items,
                        options_str,
                    ],
                )?;
                let job_id_raw = c.last_insert_rowid();

                {
                    let mut stmt = c.prepare(
                        "INSERT INTO job_items (
                            job_id, item_index, source_path, state
                         ) VALUES (?1, ?2, ?3, 'pending')",
                    )?;
                    for (idx, path) in items.iter().enumerate() {
                        stmt.execute(params![
                            job_id_raw,
                            idx as i64,
                            path.as_os_str().as_bytes(),
                        ])?;
                    }
                }

                Ok((JobId(job_id_raw as u64), JobState::Queued, false))
            })();

            match result {
                Ok(v) => {
                    c.execute_batch("COMMIT;")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = c.execute_batch("ROLLBACK;");
                    Err(e)
                }
            }
        })
    }

    /// Returns all active jobs (`queued`, `preparing`, `running`) ordered by `id ASC`.
    pub fn list_active_jobs(&self) -> Result<Vec<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, request_id, origin, retry_of, kind, state, created_at_ms, started_at_ms,
                    finished_at_ms, updated_at_ms, total_items, pending_items,
                    succeeded_items, skipped_items, failed_items, cancelled_items,
                    options, resolved, summary_text
             FROM jobs
             WHERE state IN ('queued', 'preparing', 'running')
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], Self::map_job_summary_row)?;
        let mut out = Vec::new();
        for r in rows {
            let mut job = r?;
            Self::populate_extra_counts(&conn, &mut job)?;
            out.push(job);
        }
        Ok(out)
    }

    /// Lists jobs ordered by `created_at_ms DESC, id DESC`.
    pub fn list_jobs(
        &self,
        limit: u32,
        include_terminal: bool,
    ) -> Result<Vec<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let sql = if include_terminal {
            "SELECT id, request_id, origin, retry_of, kind, state, created_at_ms, started_at_ms,
                    finished_at_ms, updated_at_ms, total_items, pending_items,
                    succeeded_items, skipped_items, failed_items, cancelled_items,
                    options, resolved, summary_text
             FROM jobs
             ORDER BY id DESC
             LIMIT ?1"
        } else {
            "SELECT id, request_id, origin, retry_of, kind, state, created_at_ms, started_at_ms,
                    finished_at_ms, updated_at_ms, total_items, pending_items,
                    succeeded_items, skipped_items, failed_items, cancelled_items,
                    options, resolved, summary_text
             FROM jobs
             WHERE state IN ('queued', 'preparing', 'running')
             ORDER BY id DESC
             LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit.clamp(1, 500)], Self::map_job_summary_row)?;
        let mut out = Vec::new();
        for r in rows {
            let mut job = r?;
            Self::populate_extra_counts(&conn, &mut job)?;
            out.push(job);
        }
        Ok(out)
    }

    /// Queries a single [`JobSummary`] by [`JobId`].
    pub fn get_job_summary(&self, job_id: JobId) -> Result<Option<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        self.get_job_summary_locked(&conn, job_id)
    }

    fn get_job_summary_locked(
        &self,
        conn: &Connection,
        job_id: JobId,
    ) -> Result<Option<JobSummary>, StoreError> {
        let opt = conn
            .query_row(
                "SELECT id, request_id, origin, retry_of, kind, state, created_at_ms, started_at_ms,
                        finished_at_ms, updated_at_ms, total_items, pending_items,
                        succeeded_items, skipped_items, failed_items, cancelled_items,
                        options, resolved, summary_text
                 FROM jobs WHERE id = ?1",
                params![job_id.0 as i64],
                Self::map_job_summary_row,
            )
            .optional()?;
        if let Some(mut j) = opt {
            Self::populate_extra_counts(conn, &mut j)?;
            Ok(Some(j))
        } else {
            Ok(None)
        }
    }

    /// Queries a job summary and paginated [`JobItemReport`] list.
    pub fn get_job_detail(
        &self,
        job_id: JobId,
        item_offset: Option<u32>,
        item_limit: Option<u32>,
    ) -> Result<Option<(JobSummary, Vec<JobItemReport>, u32)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let Some(job) = self.get_job_summary_locked(&conn, job_id)? else {
            return Ok(None);
        };
        let offset = i64::from(item_offset.unwrap_or(0));
        let limit = i64::from(item_limit.unwrap_or(10_000).min(10_000));

        let mut stmt = conn.prepare(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items
             WHERE job_id = ?1
             ORDER BY item_index ASC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(
            params![job_id.0 as i64, limit, offset],
            Self::map_item_report_row,
        )?;
        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        let total = job.counts.total_items;
        Ok(Some((job, items, total)))
    }

    /// Queries a single [`JobItemReport`] by `item_id`.
    pub fn get_item_report(&self, item_id: i64) -> Result<Option<JobItemReport>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items
             WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )
        .optional()
        .map_err(StoreError::from)
    }

    /// Returns `(active_jobs_count, queued_items_count)` for `Ping`/`Pong` and idle checks.
    pub fn active_counts(&self) -> Result<(u32, u32), StoreError> {
        let conn = self.conn.lock().unwrap();
        let active_jobs: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('queued', 'preparing', 'running')",
            [],
            |r| r.get(0),
        )?;
        let queued_items: i64 = conn.query_row(
            "SELECT COUNT(*) FROM job_items WHERE state IN ('pending', 'inspecting', 'running', 'committing')",
            [],
            |r| r.get(0),
        )?;
        Ok((active_jobs as u32, queued_items as u32))
    }

    /// Transitions a job from `queued` to `preparing`.
    pub fn mark_job_preparing(&self, job_id: JobId) -> Result<Option<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        let changed = conn.execute(
            "UPDATE jobs
             SET state = 'preparing',
                 started_at_ms = COALESCE(started_at_ms, ?2),
                 updated_at_ms = ?2
             WHERE id = ?1 AND state = 'queued'",
            params![job_id.0 as i64, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_job_summary_locked(&conn, job_id)
    }

    /// Stores the resolved options snapshot and transitions a job from `preparing` to `running`.
    pub fn mark_job_running(
        &self,
        job_id: JobId,
        resolved: &serde_json::Value,
    ) -> Result<Option<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        let resolved_str = serde_json::to_string(resolved).unwrap_or_else(|_| "{}".to_string());
        let changed = conn.execute(
            "UPDATE jobs
             SET state = 'running',
                 resolved = ?2,
                 updated_at_ms = ?3
             WHERE id = ?1 AND state = 'preparing'",
            params![job_id.0 as i64, resolved_str, now],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_job_summary_locked(&conn, job_id)
    }

    /// Fails an entire job during `Preparing` (e.g., invalid options or unknown backend).
    pub fn fail_job_in_preparing(
        &self,
        job_id: JobId,
        error_code: &str,
        error_class: &str,
        error_message: &str,
    ) -> Result<Option<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'failed',
                 finished_at_ms = ?2,
                 error_code = ?3,
                 error_class = ?4,
                 error_message = ?5,
                 retryable = 0
             WHERE job_id = ?1 AND state = 'pending'",
            params![job_id.0 as i64, now, error_code, error_class, error_message],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        self.get_job_summary_locked(&conn, job_id)
    }

    /// Claims the next `pending` item for `job_id`, transitioning it to `inspecting` (`attempt_count + 1`).
    pub fn claim_next_pending_item_for_job(
        &self,
        job_id: JobId,
    ) -> Result<Option<ClaimedItem>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let res = (|| -> Result<Option<ClaimedItem>, StoreError> {
            let job_state_str: Option<String> = conn
                .query_row(
                    "SELECT state FROM jobs WHERE id = ?1",
                    params![job_id.0 as i64],
                    |r| r.get(0),
                )
                .optional()?;
            if job_state_str.as_deref() != Some("running") {
                return Ok(None);
            }

            let next_row: Option<(i64, i64, Vec<u8>, i64)> = conn
                .query_row(
                    "SELECT id, item_index, source_path, attempt_count
                     FROM job_items
                     WHERE job_id = ?1 AND state = 'pending'
                     ORDER BY item_index ASC
                     LIMIT 1",
                    params![job_id.0 as i64],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;

            let Some((item_id, item_index, src_bytes, prev_attempts)) = next_row else {
                return Ok(None);
            };

            let now = now_unix_ms();
            let attempt = (prev_attempts + 1) as u32;
            conn.execute(
                "UPDATE job_items
                 SET state = 'inspecting',
                     stage = 'inspecting',
                     attempt_count = ?2,
                     started_at_ms = ?3,
                     finished_at_ms = NULL,
                     duration_ms = NULL,
                     error_code = NULL,
                     error_class = NULL,
                     error_message = NULL
                 WHERE id = ?1",
                params![item_id, i64::from(attempt), now],
            )?;
            conn.execute(
                "UPDATE jobs
                 SET pending_items = CASE WHEN pending_items > 0 THEN pending_items - 1 ELSE 0 END,
                     updated_at_ms = ?2
                 WHERE id = ?1",
                params![job_id.0 as i64, now],
            )?;

            Ok(Some(ClaimedItem {
                item_id,
                job_id,
                item_index: item_index as u32,
                source_path: PathBuf::from(OsStr::from_bytes(&src_bytes)),
                attempt_count: attempt,
            }))
        })();

        match res {
            Ok(v) => {
                conn.execute_batch("COMMIT;")?;
                Ok(v)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Updates an in-flight item's `state` and `stage` (`Inspecting` / `Running`) and Returns
    /// updated [`JobProgressCounts`].
    pub fn update_item_stage(
        &self,
        job_id: JobId,
        item_id: i64,
        state: ItemState,
        stage: &str,
    ) -> Result<JobProgressCounts, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items SET state = ?2, stage = ?3 WHERE id = ?1",
            params![item_id, state.as_str(), stage],
        )?;
        conn.execute(
            "UPDATE jobs SET updated_at_ms = ?2 WHERE id = ?1",
            params![job_id.0 as i64, now],
        )?;
        self.query_progress_counts_locked(&conn, job_id)
    }

    /// Records source file identity (`dev`, `ino`, `size`, `mtime_ns`) on `job_items`.
    pub fn record_source_identity(
        &self,
        item_id: i64,
        dev: u64,
        ino: u64,
        size: u64,
        mtime_ns: i128,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let mtime_i64 = i64::try_from(mtime_ns).unwrap_or(0);
        conn.execute(
            "UPDATE job_items
             SET source_dev = ?2, source_ino = ?3, source_size = ?4, source_mtime_ns = ?5
             WHERE id = ?1",
            params![item_id, dev as i64, ino as i64, size as i64, mtime_i64],
        )?;
        Ok(())
    }

    /// `C0a` pre-creation journal: commits `stage_dir_path` with `stage_dev = NULL, stage_ino = NULL`
    /// under `PRAGMA synchronous = FULL` BEFORE `mkdirat` (`SECURITY_AND_FILE_SAFETY.md` §3.6).
    pub fn record_stage_intent(
        &self,
        item_id: i64,
        stage_dir_path: &Path,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET stage_dir_path = ?2, stage_dev = NULL, stage_ino = NULL
                 WHERE id = ?1",
                params![item_id, stage_dir_path.as_os_str().as_bytes()],
            )?;
            Ok(())
        })?;
        drop(conn);
        maybe_crash_at(CrashPoint::C0a);
        Ok(())
    }

    /// `C0b` post-creation journal: commits `(stage_dev, stage_ino)` under `PRAGMA synchronous = FULL`
    /// immediately after `mkdirat` + `fstat` (`SECURITY_AND_FILE_SAFETY.md` §3.6).
    pub fn record_stage_created(
        &self,
        item_id: i64,
        stage_dir_path: &Path,
        stage_dev: u64,
        stage_ino: u64,
    ) -> Result<(), StoreError> {
        maybe_crash_at(CrashPoint::C0b);
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET stage_dir_path = ?2, stage_dev = ?3, stage_ino = ?4
                 WHERE id = ?1",
                params![
                    item_id,
                    stage_dir_path.as_os_str().as_bytes(),
                    stage_dev as i64,
                    stage_ino as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// `C1` temp-creation journal: commits `(temp_path, temp_dev, temp_ino)` under
    /// `PRAGMA synchronous = FULL` immediately after `openat(.part)` (`SECURITY_AND_FILE_SAFETY.md` §3.6).
    pub fn record_temp_created(
        &self,
        item_id: i64,
        stage_dir_path: &Path,
        temp_path: &Path,
        temp_dev: u64,
        temp_ino: u64,
    ) -> Result<(), StoreError> {
        maybe_crash_at(CrashPoint::C1Unjournaled);
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET stage_dir_path = ?2, temp_path = ?3, temp_dev = ?4, temp_ino = ?5
                 WHERE id = ?1",
                params![
                    item_id,
                    stage_dir_path.as_os_str().as_bytes(),
                    temp_path.as_os_str().as_bytes(),
                    temp_dev as i64,
                    temp_ino as i64,
                ],
            )?;
            Ok(())
        })?;
        drop(conn);
        maybe_crash_at(CrashPoint::C1Journaled);
        Ok(())
    }

    /// `C2` pre-commit journal: commits `state = 'committing'`, `stage = 'committing'`, and the
    /// exact candidate `output_path` + `(temp_dev, temp_ino)` under `PRAGMA synchronous = FULL`
    /// BEFORE `commit_no_replace` is invoked (`SECURITY_AND_FILE_SAFETY.md` §3.6).
    pub fn record_committing_candidate(
        &self,
        item_id: i64,
        candidate_output_path: &Path,
        temp_dev: u64,
        temp_ino: u64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET state = 'committing',
                     stage = 'committing',
                     output_path = ?2,
                     temp_dev = ?3,
                     temp_ino = ?4
                 WHERE id = ?1",
                params![
                    item_id,
                    candidate_output_path.as_os_str().as_bytes(),
                    temp_dev as i64,
                    temp_ino as i64,
                ],
            )?;
            Ok(())
        })?;
        drop(conn);
        maybe_crash_at(CrashPoint::C2);
        Ok(())
    }

    /// Marks an item `Succeeded` and updates parent job counts (`C5` / `C6` crash hooks supported).
    pub fn complete_item_succeeded(
        &self,
        job_id: JobId,
        item_id: i64,
        outcome: &SucceededOutcome<'_>,
    ) -> Result<(JobItemReport, JobSummary, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();

        if CrashPoint::C5.matches_env() {
            // Commit terminal Succeeded state with stage_dir_path still non-NULL, then abort at C5!
            self.with_sync_full(&conn, |c| {
                c.execute(
                    "UPDATE job_items
                     SET state = 'succeeded',
                         stage = NULL,
                         output_path = ?2,
                         temp_path = NULL,
                         temp_dev = NULL,
                         temp_ino = NULL,
                         source_size = ?3,
                         output_bytes = ?4,
                         duration_ms = ?5,
                         finished_at_ms = ?6,
                         warnings = ?7
                     WHERE id = ?1",
                    params![
                        item_id,
                        outcome.output_path.as_os_str().as_bytes(),
                        outcome.source_bytes as i64,
                        outcome.output_bytes as i64,
                        outcome.duration_ms as i64,
                        now,
                        outcome.warnings_json,
                    ],
                )?;
                Ok(())
            })?;
            drop(conn);
            maybe_crash_at(CrashPoint::C5);
            unreachable!();
        }

        let finalize_parent = !CrashPoint::C6.matches_env();
        let sync_for_c6 = CrashPoint::C6.matches_env();

        let update_fn = |c: &Connection| -> Result<(), StoreError> {
            c.execute(
                "UPDATE job_items
                 SET state = 'succeeded',
                     stage = NULL,
                     output_path = ?2,
                     temp_path = NULL,
                     temp_dev = NULL,
                     temp_ino = NULL,
                     stage_dir_path = NULL,
                     stage_dev = NULL,
                     stage_ino = NULL,
                     source_size = ?3,
                     output_bytes = ?4,
                     duration_ms = ?5,
                     finished_at_ms = ?6,
                     warnings = ?7
                 WHERE id = ?1",
                params![
                    item_id,
                    outcome.output_path.as_os_str().as_bytes(),
                    outcome.source_bytes as i64,
                    outcome.output_bytes as i64,
                    outcome.duration_ms as i64,
                    now,
                    outcome.warnings_json,
                ],
            )?;
            self.recompute_and_update_job_locked(c, job_id, finalize_parent)?;
            Ok(())
        };

        if sync_for_c6 {
            self.with_sync_full(&conn, update_fn)?;
            drop(conn);
            maybe_crash_at(CrashPoint::C6);
            unreachable!();
        } else {
            update_fn(&conn)?;
        }

        let item = conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )?;
        let job = self.get_job_summary_locked(&conn, job_id)?.unwrap();
        let job_finished = job.state.is_terminal();
        Ok((item, job, job_finished))
    }

    /// Marks an item `Skipped` and updates parent job counts.
    pub fn complete_item_skipped(
        &self,
        job_id: JobId,
        item_id: i64,
        reason_code: &str,
        reason_message: &str,
        warnings_json: &str,
    ) -> Result<(JobItemReport, JobSummary, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'skipped',
                 stage = NULL,
                 output_path = NULL,
                 temp_path = NULL,
                 temp_dev = NULL,
                 temp_ino = NULL,
                 stage_dir_path = NULL,
                 stage_dev = NULL,
                 stage_ino = NULL,
                 finished_at_ms = ?2,
                 error_code = ?3,
                 error_class = 'skipped',
                 error_message = ?4,
                 retryable = 0,
                 warnings = ?5
             WHERE id = ?1",
            params![item_id, now, reason_code, reason_message, warnings_json],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        let item = conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )?;
        let job = self.get_job_summary_locked(&conn, job_id)?.unwrap();
        let job_finished = job.state.is_terminal();
        Ok((item, job, job_finished))
    }

    /// Marks an item `Failed` and updates parent job counts.
    ///
    /// If `preserve_stage_journal_for_recovery` is `true` (used when an item times out so the
    /// replacement worker's `C2`/`C3`/`C5` startup recovery cleans up `.part` / `.rvstage-*`),
    /// `temp_path`, `temp_dev`, `temp_ino`, `stage_dir_path`, `stage_dev`, `stage_ino` are preserved!
    pub fn complete_item_failed(
        &self,
        job_id: JobId,
        item_id: i64,
        outcome: &FailedOutcome<'_>,
        preserve_stage_journal_for_recovery: bool,
    ) -> Result<(JobItemReport, JobSummary, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        let update_fn = |c: &Connection| -> Result<(), StoreError> {
            if preserve_stage_journal_for_recovery {
                c.execute(
                    "UPDATE job_items
                     SET state = 'failed',
                         stage = NULL,
                         finished_at_ms = ?2,
                         duration_ms = ?3,
                         error_code = ?4,
                         error_class = ?5,
                         error_message = ?6,
                         retryable = ?7,
                         warnings = ?8
                     WHERE id = ?1",
                    params![
                        item_id,
                        now,
                        outcome.duration_ms as i64,
                        outcome.error_code,
                        outcome.error_class,
                        outcome.error_message,
                        i64::from(outcome.retryable),
                        outcome.warnings_json,
                    ],
                )?;
            } else {
                c.execute(
                    "UPDATE job_items
                     SET state = 'failed',
                         stage = NULL,
                         output_path = NULL,
                         temp_path = NULL,
                         temp_dev = NULL,
                         temp_ino = NULL,
                         stage_dir_path = NULL,
                         stage_dev = NULL,
                         stage_ino = NULL,
                         finished_at_ms = ?2,
                         duration_ms = ?3,
                         error_code = ?4,
                         error_class = ?5,
                         error_message = ?6,
                         retryable = ?7,
                         warnings = ?8
                     WHERE id = ?1",
                    params![
                        item_id,
                        now,
                        outcome.duration_ms as i64,
                        outcome.error_code,
                        outcome.error_class,
                        outcome.error_message,
                        i64::from(outcome.retryable),
                        outcome.warnings_json,
                    ],
                )?;
            }
            self.recompute_and_update_job_locked(c, job_id, true)?;
            Ok(())
        };

        if preserve_stage_journal_for_recovery {
            self.with_sync_full(&conn, update_fn)?;
        } else {
            update_fn(&conn)?;
        }

        let item = conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )?;
        let job = self.get_job_summary_locked(&conn, job_id)?.unwrap();
        let job_finished = job.state.is_terminal();
        Ok((item, job, job_finished))
    }

    /// Marks an in-flight item `Cancelled` and updates parent job counts.
    pub fn complete_item_cancelled(
        &self,
        job_id: JobId,
        item_id: i64,
        duration_ms: u64,
    ) -> Result<(JobItemReport, JobSummary, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'cancelled',
                 stage = NULL,
                 output_path = NULL,
                 temp_path = NULL,
                 temp_dev = NULL,
                 temp_ino = NULL,
                 stage_dir_path = NULL,
                 stage_dev = NULL,
                 stage_ino = NULL,
                 finished_at_ms = ?2,
                 duration_ms = ?3,
                 error_code = 'cancelled',
                 error_class = 'cancelled',
                 error_message = 'Conversion cancelled by user',
                 retryable = 1
             WHERE id = ?1",
            params![item_id, now, duration_ms as i64],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        let item = conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )?;
        let job = self.get_job_summary_locked(&conn, job_id)?.unwrap();
        let job_finished = job.state.is_terminal();
        Ok((item, job, job_finished))
    }

    /// Fast-fails all remaining `pending` items of `job_id` whose source parent directory matches
    /// `dest_parent` (or all remaining `pending` items of `job_id`) after 3 consecutive
    /// `NoSpace` / `DestinationNotWritable` errors (`JOB_SYSTEM_DESIGN.md` §6.3).
    pub fn fail_remaining_pending_items(
        &self,
        job_id: JobId,
        error_code: &str,
        error_class: &str,
        error_message: &str,
    ) -> Result<Option<JobSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'failed',
                 stage = NULL,
                 finished_at_ms = ?2,
                 error_code = ?3,
                 error_class = ?4,
                 error_message = ?5,
                 retryable = 1
             WHERE job_id = ?1 AND state = 'pending'",
            params![job_id.0 as i64, now, error_code, error_class, error_message],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        self.get_job_summary_locked(&conn, job_id)
    }

    /// Marks all `pending` items of `job_id` `cancelled` and recomputes job state (`AC-C2.9`).
    pub fn cancel_job(&self, job_id: JobId) -> Result<JobSummary, StoreError> {
        let conn = self.conn.lock().unwrap();
        let Some(existing) = self.get_job_summary_locked(&conn, job_id)? else {
            return Err(StoreError::Protocol {
                code: WorkerErrorCode::JobNotFound,
                message: format!("job {job_id} not found"),
            });
        };
        if existing.state.is_terminal() {
            return Ok(existing);
        }

        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'cancelled',
                 stage = NULL,
                 finished_at_ms = ?2,
                 error_code = 'cancelled',
                 error_class = 'cancelled',
                 error_message = 'Conversion cancelled by user',
                 retryable = 1
             WHERE job_id = ?1 AND state = 'pending'",
            params![job_id.0 as i64, now],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        Ok(self.get_job_summary_locked(&conn, job_id)?.unwrap())
    }

    /// Creates a new linked job (`retry_of = Some(job_id)`, `origin = Origin::Retry`) containing
    /// only the eligible items of terminal job `job_id` according to `mode`, reusing the original
    /// `jobs.resolved` snapshot unless `options_override` is supplied (`ADR-034`, `JOB_SYSTEM_DESIGN.md` §3.4, `AC-C2.10`).
    ///
    /// Leaves the original terminal job and its item history 100% immutable.
    /// If `request_id` was already submitted, returns the existing retry job with `deduplicated = true` (`AC-C2.2`).
    pub fn retry_job(
        &self,
        request_id: &RequestId,
        job_id: JobId,
        mode: RetryMode,
        options_override: Option<&serde_json::Value>,
    ) -> Result<(JobSummary, u32, bool), StoreError> {
        if let Err(msg) = request_id.validate() {
            return Err(StoreError::Protocol {
                code: WorkerErrorCode::InvalidRequestId,
                message: msg,
            });
        }

        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute_batch("BEGIN IMMEDIATE;")?;
            let res = (|| -> Result<(JobSummary, u32, bool), StoreError> {
                // 1. Idempotency check (`AC-C2.2`)
                if let Some(existing_retry_id) = c
                    .query_row(
                        "SELECT id FROM jobs WHERE request_id = ?1",
                        params![request_id.as_str()],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    let existing_retry = self
                        .get_job_summary_locked(c, JobId(existing_retry_id as u64))?
                        .unwrap();
                    let count = existing_retry.counts.total_items;
                    return Ok((existing_retry, count, true));
                }

                // 2. Validate source job exists and is terminal
                let Some(existing) = self.get_job_summary_locked(c, job_id)? else {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::JobNotFound,
                        message: format!("job {job_id} not found"),
                    });
                };
                if existing.state.is_active() {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::InvalidJobState,
                        message: format!(
                            "cannot retry active job {job_id} in state {}",
                            existing.state
                        ),
                    });
                }

                let state_filter = match mode {
                    RetryMode::FailedOnly => "state IN ('failed', 'interrupted')",
                    RetryMode::FailedAndCancelled => {
                        "state IN ('failed', 'interrupted', 'cancelled')"
                    }
                    RetryMode::All => {
                        "state IN ('succeeded', 'skipped', 'failed', 'interrupted', 'cancelled')"
                    }
                };

                let select_sql = format!(
                    "SELECT source_path, attempt_count
                     FROM job_items
                     WHERE job_id = ?1 AND {state_filter}
                     ORDER BY item_index ASC"
                );
                let eligible_items: Vec<(Vec<u8>, i64)> = {
                    let mut stmt = c.prepare(&select_sql)?;
                    let rows = stmt.query_map(params![job_id.0 as i64], |r| {
                        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };

                let eligible_count = eligible_items.len() as u32;
                if eligible_count == 0 {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::InvalidJobState,
                        message: format!(
                            "job {job_id} has no items eligible for retry under mode {}",
                            mode.as_str()
                        ),
                    });
                }

                // 3. Enforce queue capacity caps (256 active jobs, 50,000 pending items)
                let active_jobs: i64 = c.query_row(
                    "SELECT COUNT(*) FROM jobs WHERE state IN ('queued', 'preparing', 'running')",
                    [],
                    |r| r.get(0),
                )?;
                if active_jobs as usize >= MAX_ACTIVE_JOBS {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::QueueFull,
                        message: format!(
                            "active job limit ({MAX_ACTIVE_JOBS}) reached; wait for running jobs to finish"
                        ),
                    });
                }

                let pending_total: i64 = c.query_row(
                    "SELECT COUNT(*) FROM job_items WHERE state IN ('pending', 'inspecting', 'running', 'committing')",
                    [],
                    |r| r.get(0),
                )?;
                if (pending_total as usize).saturating_add(eligible_items.len())
                    > MAX_PENDING_ITEMS_TOTAL
                {
                    return Err(StoreError::Protocol {
                        code: WorkerErrorCode::QueueFull,
                        message: format!(
                            "total pending item limit ({MAX_PENDING_ITEMS_TOTAL}) exceeded"
                        ),
                    });
                }

                // 4. Snapshot or merge options (`ADR-034`, `AC-C2.10`)
                let (new_options_val, new_resolved_val) = match options_override {
                    None => (existing.options.clone(), existing.resolved.clone()),
                    Some(ov) => merge_retry_options_override(
                        &existing.options,
                        existing.resolved.as_ref(),
                        ov,
                    ),
                };

                let now = now_unix_ms();
                let options_str =
                    serde_json::to_string(&new_options_val).unwrap_or_else(|_| "{}".to_string());
                let resolved_str = new_resolved_val
                    .as_ref()
                    .and_then(|v| serde_json::to_string(v).ok());
                let total_items_i64 = i64::from(eligible_count);

                // 5. Insert new linked job row (`origin = 'retry'`, `retry_of = job_id`)
                c.execute(
                    "INSERT INTO jobs (
                        request_id, origin, retry_of, kind, state, created_at_ms, updated_at_ms,
                        total_items, pending_items, options, resolved
                     ) VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?5, ?6, ?6, ?7, ?8)",
                    params![
                        request_id.as_str(),
                        Origin::Retry.as_str(),
                        job_id.0 as i64,
                        existing.kind,
                        now,
                        total_items_i64,
                        options_str,
                        resolved_str,
                    ],
                )?;
                let new_job_id_raw = c.last_insert_rowid();

                // 6. Insert only eligible items into the new job, preserving prior attempt_count
                // so claiming the item in the retry job advances attempt_count to prev + 1.
                {
                    let mut stmt = c.prepare(
                        "INSERT INTO job_items (
                            job_id, item_index, source_path, state, attempt_count
                         ) VALUES (?1, ?2, ?3, 'pending', ?4)",
                    )?;
                    for (new_idx, (src_bytes, prev_attempts)) in eligible_items.iter().enumerate() {
                        stmt.execute(params![
                            new_job_id_raw,
                            new_idx as i64,
                            src_bytes,
                            *prev_attempts,
                        ])?;
                    }
                }

                let new_summary = self
                    .get_job_summary_locked(c, JobId(new_job_id_raw as u64))?
                    .unwrap();
                Ok((new_summary, eligible_count, false))
            })();

            match res {
                Ok(v) => {
                    c.execute_batch("COMMIT;")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = c.execute_batch("ROLLBACK;");
                    Err(e)
                }
            }
        })
    }

    /// Marks an in-flight item `Interrupted` (e.g., when aborted cooperatively during `SIGTERM` drain)
    /// and updates parent job counts.
    pub fn complete_item_interrupted(
        &self,
        job_id: JobId,
        item_id: i64,
        duration_ms: u64,
        reason: &str,
    ) -> Result<(JobItemReport, JobSummary, bool), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        conn.execute(
            "UPDATE job_items
             SET state = 'interrupted',
                 stage = NULL,
                 output_path = NULL,
                 temp_path = NULL,
                 temp_dev = NULL,
                 temp_ino = NULL,
                 stage_dir_path = NULL,
                 stage_dev = NULL,
                 stage_ino = NULL,
                 finished_at_ms = ?2,
                 duration_ms = ?3,
                 error_code = 'interrupted',
                 error_class = 'interrupted',
                 error_message = ?4,
                 retryable = 1
             WHERE id = ?1",
            params![item_id, now, duration_ms as i64, reason],
        )?;
        self.recompute_and_update_job_locked(&conn, job_id, true)?;
        let item = conn.query_row(
            "SELECT id, item_index, source_path, output_path, state, stage, attempt_count,
                    source_size, output_bytes, duration_ms, error_code, error_class,
                    error_message, retryable, warnings
             FROM job_items WHERE id = ?1",
            params![item_id],
            Self::map_item_report_row,
        )?;
        let job = self.get_job_summary_locked(&conn, job_id)?.unwrap();
        let job_finished = job.state.is_terminal();
        Ok((item, job, job_finished))
    }

    /// Marks all currently in-flight items (`inspecting`, `running`, `committing`) as `interrupted`
    /// during clean `SIGTERM` shutdown (`AC-C2.7`).
    pub fn mark_inflight_items_interrupted_on_shutdown(
        &self,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            let now = now_unix_ms();
            let count = c.execute(
                "UPDATE job_items
                 SET state = 'interrupted',
                     stage = NULL,
                     finished_at_ms = COALESCE(finished_at_ms, ?1),
                     error_code = 'interrupted',
                     error_class = 'interrupted',
                     error_message = ?2,
                     retryable = 1
                 WHERE state IN ('inspecting', 'running', 'committing')",
                params![now, reason],
            )?;
            let mut stmt =
                c.prepare("SELECT id FROM jobs WHERE state IN ('queued', 'preparing', 'running')")?;
            let job_ids: Vec<i64> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for jid in job_ids {
                self.recompute_and_update_job_locked(c, JobId(jid as u64), true)?;
            }
            Ok(count)
        })
    }

    /// Loads all item rows requiring startup crash recovery inspection (`C0a`–`C5`):
    /// any item with `state IN ('inspecting', 'running', 'committing')` OR `stage_dir_path IS NOT NULL`
    /// OR `temp_path IS NOT NULL`.
    pub fn load_recovery_items(&self) -> Result<Vec<RecoveryItemRow>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, job_id, item_index, state, source_path, output_path,
                    temp_path, temp_dev, temp_ino, stage_dir_path, stage_dev, stage_ino, warnings
             FROM job_items
             WHERE state IN ('inspecting', 'running', 'committing')
                OR stage_dir_path IS NOT NULL
                OR temp_path IS NOT NULL
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let state_str: String = r.get(3)?;
            let src_bytes: Vec<u8> = r.get(4)?;
            let out_bytes: Option<Vec<u8>> = r.get(5)?;
            let temp_bytes: Option<Vec<u8>> = r.get(6)?;
            let temp_dev_i64: Option<i64> = r.get(7)?;
            let temp_ino_i64: Option<i64> = r.get(8)?;
            let stage_bytes: Option<Vec<u8>> = r.get(9)?;
            let stage_dev_i64: Option<i64> = r.get(10)?;
            let stage_ino_i64: Option<i64> = r.get(11)?;
            let warnings_json: String = r.get(12)?;

            Ok(RecoveryItemRow {
                item_id: r.get(0)?,
                job_id: JobId(r.get::<_, i64>(1)? as u64),
                item_index: r.get::<_, i64>(2)? as u32,
                state: state_str.parse::<ItemState>().unwrap_or(ItemState::Pending),
                source_path: PathBuf::from(OsStr::from_bytes(&src_bytes)),
                output_path: out_bytes.map(|b| PathBuf::from(OsStr::from_bytes(&b))),
                temp_path: temp_bytes.map(|b| PathBuf::from(OsStr::from_bytes(&b))),
                temp_dev: temp_dev_i64.map(|v| v as u64),
                temp_ino: temp_ino_i64.map(|v| v as u64),
                stage_dir_path: stage_bytes.map(|b| PathBuf::from(OsStr::from_bytes(&b))),
                stage_dev: stage_dev_i64.map(|v| v as u64),
                stage_ino: stage_ino_i64.map(|v| v as u64),
                warnings_json,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Commits recovery outcome for a single item under `PRAGMA synchronous = FULL`.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_recovery_item_update(
        &self,
        item_id: i64,
        new_state: ItemState,
        output_path: Option<&Path>,
        output_bytes: Option<u64>,
        error_code: Option<&str>,
        error_class: Option<&str>,
        error_message: Option<&str>,
        retryable: bool,
        warnings_json: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_unix_ms();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET state = ?2,
                     stage = NULL,
                     output_path = ?3,
                     temp_path = NULL,
                     temp_dev = NULL,
                     temp_ino = NULL,
                     stage_dir_path = NULL,
                     stage_dev = NULL,
                     stage_ino = NULL,
                     finished_at_ms = COALESCE(finished_at_ms, ?4),
                     output_bytes = COALESCE(?5, output_bytes),
                     error_code = COALESCE(?6, error_code),
                     error_class = COALESCE(?7, error_class),
                     error_message = COALESCE(?8, error_message),
                     retryable = ?9,
                     warnings = ?10
                 WHERE id = ?1",
                params![
                    item_id,
                    new_state.as_str(),
                    output_path.map(|p| p.as_os_str().as_bytes()),
                    now,
                    output_bytes.map(|b| b as i64),
                    error_code,
                    error_class,
                    error_message,
                    i64::from(retryable),
                    warnings_json,
                ],
            )?;
            Ok(())
        })
    }

    /// Clears `stage_dir_path`, `stage_dev`, `stage_ino`, `temp_path`, `temp_dev`, `temp_ino` on a
    /// terminal item (`C5` recovery) and updates `warnings` under `PRAGMA synchronous = FULL`.
    pub fn clear_terminal_item_staging_journal(
        &self,
        item_id: i64,
        warnings_json: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            c.execute(
                "UPDATE job_items
                 SET temp_path = NULL,
                     temp_dev = NULL,
                     temp_ino = NULL,
                     stage_dir_path = NULL,
                     stage_dev = NULL,
                     stage_ino = NULL,
                     warnings = ?2
                 WHERE id = ?1",
                params![item_id, warnings_json],
            )?;
            Ok(())
        })
    }

    /// Reconciles all active jobs (`queued`, `preparing`, `running`) during startup crash recovery (`C6`).
    pub fn reconcile_active_jobs_on_startup(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        self.with_sync_full(&conn, |c| {
            let mut stmt = c.prepare(
                "SELECT id FROM jobs WHERE state IN ('queued', 'preparing', 'running') ORDER BY id ASC",
            )?;
            let ids: Vec<i64> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for id in &ids {
                self.recompute_and_update_job_locked(c, JobId(*id as u64), true)?;
            }
            Ok(ids.len())
        })
    }

    /// Prunes terminal jobs older than `max_days` or exceeding `max_jobs` (`JOB_SYSTEM_DESIGN.md` §4.5).
    pub fn prune_retention(&self, max_jobs: usize, max_days: u32) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let cutoff_ms = now_unix_ms() - i64::from(max_days) * 86_400_000;
        let deleted_age = conn.execute(
            "DELETE FROM jobs
             WHERE state IN ('completed', 'completed_with_errors', 'failed', 'cancelled')
               AND COALESCE(finished_at_ms, updated_at_ms) < ?1",
            params![cutoff_ms],
        )?;

        let deleted_cap = conn.execute(
            "DELETE FROM jobs
             WHERE id IN (
                 SELECT id FROM jobs
                 WHERE state IN ('completed', 'completed_with_errors', 'failed', 'cancelled')
                 ORDER BY id DESC
                 LIMIT -1 OFFSET ?1
             )",
            params![max_jobs as i64],
        )?;

        Ok(deleted_age + deleted_cap)
    }

    /// Deletes terminal jobs (`completed`, `completed_with_errors`, `failed`, `cancelled`) matching
    /// optional `states` and `older_than_ms` filters without ever deleting active jobs or touching
    /// converted output files on disk (`AC-C4.3`).
    pub fn clear_terminal_history(
        &self,
        states: Option<&[JobState]>,
        older_than_ms: Option<i64>,
    ) -> Result<u32, StoreError> {
        let allowed_terminal: Vec<&'static str> = match states {
            Some(list) if !list.is_empty() => list
                .iter()
                .copied()
                .filter(|s| s.is_terminal())
                .map(JobState::as_str)
                .collect(),
            _ => vec!["completed", "completed_with_errors", "failed", "cancelled"],
        };
        if allowed_terminal.is_empty() {
            return Ok(0);
        }

        let quoted: Vec<String> = allowed_terminal.iter().map(|s| format!("'{s}'")).collect();
        let in_clause = quoted.join(", ");

        let conn = self.conn.lock().unwrap();
        let deleted = if let Some(cutoff) = older_than_ms {
            let sql = format!(
                "DELETE FROM jobs
                 WHERE state IN ({in_clause})
                   AND COALESCE(finished_at_ms, updated_at_ms) < ?1"
            );
            conn.execute(&sql, params![cutoff])?
        } else {
            let sql = format!("DELETE FROM jobs WHERE state IN ({in_clause})");
            conn.execute(&sql, [])?
        };
        Ok(deleted as u32)
    }

    /// Inspects terminal items with remaining staging journals (`pending_journals`) and any
    /// items with `OrphanedTempPreserved` warnings (`CONVERSION_UX.md` §4.7).
    pub fn inspect_staging_and_orphans(
        &self,
        job_filter: Option<JobId>,
    ) -> Result<(u32, Vec<OrphanedStageEntry>), StoreError> {
        let conn = self.conn.lock().unwrap();
        let pending_journals: i64 = if let Some(jid) = job_filter {
            conn.query_row(
                "SELECT COUNT(*) FROM job_items
                 WHERE job_id = ?1
                   AND state IN ('succeeded', 'skipped', 'failed', 'cancelled', 'interrupted')
                   AND (stage_dir_path IS NOT NULL OR temp_path IS NOT NULL)",
                params![jid.0 as i64],
                |r| r.get(0),
            )?
        } else {
            conn.query_row(
                "SELECT COUNT(*) FROM job_items
                 WHERE state IN ('succeeded', 'skipped', 'failed', 'cancelled', 'interrupted')
                   AND (stage_dir_path IS NOT NULL OR temp_path IS NOT NULL)",
                [],
                |r| r.get(0),
            )?
        };

        let sql = if job_filter.is_some() {
            "SELECT job_id, item_index, warnings FROM job_items WHERE job_id = ?1 AND warnings != '[]' ORDER BY job_id DESC, item_index ASC"
        } else {
            "SELECT job_id, item_index, warnings FROM job_items WHERE warnings != '[]' ORDER BY job_id DESC, item_index ASC"
        };

        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<(i64, i64, String)> = if let Some(jid) = job_filter {
            stmt.query_map(params![jid.0 as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };

        let mut orphans = Vec::new();
        for (jid_raw, item_idx, warnings_json) in rows {
            if let Ok(warnings) =
                serde_json::from_str::<Vec<radivine_convert::Warning>>(&warnings_json)
            {
                for w in warnings {
                    if let radivine_convert::Warning::OrphanedTempPreserved { stage_dir, reason } =
                        w
                    {
                        orphans.push(OrphanedStageEntry {
                            job_id: JobId(jid_raw as u64),
                            item_index: item_idx as u32,
                            stage_dir,
                            reason,
                        });
                    }
                }
            }
        }

        Ok((pending_journals as u32, orphans))
    }

    fn recompute_and_update_job_locked(
        &self,
        conn: &Connection,
        job_id: JobId,
        allow_terminal_transition: bool,
    ) -> Result<(), StoreError> {
        let counts = self.query_progress_counts_locked(conn, job_id)?;
        let cur_state_str: String = conn.query_row(
            "SELECT state FROM jobs WHERE id = ?1",
            params![job_id.0 as i64],
            |r| r.get(0),
        )?;
        let cur_state = cur_state_str
            .parse::<JobState>()
            .unwrap_or(JobState::Queued);

        let now = now_unix_ms();
        let all_terminal = counts.pending_items == 0 && counts.in_flight_items == 0;

        if allow_terminal_transition && all_terminal && cur_state.is_active() {
            let job_was_cancelled = counts.cancelled_items > 0;
            let term_state = compute_terminal_job_state(
                counts.total_items,
                counts.succeeded_items,
                counts.skipped_items,
                counts.failed_items,
                counts.cancelled_items,
                counts.interrupted_items,
                job_was_cancelled,
            );
            let summary = format_job_summary_text(term_state, &counts);
            conn.execute(
                "UPDATE jobs
                 SET state = ?2,
                     pending_items = 0,
                     succeeded_items = ?3,
                     skipped_items = ?4,
                     failed_items = ?5,
                     cancelled_items = ?6,
                     finished_at_ms = COALESCE(finished_at_ms, ?7),
                     updated_at_ms = ?7,
                     summary_text = ?8
                 WHERE id = ?1",
                params![
                    job_id.0 as i64,
                    term_state.as_str(),
                    i64::from(counts.succeeded_items),
                    i64::from(counts.skipped_items),
                    i64::from(counts.failed_items + counts.interrupted_items),
                    i64::from(counts.cancelled_items),
                    now,
                    summary,
                ],
            )?;
        } else {
            // If a job was in 'preparing' during crash recovery with pending items remaining,
            // return it to 'queued' so the scheduler prepares and runs it cleanly.
            let next_state = if cur_state == JobState::Preparing && counts.in_flight_items == 0 {
                JobState::Queued
            } else {
                cur_state
            };
            conn.execute(
                "UPDATE jobs
                 SET state = ?2,
                     pending_items = ?3,
                     succeeded_items = ?4,
                     skipped_items = ?5,
                     failed_items = ?6,
                     cancelled_items = ?7,
                     updated_at_ms = ?8
                 WHERE id = ?1",
                params![
                    job_id.0 as i64,
                    next_state.as_str(),
                    i64::from(counts.pending_items),
                    i64::from(counts.succeeded_items),
                    i64::from(counts.skipped_items),
                    i64::from(counts.failed_items + counts.interrupted_items),
                    i64::from(counts.cancelled_items),
                    now,
                ],
            )?;
        }
        Ok(())
    }

    fn query_progress_counts_locked(
        &self,
        conn: &Connection,
        job_id: JobId,
    ) -> Result<JobProgressCounts, StoreError> {
        let mut counts = JobProgressCounts::default();
        let mut stmt =
            conn.prepare("SELECT state, COUNT(*) FROM job_items WHERE job_id = ?1 GROUP BY state")?;
        let rows = stmt.query_map(params![job_id.0 as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        for r in rows {
            let (st_str, cnt) = r?;
            counts.total_items += cnt;
            match st_str.as_str() {
                "pending" => counts.pending_items += cnt,
                "inspecting" | "running" | "committing" => counts.in_flight_items += cnt,
                "succeeded" => counts.succeeded_items += cnt,
                "skipped" => counts.skipped_items += cnt,
                "failed" => counts.failed_items += cnt,
                "cancelled" => counts.cancelled_items += cnt,
                "interrupted" => counts.interrupted_items += cnt,
                _ => {}
            }
        }
        Ok(counts)
    }

    fn populate_extra_counts(conn: &Connection, job: &mut JobSummary) -> Result<(), StoreError> {
        let mut stmt =
            conn.prepare("SELECT state, COUNT(*) FROM job_items WHERE job_id = ?1 GROUP BY state")?;
        let rows = stmt.query_map(params![job.job_id.0 as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        let mut counts = JobProgressCounts {
            total_items: job.counts.total_items,
            ..JobProgressCounts::default()
        };
        let mut sum = 0u32;
        for r in rows {
            let (st_str, cnt) = r?;
            sum += cnt;
            match st_str.as_str() {
                "pending" => counts.pending_items += cnt,
                "inspecting" | "running" | "committing" => counts.in_flight_items += cnt,
                "succeeded" => counts.succeeded_items += cnt,
                "skipped" => counts.skipped_items += cnt,
                "failed" => counts.failed_items += cnt,
                "cancelled" => counts.cancelled_items += cnt,
                "interrupted" => counts.interrupted_items += cnt,
                _ => {}
            }
        }
        if sum > 0 {
            counts.total_items = sum;
        }
        job.counts = counts;
        Ok(())
    }

    fn map_job_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobSummary> {
        let id: i64 = row.get(0)?;
        let req_id: String = row.get(1)?;
        let origin_str: String = row.get(2)?;
        let retry_of: Option<i64> = row.get(3)?;
        let kind: String = row.get(4)?;
        let state_str: String = row.get(5)?;
        let created_at_ms: i64 = row.get(6)?;
        let started_at_ms: Option<i64> = row.get(7)?;
        let finished_at_ms: Option<i64> = row.get(8)?;
        let updated_at_ms: i64 = row.get(9)?;
        let total_items: i64 = row.get(10)?;
        let pending_items: i64 = row.get(11)?;
        let succeeded_items: i64 = row.get(12)?;
        let skipped_items: i64 = row.get(13)?;
        let failed_items: i64 = row.get(14)?;
        let cancelled_items: i64 = row.get(15)?;
        let options_str: String = row.get(16)?;
        let resolved_str: Option<String> = row.get(17)?;
        let summary_text: Option<String> = row.get(18)?;

        Ok(JobSummary {
            job_id: JobId(id as u64),
            request_id: RequestId(req_id),
            origin: origin_str.parse::<Origin>().unwrap_or(Origin::Cli),
            retry_of: retry_of.map(|r| JobId(r as u64)),
            kind,
            state: state_str.parse::<JobState>().unwrap_or(JobState::Queued),
            created_at_ms,
            started_at_ms,
            finished_at_ms,
            updated_at_ms,
            counts: JobProgressCounts {
                total_items: total_items as u32,
                pending_items: pending_items as u32,
                in_flight_items: 0,
                succeeded_items: succeeded_items as u32,
                skipped_items: skipped_items as u32,
                failed_items: failed_items as u32,
                cancelled_items: cancelled_items as u32,
                interrupted_items: 0,
            },
            summary_text,
            options: serde_json::from_str(&options_str).unwrap_or(serde_json::Value::Null),
            resolved: resolved_str.and_then(|s| serde_json::from_str(&s).ok()),
        })
    }

    fn map_item_report_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobItemReport> {
        let item_id: i64 = row.get(0)?;
        let item_index: i64 = row.get(1)?;
        let src_bytes: Vec<u8> = row.get(2)?;
        let out_bytes: Option<Vec<u8>> = row.get(3)?;
        let state_str: String = row.get(4)?;
        let stage: Option<String> = row.get(5)?;
        let attempt_count: i64 = row.get(6)?;
        let source_size: Option<i64> = row.get(7)?;
        let output_bytes: Option<i64> = row.get(8)?;
        let duration_ms: Option<i64> = row.get(9)?;
        let error_code: Option<String> = row.get(10)?;
        let error_class: Option<String> = row.get(11)?;
        let error_message: Option<String> = row.get(12)?;
        let retryable: i64 = row.get(13)?;
        let warnings_str: String = row.get(14)?;

        let source_path = WirePath::from_raw_bytes(&src_bytes)
            .unwrap_or_else(|_| WirePath::from_raw_bytes(b"/invalid").unwrap());
        let output_path = out_bytes.and_then(|b| WirePath::from_raw_bytes(&b).ok());
        let warnings: Vec<serde_json::Value> =
            serde_json::from_str(&warnings_str).unwrap_or_default();

        Ok(JobItemReport {
            item_id,
            item_index: item_index as u32,
            source_path,
            output_path,
            state: state_str.parse::<ItemState>().unwrap_or(ItemState::Pending),
            stage,
            attempt_count: attempt_count as u32,
            source_bytes: source_size.map(|v| v as u64),
            output_bytes: output_bytes.map(|v| v as u64),
            duration_ms: duration_ms.map(|v| v as u64),
            error_code,
            error_class,
            error_message,
            retryable: retryable != 0,
            warnings,
        })
    }
}

#[must_use]
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[must_use]
pub fn format_job_summary_text(state: JobState, counts: &JobProgressCounts) -> String {
    let failed_total = counts.failed_items + counts.interrupted_items;
    match state {
        JobState::Completed => {
            if counts.skipped_items > 0 {
                format!(
                    "Converted {} of {} files ({} skipped)",
                    counts.succeeded_items, counts.total_items, counts.skipped_items
                )
            } else {
                format!("Converted {} files", counts.succeeded_items)
            }
        }
        JobState::CompletedWithErrors => format!(
            "Converted {} of {} files ({} failed, {} skipped)",
            counts.succeeded_items, counts.total_items, failed_total, counts.skipped_items
        ),
        JobState::Failed => format!("Conversion failed ({failed_total} failed)"),
        JobState::Cancelled => format!(
            "Conversion cancelled ({} completed before cancel)",
            counts.succeeded_items
        ),
        other => format!("Job {}", other.as_str()),
    }
}

pub(crate) fn normalize_options_value(val: &serde_json::Value) -> serde_json::Value {
    let mut out = val.clone();
    if let Some(obj) = out.as_object_mut() {
        if let Some(output) = obj.get_mut("output").and_then(|v| v.as_object_mut())
            && let Some(loc) = output.get_mut("location").and_then(|v| v.as_object_mut())
            && !loc.contains_key("kind")
            && let Some(folder) = loc
                .remove("folder")
                .or_else(|| loc.remove("directory"))
                .or_else(|| loc.remove("path"))
        {
            loc.insert(
                "kind".to_string(),
                serde_json::Value::String("directory".to_string()),
            );
            loc.insert("value".to_string(), folder);
        }
        if let Some(loc) = obj
            .get_mut("output_location")
            .and_then(|v| v.as_object_mut())
            && !loc.contains_key("kind")
            && let Some(folder) = loc
                .remove("folder")
                .or_else(|| loc.remove("directory"))
                .or_else(|| loc.remove("path"))
        {
            loc.insert(
                "kind".to_string(),
                serde_json::Value::String("directory".to_string()),
            );
            loc.insert("value".to_string(), folder);
        }
    }
    out
}

fn merge_json_non_null(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(t_map), serde_json::Value::Object(p_map)) => {
            for (k, v) in p_map {
                if v.is_null() {
                    continue;
                }
                if (k == "location" || k == "output_location")
                    && v.as_object().is_some_and(|o| o.contains_key("kind"))
                {
                    t_map.insert(k.clone(), v.clone());
                    continue;
                }
                match t_map.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => {
                        merge_json_non_null(existing, v);
                    }
                    _ => {
                        t_map.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, p) if !p.is_null() => {
            *t = p.clone();
        }
        _ => {}
    }
}

fn apply_conversion_options_shorthands(
    resolved_obj: &mut serde_json::Value,
    norm_ov: &serde_json::Value,
) {
    let Some(ov_map) = norm_ov.as_object() else {
        return;
    };
    let Some(res_map) = resolved_obj.as_object_mut() else {
        return;
    };
    res_map.remove("overrides");
    res_map.remove("preset_ref");

    for src_map in [
        Some(ov_map),
        ov_map.get("overrides").and_then(|v| v.as_object()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(q) = src_map.get("quality").filter(|v| !v.is_null()) {
            res_map.insert("quality".to_string(), q.clone());
        }
        if let Some(r) = src_map.get("resize").filter(|v| !v.is_null()) {
            res_map.insert("resize".to_string(), r.clone());
        }
        if let Some(sf) = src_map.get("same_format").filter(|v| !v.is_null()) {
            res_map.insert("same_format".to_string(), sf.clone());
        }
        if let Some(loc) = src_map.get("output_location").filter(|v| !v.is_null())
            && let Some(out_map) = res_map.get_mut("output").and_then(|v| v.as_object_mut())
        {
            out_map.insert("location".to_string(), normalize_options_value(loc));
        }
        if let Some(col) = src_map.get("collision").filter(|v| !v.is_null())
            && let Some(out_map) = res_map.get_mut("output").and_then(|v| v.as_object_mut())
        {
            out_map.insert("collision".to_string(), col.clone());
        }
        if let Some(nam) = src_map.get("naming").filter(|v| !v.is_null())
            && let Some(out_map) = res_map.get_mut("output").and_then(|v| v.as_object_mut())
        {
            out_map.insert("naming".to_string(), nam.clone());
        }
        if let Some(pt) = src_map.get("preserve_timestamps").filter(|v| !v.is_null())
            && let Some(out_map) = res_map.get_mut("output").and_then(|v| v.as_object_mut())
        {
            out_map.insert("preserve_timestamps".to_string(), pt.clone());
        }
    }
}

fn merge_retry_options_override(
    base_options: &serde_json::Value,
    base_resolved: Option<&serde_json::Value>,
    ov: &serde_json::Value,
) -> (serde_json::Value, Option<serde_json::Value>) {
    let norm_ov = normalize_options_value(ov);
    let mut merged_opts = normalize_options_value(base_options);
    merge_json_non_null(&mut merged_opts, &norm_ov);
    if let Some(loc) = norm_ov
        .get("output")
        .and_then(|o| o.get("location"))
        .filter(|v| !v.is_null())
        && let Some(opts_map) = merged_opts.as_object_mut()
        && !opts_map.contains_key("alpha")
    {
        opts_map.insert("output_location".to_string(), loc.clone());
    }

    let merged_resolved = base_resolved.map(|res| {
        let mut r = normalize_options_value(res);
        merge_json_non_null(&mut r, &norm_ov);
        apply_conversion_options_shorthands(&mut r, &norm_ov);
        r
    });
    (merged_opts, merged_resolved)
}
