//! Generic job and item state machines, identifiers, and transition validation (`JOB_SYSTEM_DESIGN.md` §3, `AC-C2.3`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical job kind identifier for built-in image conversion (`"convert.image"`).
pub const CONVERT_IMAGE_KIND: &str = "convert.image";

/// Monotonically increasing SQLite job identifier (`INTEGER PRIMARY KEY AUTOINCREMENT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Client-generated idempotency key for `submit` requests (`16..=64` ASCII `[0-9a-zA-Z_-]` chars).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

impl RequestId {
    /// Generates a fresh random 32-hex-character [`RequestId`].
    #[must_use]
    pub fn generate() -> Self {
        Self(format!(
            "{:016x}{:016x}",
            fastrand::u64(..),
            fastrand::u64(..)
        ))
    }

    /// Validates that the request ID is `16..=64` ASCII `[0-9a-zA-Z_-]` characters (`JOB_SYSTEM_DESIGN.md` §5.3).
    pub fn validate(&self) -> Result<(), String> {
        let len = self.0.len();
        if !(16..=64).contains(&len) {
            return Err(format!(
                "request_id length {len} must be between 16 and 64 characters"
            ));
        }
        if !self
            .0
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(
                "request_id may only contain ASCII alphanumeric, '_', or '-' characters"
                    .to_string(),
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Origin surface that submitted a job (`JOB_SYSTEM_DESIGN.md` §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Wheel,
    Cli,
    ControlCenter,
    Test,
}

impl Origin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wheel => "wheel",
            Self::Cli => "cli",
            Self::ControlCenter => "control_center",
            Self::Test => "test",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Origin {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "wheel" => Ok(Self::Wheel),
            "cli" => Ok(Self::Cli),
            "control_center" => Ok(Self::ControlCenter),
            "test" => Ok(Self::Test),
            other => Err(format!("unknown job origin '{other}'")),
        }
    }
}

/// Retry selection mode for [`crate::protocol::WorkerRequest::Retry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryMode {
    #[default]
    FailedOnly,
    FailedAndCancelled,
    All,
}

impl RetryMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FailedOnly => "failed_only",
            Self::FailedAndCancelled => "failed_and_cancelled",
            Self::All => "all",
        }
    }
}

/// Lifecycle state of a background job (`JOB_SYSTEM_DESIGN.md` §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Preparing,
    Running,
    Completed,
    CompletedWithErrors,
    Failed,
    Cancelled,
}

impl JobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Returns `true` if the job is currently active (`Queued`, `Preparing`, or `Running`).
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Preparing | Self::Running)
    }

    /// Returns `true` if the job is in a terminal state (`Completed`, `CompletedWithErrors`, `Failed`, `Cancelled`).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !self.is_active()
    }

    /// Returns `true` if normal forward execution permits transitioning from `self` to `next`.
    ///
    /// Terminal states (`Completed`, `CompletedWithErrors`, `Failed`, `Cancelled`) never transition
    /// during normal execution; reopening a job via `retry` uses [`Self::can_reopen_for_retry`].
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Queued,
                Self::Preparing | Self::Cancelled | Self::Failed
            ) | (
                Self::Preparing,
                Self::Running
                    | Self::Completed
                    | Self::CompletedWithErrors
                    | Self::Failed
                    | Self::Cancelled,
            ) | (
                Self::Running,
                Self::Completed | Self::CompletedWithErrors | Self::Failed | Self::Cancelled,
            )
        )
    }

    /// Validates a normal forward state transition, returning [`InvalidStateTransition`] if forbidden.
    pub const fn validate_transition(self, next: Self) -> Result<(), InvalidStateTransition> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(InvalidStateTransition::Job {
                from: self,
                to: next,
            })
        }
    }

    /// Returns `true` if a terminal job in state `self` can be reopened to `Queued` by a `retry` request.
    ///
    /// Note that `RetryMode::All` can reopen even `Completed` jobs, whereas `FailedOnly` and
    /// `FailedAndCancelled` reopen `CompletedWithErrors`, `Failed`, or `Cancelled`.
    #[must_use]
    pub const fn can_reopen_for_retry(self, mode: RetryMode) -> bool {
        match mode {
            RetryMode::All => self.is_terminal(),
            RetryMode::FailedOnly => {
                matches!(self, Self::CompletedWithErrors | Self::Failed)
            }
            RetryMode::FailedAndCancelled => matches!(
                self,
                Self::CompletedWithErrors | Self::Failed | Self::Cancelled
            ),
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(Self::Queued),
            "preparing" => Ok(Self::Preparing),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "completed_with_errors" => Ok(Self::CompletedWithErrors),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown job state '{other}'")),
        }
    }
}

/// Lifecycle state of an individual item within a job (`JOB_SYSTEM_DESIGN.md` §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Pending,
    Inspecting,
    Running,
    Committing,
    Succeeded,
    Skipped,
    Failed,
    Cancelled,
    Interrupted,
}

impl ItemState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Inspecting => "inspecting",
            Self::Running => "running",
            Self::Committing => "committing",
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    /// Returns `true` if this item state is in-flight (`Inspecting`, `Running`, or `Committing`).
    #[must_use]
    pub const fn is_in_flight(self) -> bool {
        matches!(self, Self::Inspecting | Self::Running | Self::Committing)
    }

    /// Returns `true` if this item state is terminal (`Succeeded`, `Skipped`, `Failed`, `Cancelled`, `Interrupted`).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Skipped | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Returns `true` if normal execution or crash recovery permits transitioning from `self` to `next`.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Pending,
                Self::Inspecting | Self::Skipped | Self::Cancelled | Self::Failed
            ) | (
                Self::Inspecting,
                Self::Running | Self::Skipped | Self::Failed | Self::Cancelled | Self::Interrupted,
            ) | (
                Self::Running,
                Self::Committing | Self::Failed | Self::Cancelled | Self::Interrupted,
            ) | (
                Self::Committing,
                Self::Succeeded
                    | Self::Skipped
                    | Self::Failed
                    | Self::Cancelled
                    | Self::Interrupted,
            ) | (Self::Interrupted, Self::Pending)
        )
    }

    /// Validates an item state transition, returning [`InvalidStateTransition`] if forbidden.
    pub const fn validate_transition(self, next: Self) -> Result<(), InvalidStateTransition> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(InvalidStateTransition::Item {
                from: self,
                to: next,
            })
        }
    }

    /// Returns `true` if an item in state `self` should be reset to `Pending` under `mode`.
    #[must_use]
    pub const fn should_reset_for_retry(self, mode: RetryMode) -> bool {
        match mode {
            RetryMode::FailedOnly => matches!(self, Self::Failed | Self::Interrupted),
            RetryMode::FailedAndCancelled => {
                matches!(self, Self::Failed | Self::Interrupted | Self::Cancelled)
            }
            RetryMode::All => self.is_terminal(),
        }
    }
}

impl fmt::Display for ItemState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ItemState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "inspecting" => Ok(Self::Inspecting),
            "running" => Ok(Self::Running),
            "committing" => Ok(Self::Committing),
            "succeeded" => Ok(Self::Succeeded),
            "skipped" => Ok(Self::Skipped),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            other => Err(format!("unknown item state '{other}'")),
        }
    }
}

/// Computes the terminal [`JobState`] from final item outcome counts (`JOB_SYSTEM_DESIGN.md` §3.1).
#[must_use]
pub const fn compute_terminal_job_state(
    total_items: u32,
    succeeded_items: u32,
    skipped_items: u32,
    failed_items: u32,
    cancelled_items: u32,
    interrupted_items: u32,
    job_was_cancelled: bool,
) -> JobState {
    let bad_items = failed_items + interrupted_items;
    if job_was_cancelled || cancelled_items > 0 {
        return JobState::Cancelled;
    }
    if bad_items == 0 {
        return JobState::Completed;
    }
    if succeeded_items > 0 || skipped_items > 0 {
        return JobState::CompletedWithErrors;
    }
    let _ = total_items;
    JobState::Failed
}

/// Error returned when an illegal [`JobState`] or [`ItemState`] transition is attempted.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InvalidStateTransition {
    #[error("invalid job state transition from {from} to {to}")]
    Job { from: JobState, to: JobState },
    #[error("invalid item state transition from {from} to {to}")]
    Item { from: ItemState, to: ItemState },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AC-C2.3`: Table-driven unit test suite covering `>= 40` valid and invalid job and item
    /// state transitions (`JOB_SYSTEM_DESIGN.md` §3.1–§3.2).
    #[test]
    fn ac_c2_3_table_driven_state_transitions_at_least_40_cases() {
        let job_cases: &[(JobState, JobState, bool)] = &[
            // Valid from Queued
            (JobState::Queued, JobState::Preparing, true),
            (JobState::Queued, JobState::Cancelled, true),
            (JobState::Queued, JobState::Failed, true),
            // Invalid from Queued
            (JobState::Queued, JobState::Queued, false),
            (JobState::Queued, JobState::Running, false),
            (JobState::Queued, JobState::Completed, false),
            (JobState::Queued, JobState::CompletedWithErrors, false),
            // Valid from Preparing
            (JobState::Preparing, JobState::Running, true),
            (JobState::Preparing, JobState::Completed, true),
            (JobState::Preparing, JobState::CompletedWithErrors, true),
            (JobState::Preparing, JobState::Failed, true),
            (JobState::Preparing, JobState::Cancelled, true),
            // Invalid from Preparing
            (JobState::Preparing, JobState::Queued, false),
            (JobState::Preparing, JobState::Preparing, false),
            // Valid from Running
            (JobState::Running, JobState::Completed, true),
            (JobState::Running, JobState::CompletedWithErrors, true),
            (JobState::Running, JobState::Failed, true),
            (JobState::Running, JobState::Cancelled, true),
            // Invalid from Running
            (JobState::Running, JobState::Queued, false),
            (JobState::Running, JobState::Preparing, false),
            (JobState::Running, JobState::Running, false),
            // Invalid from Terminal states
            (JobState::Completed, JobState::Running, false),
            (JobState::Completed, JobState::Queued, false),
            (JobState::Completed, JobState::Failed, false),
            (JobState::CompletedWithErrors, JobState::Running, false),
            (JobState::CompletedWithErrors, JobState::Completed, false),
            (JobState::Failed, JobState::Running, false),
            (JobState::Failed, JobState::Completed, false),
            (JobState::Cancelled, JobState::Running, false),
            (JobState::Cancelled, JobState::Completed, false),
        ];

        let item_cases: &[(ItemState, ItemState, bool)] = &[
            // Valid from Pending
            (ItemState::Pending, ItemState::Inspecting, true),
            (ItemState::Pending, ItemState::Skipped, true),
            (ItemState::Pending, ItemState::Cancelled, true),
            (ItemState::Pending, ItemState::Failed, true),
            // Invalid from Pending
            (ItemState::Pending, ItemState::Running, false),
            (ItemState::Pending, ItemState::Committing, false),
            (ItemState::Pending, ItemState::Succeeded, false),
            (ItemState::Pending, ItemState::Interrupted, false),
            // Valid from Inspecting
            (ItemState::Inspecting, ItemState::Running, true),
            (ItemState::Inspecting, ItemState::Skipped, true),
            (ItemState::Inspecting, ItemState::Failed, true),
            (ItemState::Inspecting, ItemState::Cancelled, true),
            (ItemState::Inspecting, ItemState::Interrupted, true),
            // Invalid from Inspecting
            (ItemState::Inspecting, ItemState::Committing, false),
            (ItemState::Inspecting, ItemState::Succeeded, false),
            // Valid from Running
            (ItemState::Running, ItemState::Committing, true),
            (ItemState::Running, ItemState::Failed, true),
            (ItemState::Running, ItemState::Cancelled, true),
            (ItemState::Running, ItemState::Interrupted, true),
            // Invalid from Running
            (ItemState::Running, ItemState::Succeeded, false),
            (ItemState::Running, ItemState::Pending, false),
            // Valid from Committing
            (ItemState::Committing, ItemState::Succeeded, true),
            (ItemState::Committing, ItemState::Skipped, true),
            (ItemState::Committing, ItemState::Failed, true),
            (ItemState::Committing, ItemState::Cancelled, true),
            (ItemState::Committing, ItemState::Interrupted, true),
            // Valid from Interrupted
            (ItemState::Interrupted, ItemState::Pending, true),
            // Invalid from Terminal item states
            (ItemState::Succeeded, ItemState::Running, false),
            (ItemState::Succeeded, ItemState::Pending, false),
            (ItemState::Succeeded, ItemState::Failed, false),
            (ItemState::Cancelled, ItemState::Succeeded, false),
            (ItemState::Cancelled, ItemState::Running, false),
            (ItemState::Failed, ItemState::Succeeded, false),
            (ItemState::Skipped, ItemState::Running, false),
            (ItemState::Interrupted, ItemState::Succeeded, false),
        ];

        assert!(
            job_cases.len() + item_cases.len() >= 40,
            "must test at least 40 transitions (got {})",
            job_cases.len() + item_cases.len()
        );

        for &(from, to, expected) in job_cases {
            assert_eq!(
                from.can_transition_to(to),
                expected,
                "JobState transition {from} -> {to} expected {expected}"
            );
            assert_eq!(from.validate_transition(to).is_ok(), expected);
        }

        for &(from, to, expected) in item_cases {
            assert_eq!(
                from.can_transition_to(to),
                expected,
                "ItemState transition {from} -> {to} expected {expected}"
            );
            assert_eq!(from.validate_transition(to).is_ok(), expected);
        }
    }
}
