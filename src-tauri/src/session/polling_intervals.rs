use std::time::Duration;

pub const ACTIVATION_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub const SMOKE_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
pub const SMOKE_ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(15);
pub const SMOKE_EVALUATOR_FIRST_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub const STANDARD_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(480);
pub const STANDARD_ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(480);
pub const STANDARD_EVALUATOR_FIRST_POLL_INTERVAL: Duration = Duration::from_secs(1200);

/// Interval at which the frontend application-state store polls `/application-state/poll`
/// for watermark-based sync. Documented here for consistency; the frontend hardcodes the
/// matching 1000ms value (the Rust consts are not exported to TS).
#[allow(dead_code)]
pub const APPLICATION_STATE_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub fn format_poll_label(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs % 60 == 0 && secs >= 60 {
        let minutes = secs / 60;
        let unit = if minutes == 1 { "minute" } else { "minutes" };
        format!("{minutes} {unit}")
    } else {
        let unit = if secs == 1 { "second" } else { "seconds" };
        format!("{secs} {unit}")
    }
}
