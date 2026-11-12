//! The fixed game tick shared by the match worker, the script VM and replication.

use std::time::Duration;

pub const TICK_PERIOD: Duration = Duration::from_millis(16);
pub const FIRST_TICK: u32 = 1000;
pub const MAX_LEASE_FRAME: u32 = u32::MAX - 2 * (1 << 22);
