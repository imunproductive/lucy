//! The announcer's multikill lines ("Double kill" to "Six kills").
//!
//! Neither Python nor upstream has them. Research (104319 data):
//!
//! - The PvP controller graph `0C90` runs graph `0215` under its st116
//!   (a BooleanSwitch on the owner) as st117 (SubScript). No other graph
//!   names `0215`, and no other graph names its five voice stimuli.
//! - The Practice Range: in retail the announcer says these lines there,
//!   for kills of Training Bots too (seen in the game). The map (`0688`) has
//!   two modes. In `0018` (the solo Range, which Python ran) the player's
//!   controller is `13C1`, and nothing in that mode runs `0215`: not 13C1 or
//!   the graphs it starts (`20E4`, `288A`), not the mode script `13C0`, the
//!   team body script `0C8E`, the hero's body graphs or the bot graphs. In
//!   `005A` (the group Range, six players, ruleset `0034.0C0`) both teams
//!   have 0C90, so there the player's 0C90 runs `0215`. The server runs the
//!   Range as `005A`, so the lines come as in the PvP modes.
//! - `0215` (server-only part): every elimination the controller gets
//!   (`0093.025`: killer `0012`, victim `0015`) of an enemy hero
//!   (rid15: filter with target tags `06AD` or `031D`) adds 1 to its count
//!   #2023 (rid17) and re-arms the wait st4 of #2036 = 2 s (`m_0DBAFD7F`:
//!   re-armed on a re-entry). When that wait ends, 2 s after the last kill,
//!   the count goes back to 0 (rid44). On each kill, unless #1954 or #21925
//!   (rid54), `ActionSwitch` rid30 on the count sets #10122 to the voice
//!   stimulus of the count (2 `058A`, 3 `058B`, 4 `058C`, 5 `058D`, 6
//!   `058E`; no other count has one) and enters the `ClientOnlyPulser` st21.
//! - The client: the pulse runs st21's client-only plug: if the local
//!   view is the controller's own (rid71), after `STU_30667BB6` it starts the
//!   client-only `VoiceStimulus` st23 on #10122, targets `STU_A3DEF368()` (the
//!   game mode entity, as for the kill feed's `0765.025`), delay 0.2 s. The
//!   mode entity (`16B2`) has the announcer's voice set `001D.05F`, which has
//!   one line for each of the five stimuli. #10122 reaches the client through
//!   st31 (`ClientOnlySharedVars`, presence bit).
//!
//! So the server sends no voice line: it pulses st21 with #10122 set, and the
//! killer's own client says the line. The mode entity is `16B2` in every
//! mode, the Practice Range too.
//!
//! The server runs `0215` itself under the player's `0C90` in the VM
//! (`orpheus_script::controller`, enabler 2 of `docs/dev/graphs-plan.md`):
//! each elimination goes to the controller as `0093.025`, and the
//! controller frames carry st21's count and #10122 as the script has them
//! ([`pulse`]). Assumptions: an elimination is the final blow (the server
//! credits no assists; `0215` rid57 counts any elimination, or with
//! `STU_A3DEF368().id(12421)`, which no mode sets, only `0091.026`, likely
//! the final blow); every enemy player's body or bot passes rid15 (the
//! controller world answers its filter true). The Range's Training
//! Bots (`1403`, `14ED`) carry only the target tag `02ED`; `031D` is on two
//! other Training Bots only (`14A9`, `14D2`, placed on the Tutorial map
//! `0661`), and `06AD` is on no hero body (only on B.O.B., `25C3`), so the
//! game gives heroes `06AD` itself; since retail counts Training Bot kills,
//! the bots must get it too, or pass rid15 some other way. #1954 (set on
//! `069C.025`) and #21925 (a mode entity variable) stay off. The wait ends
//! before a kill of the same tick is counted. The frames carry `0215` from
//! st21's first pulse on (retail: from the start; the frames are still
//! built by hand, `crate::controller`). Not done: the hero's own reaction
//! 1.6 s after a third kill (`00C9.078` on the controller, rid85: no hero's
//! voice set has a line for it), the multikill career stats (rid28,
//! `0345`/`0346`/`0348.062`), the "N PLAYER KILL STREAK!" text and the team
//! kill (see `docs/notes/behavior.md`).

use orpheus_core::tick::TICK_PERIOD;
use orpheus_script::controller::Multikill;

use crate::replication::kill_feed_fresh;

/// `0215` st4's wait, #2036 = 2 s, in world ticks (2000 / 16 = 125).
pub const MULTIKILL_WINDOW_TICKS: u32 = 2000 / TICK_PERIOD.as_millis() as u32;

/// What `0215`'s pulser and #10122 send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultikillPulse {
    /// st21's pulse count (8 bits on the wire).
    pub count: u8,
    /// Its fresh bit: the last pulse is at most 500 ms old (the pulser
    /// policy, [`kill_feed_fresh`]). A client that reads st21 for the first
    /// time pulses once with the bit set, else takes the count as seen.
    pub fresh: bool,
    /// #10122; none once a kill without a line cleared it (`0215` rid72).
    pub stimulus: Option<u64>,
}

/// st21 and #10122 of the controller's `0215` at world tick `tick`.
pub fn pulse(multikill: Multikill, tick: u32) -> MultikillPulse {
    let pulsed = multikill.pulsed_ms / TICK_PERIOD.as_millis() as u64;
    MultikillPulse {
        count: multikill.pulses,
        fresh: kill_feed_fresh(tick, u32::try_from(pulsed).unwrap_or(u32::MAX)),
        stimulus: multikill.stimulus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pulse stays fresh for 500 ms: 31 ticks of 16 ms, not 32.
    #[test]
    fn a_multikill_pulse_is_fresh_for_500_ms() {
        let multikill = Multikill {
            pulses: 2,
            pulsed_ms: 1000 * 16,
            stimulus: Some(0x0ee0_0000_0000_058b),
        };
        assert_eq!(
            pulse(multikill, 1000 + 31),
            MultikillPulse {
                count: 2,
                fresh: true,
                stimulus: Some(0x0ee0_0000_0000_058b),
            }
        );
        assert!(!pulse(multikill, 1000 + 32).fresh);
    }
}
