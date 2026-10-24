//! `radivine-worker` library surface for durable background job execution, SQLite persistence,
//! `C0a`–`C6` startup crash recovery, round-robin scheduling, and JSON-lines IPC (`ADR-027`, `ADR-032`).

#![forbid(unsafe_code)]

pub mod fault;
pub mod fence;
pub mod governor;
pub mod handler;
pub mod notify;
pub mod recovery;
pub mod scheduler;
pub mod server;
pub mod store;
