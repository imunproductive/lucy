//! The fixed game tick shared by the match worker, the script VM and replication.

use std::time::Duration;

pub const TICK_PERIOD: Duration = Duration::from_millis(16);
/// World frames are sent every third tick (Python's cadence, about 20.8 a
/// second); the server's `ORPHEUS_SEND_EVERY` switch can send them more often.
pub const SEND_EVERY: u32 = 3;
pub const FIRST_TICK: u32 = 1000;
pub const MAX_LEASE_FRAME: u32 = u32::MAX - 2 * (1 << 22);
