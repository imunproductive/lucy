//! The UX presenter `STU_4BD2E886` (over `STUStatescriptStateUXPresenter`
//! `STU_7A7F2732`): a fade in, a hold and a fade out. The match end's card
//! presenter `180A` st19 moves on to the next card with it: its
//! `m_82A8A67E` plug sets `#59 + 1`.
//!
//! Client (1.74, state record #719, runtime vtable 0x7FF78B563AC8;
//! `python tools/stu.py STU_4BD2E886`), the state's block at `+0x468`:
//!
//! - Begin (slot 23 `0x7FF7899E73D0`): the progress (`+0x468`, a float
//!   from 0 to 1) starts at 0, at 1 when the fade in `m_AD8C28BA` (seconds)
//!   is not above 0; the hold (`+0x46C`) is `m_C69C2A73`; the bit
//!   (`+0x470`) and the fired byte (`+0x471`) are cleared; the state goes
//!   on the update list.
//! - Update (slot 14 `0x7FF7899E76D0`, with the milliseconds since the
//!   last update): it evaluates the fade out `m_D7ED647E` and the fade in.
//!   With `m_C69C2A73` set and the progress at 1, the hold loses the
//!   seconds and the bit is set once it is not above 0. With the bit set,
//!   the progress falls by the seconds over the fade out (to 0 when that
//!   is not above 0), and at 0 the state aborts itself (slot 39
//!   `0x7FF78AA81120`, the state op 2 `0x7FF78AA80480`). Else it rises by
//!   the seconds over the fade in (to 1 when that is not above 0), and the
//!   first time it reaches 1 the state follows `m_82A8A67E`. The progress
//!   is then clamped to 0..1. The arithmetic is the client's, in `f32`.
//! - Event 1 (slot 18 `0x7FF7899E7BE0`) sets the bit; the server's graphs
//!   post the state no event, so only the hold sets it here.
//! - End (slot 24 `0x7FF7899E7530`) clears the block.
//! - The frames carry the bit (reader `0x7FF7899E7050`, one bit into
//!   `+0x470`): the clients fade out with the server.
//!
//! The server runs the update once a server frame, with the milliseconds
//! since the last one (the begin's time first): the time it adds up is the
//! same as the client's updates within the frame.

use super::*;

/// A presenter's block (`+0x468..+0x472`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Presenter {
    /// `+0x468`: 0 hidden, 1 shown.
    pub progress: f32,
    /// `+0x46C`: the hold's seconds left.
    pub hold: f32,
    /// `+0x470`: fading out; the frames carry it.
    pub bit: bool,
    /// `+0x471`: `m_82A8A67E` was followed.
    pub fired: bool,
    /// When the last update ran.
    pub last_ms: u64,
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    pub(super) fn begin_presenter(&mut self, key: StateKey, node: &Node) {
        let fade_in = self.presenter_seconds(key, node, "m_AD8C28BA");
        let hold = self.presenter_seconds(key, node, "m_C69C2A73");
        let now = self.machine.now_ms;
        if let Some(state) = self.state_mut(key) {
            state.presenter = Some(Presenter {
                progress: if fade_in <= 0.0 { 1.0 } else { 0.0 },
                hold,
                bit: false,
                fired: false,
                last_ms: now,
            });
        }
        self.register_tick(key, now);
    }

    /// One update (module notes).
    pub(super) fn tick_presenter(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let Some(mut presenter) = self.state(key).and_then(|state| state.presenter) else {
            return Ok(());
        };
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("presenter");
        let now = self.machine.now_ms;
        let ms = now.saturating_sub(presenter.last_ms) as f32;
        presenter.last_ms = now;
        let fade_out = self.presenter_seconds(key, node, "m_D7ED647E");
        let fade_in = self.presenter_seconds(key, node, "m_AD8C28BA");
        if !field(&node.fields, "m_C69C2A73").is_null() && presenter.progress >= 1.0 {
            presenter.hold -= (ms * 0.001).max(0.0);
            if presenter.hold <= 0.0 {
                presenter.bit = true;
            }
        }
        let mut abort = false;
        let mut fire = false;
        if presenter.bit {
            if presenter.progress > 0.0 {
                presenter.progress = if fade_out <= 0.0 {
                    0.0
                } else {
                    presenter.progress - ms * 0.001 / fade_out
                };
            }
            abort = presenter.progress <= 0.0;
        } else {
            if presenter.progress < 1.0 {
                presenter.progress = if fade_in <= 0.0 {
                    1.0
                } else {
                    ms * 0.001 / fade_in + presenter.progress
                };
            }
            if presenter.progress >= 1.0 && !presenter.fired {
                presenter.fired = true;
                fire = true;
            }
        }
        presenter.progress = presenter.progress.clamp(0.0, 1.0);
        if let Some(state) = self.state_mut(key) {
            state.presenter = Some(presenter);
        }
        if abort {
            self.post(id, Event::Abort(key.raw))?;
        }
        if fire {
            self.follow(id, node, "m_82A8A67E", Some(key), depth)?;
        }
        Ok(())
    }

    /// A seconds config of the presenter, 0 when unset or not a number.
    fn presenter_seconds(&mut self, key: StateKey, node: &Node, name: &str) -> f32 {
        match field(&node.fields, name) {
            Json::Null => 0.0,
            config => self
                .evaluate(key.instance, node, config, Some(key))
                .map_or(0.0, |value| expr::to_float(&value) as f32),
        }
    }
}
