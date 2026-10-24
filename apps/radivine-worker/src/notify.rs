//! Desktop completion notifications via `radivine-dbus` (`JOB_SYSTEM_DESIGN.md` §8, `ADR-027`).

use std::env;

use radivine_dbus::call_desktop_notify;
use radivine_jobs::{JobState, JobSummary, JobsNotifyPolicy, JobsSettings};
use tracing::debug;

/// Decides whether a terminal job should trigger a desktop notification according to `settings`
/// and environment variables (`JOB_SYSTEM_DESIGN.md` §8.2).
#[must_use]
pub fn should_notify_for_job(job: &JobSummary, settings: &JobsSettings) -> bool {
    if env::var("RADIVINE_SUPPRESS_DESKTOP_NOTIFY")
        .ok()
        .is_some_and(|v| v.trim() == "1" || v.eq_ignore_ascii_case("true"))
    {
        return false;
    }
    if !settings.global_notifications_enabled {
        return false;
    }
    match settings.notify {
        JobsNotifyPolicy::Off => false,
        JobsNotifyPolicy::ErrorsOnly => {
            matches!(job.state, JobState::CompletedWithErrors | JobState::Failed)
        }
        JobsNotifyPolicy::All => matches!(
            job.state,
            JobState::Completed | JobState::CompletedWithErrors | JobState::Failed
        ),
    }
}

/// Formats `(summary, body)` for a completed job notification (`JOB_SYSTEM_DESIGN.md` §8.2).
#[must_use]
pub fn format_notification_message(job: &JobSummary) -> (String, String) {
    let c = &job.counts;
    let failed_total = c.failed_items + c.interrupted_items;
    match job.state {
        JobState::Completed => {
            let title = if c.total_items == 1 {
                "Conversion complete".to_string()
            } else {
                format!("Converted {} files", c.succeeded_items)
            };
            let body = job.summary_text.clone().unwrap_or_else(|| {
                format!("Converted {} of {} files", c.succeeded_items, c.total_items)
            });
            (title, body)
        }
        JobState::CompletedWithErrors => (
            format!(
                "Converted {} of {} files ({failed_total} failed)",
                c.succeeded_items, c.total_items
            ),
            job.summary_text
                .clone()
                .unwrap_or_else(|| "Some items could not be converted.".to_string()),
        ),
        JobState::Failed => (
            "Conversion failed".to_string(),
            job.summary_text
                .clone()
                .unwrap_or_else(|| format!("{failed_total} file(s) failed to convert.")),
        ),
        _ => (
            "Conversion update".to_string(),
            job.summary_text.clone().unwrap_or_default(),
        ),
    }
}

/// Dispatches a non-blocking desktop notification for a terminal job if policy permits.
pub fn notify_job_completed(job: &JobSummary) {
    let (settings, _) = JobsSettings::load_lenient(&JobsSettings::default_config_path());
    if !should_notify_for_job(job, &settings) {
        return;
    }
    let (title, body) = format_notification_message(job);
    let _ = std::thread::Builder::new()
        .name("radivine-worker-notify".to_string())
        .spawn(move || {
            if let Err(e) = call_desktop_notify(None, "Radivine", "radivine", &title, &body, 5000) {
                debug!(error = %e, "desktop notification skipped or failed");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use radivine_jobs::{JobId, JobProgressCounts, Origin, RequestId};

    fn dummy_job(state: JobState) -> JobSummary {
        JobSummary {
            job_id: JobId(1),
            request_id: RequestId("req_0123456789abcdef".to_string()),
            origin: Origin::Cli,
            kind: "convert.image".to_string(),
            state,
            created_at_ms: 0,
            started_at_ms: Some(0),
            finished_at_ms: Some(10),
            updated_at_ms: 10,
            counts: JobProgressCounts {
                total_items: 2,
                pending_items: 0,
                in_flight_items: 0,
                succeeded_items: 1,
                skipped_items: 0,
                failed_items: 1,
                cancelled_items: 0,
                interrupted_items: 0,
            },
            summary_text: Some("Converted 1 of 2 files".to_string()),
            options: serde_json::Value::Null,
            resolved: None,
        }
    }

    #[test]
    fn notification_policy_gating() {
        let mut settings = JobsSettings::default();
        assert!(should_notify_for_job(
            &dummy_job(JobState::Completed),
            &settings
        ));
        assert!(should_notify_for_job(
            &dummy_job(JobState::CompletedWithErrors),
            &settings
        ));
        assert!(should_notify_for_job(
            &dummy_job(JobState::Failed),
            &settings
        ));
        assert!(!should_notify_for_job(
            &dummy_job(JobState::Cancelled),
            &settings
        ));

        settings.notify = JobsNotifyPolicy::ErrorsOnly;
        assert!(!should_notify_for_job(
            &dummy_job(JobState::Completed),
            &settings
        ));
        assert!(should_notify_for_job(
            &dummy_job(JobState::CompletedWithErrors),
            &settings
        ));
        assert!(should_notify_for_job(
            &dummy_job(JobState::Failed),
            &settings
        ));

        settings.notify = JobsNotifyPolicy::Off;
        assert!(!should_notify_for_job(
            &dummy_job(JobState::Failed),
            &settings
        ));

        settings.notify = JobsNotifyPolicy::All;
        settings.global_notifications_enabled = false;
        assert!(!should_notify_for_job(
            &dummy_job(JobState::Completed),
            &settings
        ));
    }
}
