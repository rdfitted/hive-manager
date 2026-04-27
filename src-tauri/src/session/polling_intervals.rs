use std::time::Duration;

pub const ACTIVATION_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub const SMOKE_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
pub const SMOKE_ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(15);
pub const SMOKE_EVALUATOR_FIRST_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub const STANDARD_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(480);
pub const STANDARD_ACTIVE_POLL_INTERVAL: Duration = Duration::from_secs(480);
pub const STANDARD_EVALUATOR_FIRST_POLL_INTERVAL: Duration = Duration::from_secs(1200);

pub const SMOKE_IDLE_POLL_LABEL: &str = "30 seconds";
pub const SMOKE_ACTIVE_POLL_LABEL: &str = "15 seconds";
pub const SMOKE_EVALUATOR_FIRST_POLL_LABEL: &str = "30 seconds";

pub const STANDARD_IDLE_POLL_LABEL: &str = "8 minutes";
pub const STANDARD_ACTIVE_POLL_LABEL: &str = "8 minutes";
pub const STANDARD_EVALUATOR_FIRST_POLL_LABEL: &str = "20 minutes";
