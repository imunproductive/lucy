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
//! A chunk bigger than [`MAX_CHUNK_BYTES`] (a Workshop match's frame: its
//! rules' global arrays and action instances on the mode entity, about
//! 10 KB) goes in parts, each a record of its own, which the client joins
//! into the chunk once it has them all ([`chunk_parts`]). A lost part
//! alone goes again while the chunk would; the chunk counts as arrived
//! once every part did, and no new delta goes while one is in parts.

use std::{collections::BTreeMap, sync::Arc};

use orpheus_core::bits::BitWriter;
use orpheus_script::{
    snapshot::{ProjectedSnapshot, SnapshotEncoder, SnapshotError},
    stream::{BodyStream, DeltaKind, ScriptChunk, Stream},
    timeline::FRAME_MS,
    wire::chunk_parts,
};
use orpheus_wire::entity::{EntityUpdate, GAME_MODE_ENTITY};

/// An unchanged graph frame whose delta the client hasn't acknowledged
/// goes again after this many ticks (a lost delta isn't sent again by
/// itself), as another body's frame of values only (`others.rs`).
const RESEND_TICKS: u32 = 8;

/// The biggest chunk that goes whole, and the biggest part of a bigger
/// one: a record must fit one datagram by itself
/// (`orpheus_wire::entity::MAX_PAYLOAD`, 1230 bytes) beside the frame's
/// header, its own header and the tick's reliable messages, or nothing
/// after it in the queue goes (`select_frame`'s "fragmentation
/// required"). The Range's whole `13C0` frame (about 920 bytes) is under
/// it.
pub(crate) const MAX_CHUNK_BYTES: usize = 1024;

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

/// A chunk for the client: its last number and its records, each with its
/// part number (none: the chunk whole).
#[derive(Debug, PartialEq)]
pub(crate) struct ModeChunk {
    pub last: u32,
    pub records: Vec<(EntityUpdate, Option<u16>)>,
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
    /// The chunks on their way in parts: each part's arrival.
    parts: BTreeMap<u32, Vec<bool>>,
}

impl ModeView {
    pub(crate) fn new() -> Self {
        Self {
            chunks: Chunks::Full(Stream::new(GAME_MODE_ENTITY)),
            arrived: false,
            queued: None,
            unsent: None,
            parts: BTreeMap::new(),
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
    ) -> Result<Option<ModeChunk>, SnapshotError> {
        let current = |chunks: &Chunks, last| match chunks {
            Chunks::Full(_) => true,
            Chunks::Graph(stream) => stream.ready_to_send(last),
        };
        self.parts.retain(|&last, _| current(&self.chunks, last));
        let chunk = match frame {
            ModeFrame::Full(payload) => {
                if let Chunks::Graph(stream) = &self.chunks {
                    self.chunks = Chunks::Full(Stream::after(GAME_MODE_ENTITY, stream.last()));
                    self.queued = None;
                    self.unsent = None;
                    self.parts.clear();
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
                    self.parts.clear();
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
                    if !self.parts.is_empty() {
                        return Ok(None); // a chunk in parts is on its way
                    }
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
        self.split(chunk).map(Some)
    }

    /// `chunk`'s records: itself, or its parts when it is bigger than
    /// [`MAX_CHUNK_BYTES`].
    fn split(&mut self, chunk: ScriptChunk) -> Result<ModeChunk, SnapshotError> {
        let ScriptChunk { update, last, .. } = chunk;
        let encoded = update.chunk.as_ref().expect("a script chunk");
        let Some(parts) = chunk_parts(encoded, MAX_CHUNK_BYTES)? else {
            return Ok(ModeChunk {
                last,
                records: vec![(update, None)],
            });
        };
        self.parts.insert(last, vec![false; parts.len()]);
        let records = (0..)
            .zip(parts)
            .map(|(number, part)| {
                let update = EntityUpdate {
                    entity: update.entity,
                    chunk: Some(part),
                    ..EntityUpdate::default()
                };
                (update, Some(number))
            })
            .collect();
        Ok(ModeChunk { last, records })
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

    /// Part `part` (none: the whole) of chunk `last` arrived or was lost;
    /// true: send it again. The chunk arrived once its every part did; a
    /// lost part goes again while the chunk would, else the chunk is lost.
    pub(crate) fn delivered(&mut self, last: u32, part: Option<u16>, arrived: bool) -> bool {
        if let Some(part) = part {
            let Some(parts) = self.parts.get_mut(&last) else {
                return false; // the chunk was given up
            };
            if arrived {
                parts[usize::from(part)] = true;
                if !parts.iter().all(|&arrived| arrived) {
                    return false;
                }
                self.parts.remove(&last);
            } else {
                let retry = self.whole_delivered(last, false);
                if !retry {
                    self.parts.remove(&last);
                }
                return retry;
            }
        }
        self.whole_delivered(last, arrived)
    }

    fn whole_delivered(&mut self, last: u32, arrived: bool) -> bool {
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
    use orpheus_content::maps::GameMap;
    use orpheus_script::wire::Variables;

    /// The Practice Range's `13C0` frame changes every tick (`0CBB` st58's
    /// timed ChaseVar #19530e toward 99999). A client gets it whole once
    /// (about 920 bytes), then each tick's change as a delta of a few bytes
    /// from the frame he acknowledged, which then is the graph's frame;
    /// an unchanged frame goes no more once acknowledged, and is sent
    /// again after `RESEND_TICKS` while it isn't.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_mode_frame_goes_whole_once_then_as_small_deltas() {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let mut mode = ModeRunner::start(&scene, 100, Vec::new());
        let mut view = ModeView::new();
        let mut last: Option<Arc<ProjectedSnapshot>> = None;
        let bytes = |chunk: &ModeChunk| {
            let [(update, None)] = chunk.records.as_slice() else {
                panic!("a chunk in parts")
            };
            update.chunk.as_ref().unwrap().bit_len().div_ceil(8)
        };
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
            assert!(!view.delivered(chunk.last, None, true));
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

    /// A frame bigger than a chunk goes in parts, each small enough for a
    /// datagram; it counts as arrived once every part did, and a lost part
    /// goes again by itself.
    #[test]
    fn a_frame_bigger_than_a_chunk_arrives_once_all_its_parts_did() {
        let mut payload = BitWriter::new();
        for byte in 0..3000u32 {
            payload.bits(u64::from(byte % 251), 8).unwrap();
        }
        let mut view = ModeView::new();
        let chunk = view.queue(ModeFrame::Full(&payload), 1).unwrap().unwrap();
        let parts: Vec<u16> = chunk
            .records
            .iter()
            .map(|(update, part)| {
                assert!(update.chunk.as_ref().unwrap().as_bytes().len() <= MAX_CHUNK_BYTES);
                part.expect("a part")
            })
            .collect();
        assert_eq!(parts, [0, 1, 2]);
        view.sent(chunk.last);
        assert!(!view.delivered(chunk.last, Some(0), true));
        assert!(
            view.delivered(chunk.last, Some(1), false),
            "a lost part goes again"
        );
        assert!(!view.delivered(chunk.last, Some(2), true));
        assert!(!view.arrived());
        assert!(!view.delivered(chunk.last, Some(1), true));
        assert!(view.arrived());
    }

    fn graph<'a>(mode: &'a ModeRunner, projected: &'a Arc<ProjectedSnapshot>) -> ModeFrame<'a> {
        ModeFrame::Graph {
            encoder: mode.encoder().unwrap(),
            projected,
        }
    }
}
