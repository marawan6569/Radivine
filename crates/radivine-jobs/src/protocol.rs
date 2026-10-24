//! JSON-lines IPC Protocol v1 (`JOB_SYSTEM_DESIGN.md` §5, `SECURITY_AND_FILE_SAFETY.md` §7.2).

use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::state::{ItemState, JobId, JobState, Origin, RequestId, RetryMode};
use crate::wire_path::WirePath;

/// Current IPC wire protocol version (`1`).
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum byte length of a `submit` request line (`16 MiB`, `SECURITY_AND_FILE_SAFETY.md` §7.2).
pub const MAX_SUBMIT_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Maximum byte length of any non-`submit` request line (`64 KiB`, `SECURITY_AND_FILE_SAFETY.md` §7.2).
pub const MAX_NON_SUBMIT_LINE_BYTES: usize = 64 * 1024;

/// Maximum byte length of a synchronous response line (`4 MiB`).
pub const MAX_RESPONSE_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum byte length of a pushed subscription event line (`64 KiB`).
pub const MAX_EVENT_LINE_BYTES: usize = 64 * 1024;

/// Maximum number of items allowed in a single `submit` request (`10,000`).
pub const MAX_ITEMS_PER_JOB: usize = 10_000;

/// Maximum number of concurrently active jobs (`256`).
pub const MAX_ACTIVE_JOBS: usize = 256;

/// Maximum total pending/in-flight items across all active jobs (`50,000`).
pub const MAX_PENDING_ITEMS_TOTAL: usize = 50_000;

const fn default_protocol_v() -> u32 {
    PROTOCOL_VERSION
}

/// Machine-readable error codes returned in [`WorkerResponse::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    UnsupportedProtocolVersion,
    MalformedRequest,
    MessageTooLarge,
    InvalidRequestId,
    TooManyItems,
    InvalidPath,
    UnknownJobKind,
    InvalidOptions,
    QueueFull,
    JobNotFound,
    InvalidJobState,
    DatabaseBusy,
    InternalError,
}

/// Aggregated item counts for a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JobProgressCounts {
    pub total_items: u32,
    pub pending_items: u32,
    pub in_flight_items: u32,
    pub succeeded_items: u32,
    pub skipped_items: u32,
    pub failed_items: u32,
    pub cancelled_items: u32,
    pub interrupted_items: u32,
}

impl JobProgressCounts {
    /// Returns the number of items that have reached a terminal state.
    #[must_use]
    pub const fn completed_items(&self) -> u32 {
        self.succeeded_items
            + self.skipped_items
            + self.failed_items
            + self.cancelled_items
            + self.interrupted_items
    }
}

/// High-level summary of a job returned by `list_jobs`, `get_job`, and subscription events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub job_id: JobId,
    pub request_id: RequestId,
    pub origin: Origin,
    #[serde(default)]
    pub retry_of: Option<JobId>,
    pub kind: String,
    pub state: JobState,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub counts: JobProgressCounts,
    pub summary_text: Option<String>,
    pub options: serde_json::Value,
    pub resolved: Option<serde_json::Value>,
}

/// Detailed report for a single item within a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobItemReport {
    pub item_id: i64,
    pub item_index: u32,
    pub source_path: WirePath,
    pub output_path: Option<WirePath>,
    pub state: ItemState,
    pub stage: Option<String>,
    pub attempt_count: u32,
    pub source_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
    pub duration_ms: Option<u64>,
    pub error_code: Option<String>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
    pub retryable: bool,
    pub warnings: Vec<serde_json::Value>,
}

/// Preserved orphaned staging entry reported by `inspect_staging` / `cleanup_staging` (`CONVERSION_UX.md` §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanedStageEntry {
    pub job_id: JobId,
    pub item_index: u32,
    pub stage_dir: String,
    pub reason: String,
}

/// Client-to-worker request frame (`JOB_SYSTEM_DESIGN.md` §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerRequest {
    Submit {
        #[serde(default = "default_protocol_v")]
        v: u32,
        request_id: RequestId,
        origin: Origin,
        kind: String,
        items: Vec<WirePath>,
        #[serde(default)]
        options: serde_json::Value,
    },
    Cancel {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
    },
    Retry {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default)]
        request_id: Option<RequestId>,
        job_id: JobId,
        #[serde(default, alias = "scope")]
        mode: RetryMode,
        #[serde(default)]
        options_override: Option<serde_json::Value>,
    },
    GetJob {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        #[serde(default)]
        item_offset: Option<u32>,
        #[serde(default)]
        item_limit: Option<u32>,
    },
    ListJobs {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default = "default_list_limit")]
        limit: u32,
        #[serde(default = "default_true")]
        include_terminal: bool,
    },
    ClearHistory {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default)]
        states: Option<Vec<JobState>>,
        #[serde(default)]
        older_than_ms: Option<i64>,
    },
    ReloadSettings {
        #[serde(default = "default_protocol_v")]
        v: u32,
    },
    InspectStaging {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default)]
        job_id: Option<JobId>,
    },
    CleanupStaging {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default)]
        job_id: Option<JobId>,
    },
    Subscribe {
        #[serde(default = "default_protocol_v")]
        v: u32,
        #[serde(default)]
        job_id: Option<JobId>,
    },
    Ping {
        #[serde(default = "default_protocol_v")]
        v: u32,
    },
    ShutdownIfIdle {
        #[serde(default = "default_protocol_v")]
        v: u32,
    },
}

const fn default_list_limit() -> u32 {
    50
}

const fn default_true() -> bool {
    true
}

impl WorkerRequest {
    /// Returns the protocol version field `v` of this request.
    #[must_use]
    pub const fn version(&self) -> u32 {
        match self {
            Self::Submit { v, .. }
            | Self::Cancel { v, .. }
            | Self::Retry { v, .. }
            | Self::GetJob { v, .. }
            | Self::ListJobs { v, .. }
            | Self::ClearHistory { v, .. }
            | Self::ReloadSettings { v }
            | Self::InspectStaging { v, .. }
            | Self::CleanupStaging { v, .. }
            | Self::Subscribe { v, .. }
            | Self::Ping { v }
            | Self::ShutdownIfIdle { v } => *v,
        }
    }
}

/// Worker-to-client synchronous response frame (`JOB_SYSTEM_DESIGN.md` §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerResponse {
    Submitted {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        state: JobState,
        deduplicated: bool,
    },
    JobDetail {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job: Box<JobSummary>,
        items: Vec<JobItemReport>,
        total_items: u32,
    },
    JobList {
        #[serde(default = "default_protocol_v")]
        v: u32,
        jobs: Vec<JobSummary>,
    },
    Cancelled {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        state: JobState,
    },
    Retried {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        #[serde(default)]
        retry_of: Option<JobId>,
        state: JobState,
        #[serde(alias = "queued_items")]
        reset_items: u32,
        #[serde(default)]
        deduplicated: bool,
    },
    HistoryCleared {
        #[serde(default = "default_protocol_v")]
        v: u32,
        deleted_jobs: u32,
    },
    SettingsReloaded {
        #[serde(default = "default_protocol_v")]
        v: u32,
        max_parallel: usize,
        memory_budget_mb: u64,
    },
    StagingReport {
        #[serde(default = "default_protocol_v")]
        v: u32,
        cleaned_files: u32,
        cleaned_dirs: u32,
        #[serde(default)]
        pending_journals: u32,
        #[serde(default)]
        preserved_orphans: Vec<OrphanedStageEntry>,
    },
    Subscribed {
        #[serde(default = "default_protocol_v")]
        v: u32,
        active_jobs: Vec<JobSummary>,
    },
    Pong {
        #[serde(default = "default_protocol_v")]
        v: u32,
        pid: u32,
        version: String,
        active_jobs: u32,
        queued_items: u32,
        schema_version: u32,
    },
    ShutdownAck {
        #[serde(default = "default_protocol_v")]
        v: u32,
        shutting_down: bool,
        active_jobs: u32,
    },
    Error {
        #[serde(default = "default_protocol_v")]
        v: u32,
        code: WorkerErrorCode,
        message: String,
    },
}

/// Asynchronous progress event pushed on subscribed connections (`JOB_SYSTEM_DESIGN.md` §5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobUpdate {
    JobCreated {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job: Box<JobSummary>,
    },
    JobStateChanged {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        state: JobState,
        summary: JobProgressCounts,
    },
    ItemProgress {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        item_index: u32,
        state: ItemState,
        stage: Option<String>,
        summary: JobProgressCounts,
    },
    ItemCompleted {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job_id: JobId,
        item: Box<JobItemReport>,
        summary: JobProgressCounts,
    },
    JobFinished {
        #[serde(default = "default_protocol_v")]
        v: u32,
        job: Box<JobSummary>,
    },
}

/// Errors returned while reading or parsing a bounded JSON-line frame.
#[derive(Debug, Error)]
pub enum FrameReadError {
    #[error("EOF before frame newline")]
    Eof,
    #[error("frame exceeds maximum allowed size of {max_bytes} bytes")]
    MessageTooLarge { max_bytes: usize },
    #[error("I/O error reading frame: {0}")]
    Io(#[from] io::Error),
    #[error("invalid UTF-8 in frame: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("malformed JSON frame: {0}")]
    MalformedJson(#[from] serde_json::Error),
}

/// Reads a single `\n`-delimited line from `reader` with a hard pre-allocation byte cap `max_bytes`.
///
/// If more than `max_bytes` are read before encountering `\n`, stops immediately without allocating
/// unbounded memory and returns [`FrameReadError::MessageTooLarge`].
pub fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<String, FrameReadError> {
    let mut buf = Vec::with_capacity(max_bytes.min(4096));
    loop {
        let available = match reader.fill_buf() {
            Ok([]) => {
                if buf.is_empty() {
                    return Err(FrameReadError::Eof);
                }
                break;
            }
            Ok(slice) => slice,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameReadError::Io(e)),
        };

        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let to_take = newline_pos + 1;
            if buf.len() + to_take > max_bytes {
                reader.consume(to_take);
                return Err(FrameReadError::MessageTooLarge { max_bytes });
            }
            buf.extend_from_slice(&available[..newline_pos]);
            reader.consume(to_take);
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(String::from_utf8(buf)?);
        } else {
            let chunk_len = available.len();
            if buf.len() + chunk_len > max_bytes {
                reader.consume(chunk_len);
                return Err(FrameReadError::MessageTooLarge { max_bytes });
            }
            buf.extend_from_slice(available);
            reader.consume(chunk_len);
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(String::from_utf8(buf)?)
}

/// Reads and parses a client [`WorkerRequest`] line enforcing the two-tier size cap:
/// - `64 KiB` (`MAX_NON_SUBMIT_LINE_BYTES`) for all non-`submit` requests
/// - `16 MiB` (`MAX_SUBMIT_LINE_BYTES`) for `submit` requests
pub fn read_worker_request<R: BufRead>(reader: &mut R) -> Result<WorkerRequest, FrameReadError> {
    let mut buf = Vec::with_capacity(4096);
    let mut current_cap = MAX_NON_SUBMIT_LINE_BYTES;
    let mut checked_submit_upgrade = false;

    loop {
        let available = match reader.fill_buf() {
            Ok([]) => {
                if buf.is_empty() {
                    return Err(FrameReadError::Eof);
                }
                break;
            }
            Ok(slice) => slice,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameReadError::Io(e)),
        };

        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let to_take = newline_pos + 1;
            if buf.len() + to_take > current_cap && !checked_submit_upgrade {
                let mut prefix = Vec::with_capacity(512);
                let from_buf = buf.len().min(512);
                prefix.extend_from_slice(&buf[..from_buf]);
                if prefix.len() < 512 {
                    let rem = (512 - prefix.len()).min(newline_pos);
                    prefix.extend_from_slice(&available[..rem]);
                }
                if looks_like_submit_prefix(&prefix) {
                    current_cap = MAX_SUBMIT_LINE_BYTES;
                }
            }
            if buf.len() + to_take > current_cap {
                reader.consume(to_take);
                return Err(FrameReadError::MessageTooLarge {
                    max_bytes: current_cap,
                });
            }
            buf.extend_from_slice(&available[..newline_pos]);
            reader.consume(to_take);
            break;
        } else {
            let chunk_len = available.len();
            if buf.len() + chunk_len > current_cap && !checked_submit_upgrade {
                checked_submit_upgrade = true;
                let mut prefix = Vec::with_capacity(512);
                let from_buf = buf.len().min(512);
                prefix.extend_from_slice(&buf[..from_buf]);
                if prefix.len() < 512 {
                    let rem = (512 - prefix.len()).min(chunk_len);
                    prefix.extend_from_slice(&available[..rem]);
                }
                if looks_like_submit_prefix(&prefix) {
                    current_cap = MAX_SUBMIT_LINE_BYTES;
                }
            }
            if buf.len() + chunk_len > current_cap {
                reader.consume(chunk_len);
                return Err(FrameReadError::MessageTooLarge {
                    max_bytes: current_cap,
                });
            }
            buf.extend_from_slice(available);
            reader.consume(chunk_len);
        }
    }

    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    let line_len = buf.len();
    let s = String::from_utf8(buf)?;
    let req: WorkerRequest = serde_json::from_str(&s)?;

    // Post-parse enforcement: even if a non-submit payload contained the substring "submit",
    // reject any non-Submit request whose wire length exceeded MAX_NON_SUBMIT_LINE_BYTES.
    if !matches!(req, WorkerRequest::Submit { .. }) && line_len > MAX_NON_SUBMIT_LINE_BYTES {
        return Err(FrameReadError::MessageTooLarge {
            max_bytes: MAX_NON_SUBMIT_LINE_BYTES,
        });
    }
    Ok(req)
}

fn looks_like_submit_prefix(prefix: &[u8]) -> bool {
    std::str::from_utf8(prefix)
        .ok()
        .is_some_and(|s| s.contains("\"submit\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::Path;

    #[test]
    fn request_response_and_event_roundtrip() {
        let req = WorkerRequest::Submit {
            v: PROTOCOL_VERSION,
            request_id: RequestId::generate(),
            origin: Origin::Cli,
            kind: "convert.image".to_string(),
            items: vec![WirePath::from_absolute_path(Path::new("/tmp/a.png")).unwrap()],
            options: serde_json::json!({"target": "webp"}),
        };
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        let mut cursor = Cursor::new(line.into_bytes());
        let parsed = read_worker_request(&mut cursor).unwrap();
        assert_eq!(parsed, req);

        let resp = WorkerResponse::Submitted {
            v: PROTOCOL_VERSION,
            job_id: JobId(42),
            state: JobState::Queued,
            deduplicated: false,
        };
        let resp_json = serde_json::to_string(&resp).unwrap();
        let parsed_resp: WorkerResponse = serde_json::from_str(&resp_json).unwrap();
        assert_eq!(parsed_resp, resp);
    }

    #[test]
    fn non_submit_line_over_64kib_rejected_immediately() {
        let huge = format!(
            "{{\"v\":1,\"type\":\"ping\",\"pad\":\"{}\"}}\n",
            "x".repeat(MAX_NON_SUBMIT_LINE_BYTES + 1024)
        );
        let mut cursor = Cursor::new(huge.into_bytes());
        let err = read_worker_request(&mut cursor).unwrap_err();
        assert!(matches!(
            err,
            FrameReadError::MessageTooLarge {
                max_bytes: MAX_NON_SUBMIT_LINE_BYTES
            }
        ));
    }
}
