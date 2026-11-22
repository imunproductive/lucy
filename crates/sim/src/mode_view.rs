//! The game mode entity's statescript on one client.
//!
//! A hand-built mode frame (the match flow's mode UI,
//! [`crate::scene_assets::PreparedScene::mode_frame`]) goes whole each time.
//! Where the mode graph drives the mode entity ([`crate::mode_script`]),
//! its frame goes as the other bodies' (`others.rs`) and the bots'
//! (`bot_script.rs`) do: the full frame once, then observer deltas from
//! what the client acknowledged. The client's statescript host takes them
//! the same way for every entity: it applies a chunk whose first number is
//! at most one past the last it applied (`0x7FF78991E630`), whole
//! (`0x7FF78991ECA0`) or as a delta (`0x7FF78991F080`)
//! (refs/reports/OwnerPrediction.md, "Day (2026-11-17)", DayJoin). So a
//! tick's change costs a few bytes, where the Range's whole `13C0` frame
//! is about 920 (refs/reports/ModeGraphGaps.md, "DayModeFrame").
//!
//! A chunk too big for a datagram (a Workshop match's frame: its rules'
//! global arrays and action instances on the mode entity, about 10 KB)
//! goes in parts, as any entity's (`replication/parts.rs`).

use std::sync::Arc;

use lucy_core::bits::BitWriter;
use lucy_script::{
    snapshot::{ProjectedSnapshot, SnapshotEncoder, SnapshotError},
    stream::{BodyStream, DeltaKind, ScriptChunk, Stream},
    timeline::FRAME_MS,
};
use lucy_wire::entity::GAME_MODE_ENTITY;

/// An unchanged graph frame whose delta the client hasn't acknowledged
/// goes again after this many ticks (a lost delta isn't sent again by
/// itself), as another body's frame of values only (`others.rs`).
const RESEND_TICKS: u32 = 8;

/// The mode entity's frame now.
pub enum ModeFrame<'a> {
    /// A hand-built full frame.
    Full(&'a BitWriter),
    /// The mode graph's observer projection, with the encoder of its graphs.
    Graph {
        encoder: SnapshotEncoder<'a>,
        projected: &'a Arc<ProjectedSnapshot>,
    },
}

enum Chunks {
    Full(Stream),
    Graph(Box<BodyStream<ProjectedSnapshot>>),
}

/// The mode entity's frames on one client.
pub(crate) struct ModeView {
    chunks: Chunks,
    /// A frame arrived: the client has the mode UI.
    arrived: bool,
    /// The graph frame the newest delta was made from, and the tick it
    /// may go again unchanged.
    queued: Option<(Arc<ProjectedSnapshot>, u32)>,
    /// The newest graph delta not sent yet: a newer one replaces it.
    unsent: Option<u32>,
}

impl ModeView {
    pub(crate) fn new() -> Self {
        Self {
            chunks: Chunks::Full(Stream::new(GAME_MODE_ENTITY)),
            arrived: false,
            queued: None,
            unsent: None,
        }
    }

    /// A frame was queued for the client.
    pub(crate) fn started(&self) -> bool {
        match &self.chunks {
            Chunks::Full(stream) => stream.latest_data() != 0,
            Chunks::Graph(stream) => stream.last() != 0,
        }
    }

    /// A frame arrived.
    pub(crate) fn arrived(&self) -> bool {
        self.arrived
    }

    /// The chunk that brings the client `frame` at `tick`, if one is due.
    pub(crate) fn queue(
        &mut self,
        frame: ModeFrame<'_>,
        tick: u32,
    ) -> Result<Option<ScriptChunk>, SnapshotError> {
        let chunk = match frame {
            ModeFrame::Full(payload) => {
                if let Chunks::Graph(stream) = &self.chunks {
                    self.chunks = Chunks::Full(Stream::after(GAME_MODE_ENTITY, stream.last()));
                    self.queued = None;
                    self.unsent = None;
                }
                let Chunks::Full(stream) = &mut self.chunks else {
                    unreachable!("a stream of full frames")
                };
                stream.full(payload)?
            }
            ModeFrame::Graph { encoder, projected } => {
                if let Chunks::Full(stream) = &self.chunks {
                    let mut graph = BodyStream::after(GAME_MODE_ENTITY, stream.latest_data());
                    // The mode entity's create arrived before any frame.
                    graph.create_arrived();
                    self.chunks = Chunks::Graph(Box::new(graph));
                }
                let Chunks::Graph(stream) = &mut self.chunks else {
                    unreachable!("a stream of graph frames")
                };
                if stream.base().is_none() {
                    if stream.pending().next().is_some() {
                        return Ok(None); // the full frame is on its way
                    }
                    self.queued = Some((Arc::clone(projected), tick.saturating_add(RESEND_TICKS)));
                    encoder.queue_full(stream, projected)?
                } else {
                    if self.queued.as_ref().is_some_and(|(queued, again)| {
                        Arc::ptr_eq(queued, projected) && tick < *again
                    }) {
                        return Ok(None);
                    }
                    self.queued = Some((Arc::clone(projected), tick.saturating_add(RESEND_TICKS)));
                    let Some(chunk) = encoder.queue_delta(
                        stream,
                        projected,
                        DeltaKind::Observer { resend: false },
                        FRAME_MS as u32,
                    )?
                    else {
                        return Ok(None);
                    };
                    if let Some(old) = self.unsent.replace(chunk.last) {
                        stream.discard_unsent(old);
                    }
                    chunk
                }
            }
        };
        Ok(Some(chunk))
    }

    /// Whether chunk `last` still has to reach the client: a graph chunk
    /// that a newer one replaced before it went out, or that an
    /// acknowledged one covers, doesn't.
    pub(crate) fn current(&self, last: u32) -> bool {
        match &self.chunks {
            Chunks::Full(_) => true,
            Chunks::Graph(stream) => stream.ready_to_send(last),
        }
    }

    pub(crate) fn sent(&mut self, last: u32) {
        if let Chunks::Graph(stream) = &mut self.chunks {
            stream.sent(last);
            if self.unsent == Some(last) {
                self.unsent = None;
            }
        }
    }

    /// Chunk `last` arrived or was lost; true: send it again.
    pub(crate) fn delivered(&mut self, last: u32, arrived: bool) -> bool {
        match &mut self.chunks {
            Chunks::Full(stream) if arrived => {
                self.arrived |= stream.arrived(last);
                false
            }
            Chunks::Full(_) => true,
            Chunks::Graph(stream) if arrived => {
                self.arrived |= stream.arrived(last).advanced;
                false
            }
            Chunks::Graph(stream) => stream.lost(last).retry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mode_script::ModeRunner, scene_assets::SceneAssets};
    use lucy_content::maps::GameMap;
    use lucy_script::wire::Variables;

    /// The Practice Range's `13C0` frame changes every tick (`0CBB` st58's
    /// timed ChaseVar #19530e toward 99999). A client gets it whole once
    /// (about 920 bytes), then each tick's change as a delta of a few bytes
    /// from the frame he acknowledged, which then is the graph's frame;
    /// an unchanged frame goes no more once acknowledged, and is sent
    /// again after `RESEND_TICKS` while it isn't.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_mode_frame_goes_whole_once_then_as_small_deltas() {
        let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let mut mode = ModeRunner::start(&scene, 100, Vec::new());
        let mut view = ModeView::new();
        let mut last: Option<Arc<ProjectedSnapshot>> = None;
        let bytes =
            |chunk: &ScriptChunk| chunk.update.chunk.as_ref().unwrap().bit_len().div_ceil(8);
        let mut deltas = Vec::new();
        for tick in 101..=160 {
            assert!(mode.advance(tick, &Variables::new(), Vec::new()).is_empty());
            let projected = Arc::new(mode.project(last.as_deref()).unwrap());
            let frame = ModeFrame::Graph {
                encoder: mode.encoder().unwrap(),
                projected: &projected,
            };
            let chunk = view.queue(frame, tick).unwrap().expect("a frame each tick");
            if tick == 101 {
                assert!(
                    bytes(&chunk) > 900,
                    "the full frame: {} bytes",
                    bytes(&chunk)
                );
            } else {
                deltas.push(bytes(&chunk));
            }
            view.sent(chunk.last);
            assert!(!view.delivered(chunk.last, true));
            assert!(view.arrived());
            let Chunks::Graph(stream) = &view.chunks else {
                panic!("graph frames")
            };
            assert_eq!(stream.base(), Some((chunk.last, &*projected)));
            last = Some(projected);
        }
        assert!(
            deltas.iter().all(|&bytes| bytes <= 32),
            "a tick's change: {deltas:?}"
        );
        // The same frame, acknowledged: nothing more.
        let projected = last.unwrap();
        assert_eq!(view.queue(graph(&mode, &projected), 161).unwrap(), None);
        let later = 161 + RESEND_TICKS;
        assert_eq!(view.queue(graph(&mode, &projected), later).unwrap(), None);
        // A change whose delta isn't acknowledged goes again after
        // `RESEND_TICKS`, unchanged.
        mode.advance(161, &Variables::new(), Vec::new());
        let changed = Arc::new(mode.project(Some(&projected)).unwrap());
        let delta = view.queue(graph(&mode, &changed), 162).unwrap().unwrap();
        view.sent(delta.last);
        assert_eq!(view.queue(graph(&mode, &changed), 163).unwrap(), None);
        let again = view
            .queue(graph(&mode, &changed), 162 + RESEND_TICKS)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(&again), bytes(&delta));
    }

    fn graph<'a>(mode: &'a ModeRunner, projected: &'a Arc<ProjectedSnapshot>) -> ModeFrame<'a> {
        ModeFrame::Graph {
            encoder: mode.encoder().unwrap(),
            projected,
        }
    }
}
