//! `radivine-jobs` — Generic durable background job contracts, lossless `WirePath` codec,
//! JSON-lines IPC protocol v1, `[jobs]` settings, and client/launcher for Radivine (`ADR-027`).
//!
//! Enforces `#![forbid(unsafe_code)]` and has zero internal Radivine crate dependencies (`AC-C2.17`).

#![forbid(unsafe_code)]

pub mod client;
pub mod protocol;
pub mod settings;
pub mod state;
pub mod wire_path;

pub use client::{
    ClientError, JobsClient, Subscription, WorkerLaunchMode, WorkerLauncher, WorkerRuntimePaths,
};
pub use protocol::{
    FrameReadError, JobItemReport, JobProgressCounts, JobSummary, JobUpdate, MAX_ACTIVE_JOBS,
    MAX_EVENT_LINE_BYTES, MAX_ITEMS_PER_JOB, MAX_NON_SUBMIT_LINE_BYTES, MAX_PENDING_ITEMS_TOTAL,
    MAX_RESPONSE_LINE_BYTES, MAX_SUBMIT_LINE_BYTES, PROTOCOL_VERSION, WorkerErrorCode,
    WorkerRequest, WorkerResponse, read_bounded_line, read_worker_request,
};
pub use settings::{JobsNotifyPolicy, JobsSettings, MaxParallelSetting, MemoryBudgetSetting};
pub use state::{
    CONVERT_IMAGE_KIND, InvalidStateTransition, ItemState, JobId, JobState, Origin, RequestId,
    RetryMode, compute_terminal_job_state,
};
pub use wire_path::{MAX_WIRE_PATH_BYTES, WirePath, WirePathError};
