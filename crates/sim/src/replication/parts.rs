//! Records whose statescript chunk is too big for one datagram, in parts.
//!
//! A record must fit one datagram by itself
//! (`lucy_wire::entity::MAX_PAYLOAD`, 1230 bytes) beside the frame's
//! header, its own header and the tick's reliable messages, or nothing
//! after it in the queue goes (`select_frame`'s "fragmentation required").
//! The client's statescript host joins a chunk from parts for every entity
//! (refs/reports/Workshop.md, "WorkshopRuntime6"): its chunk reader
//! (`0x7FF7898A6450`) reads a part's number and last-part bit, its applier
//! (`0x7FF78991E630`) applies no part, and its joiner (`0x7FF78991FE30`)
//! joins parts 0 to the last of one chunk once all are queued
//! ([`chunk_parts`]). So any record whose chunk is bigger than
//! [`MAX_CHUNK_BYTES`] (a Workshop match's mode frame, its rules' global
//! arrays and action instances, about 10 KB; a controller or body frame
//! as big) goes as parts, each a record of its own: the first with the
//! record's other fields, the rest with their share of the chunk alone. A
//! lost part alone goes again while the record would; the record counts as
//! arrived once every part did. While a chunk is in parts, the entity's
//! later chunks wait: the client applies a chunk whose first number is at
//! most one past the last it applied, so a delta from the acknowledged
//! frame would go ahead of the joined chunk, which then took it back.

use std::{collections::HashMap, sync::Arc};

use lucy_script::wire::{WireError, chunk_parts};
use lucy_wire::entity::EntityUpdate;

use super::Record;

/// The biggest chunk that goes whole, and the biggest part of a bigger
/// one. The Range's whole `13C0` mode frame (about 920 bytes) is under it.
pub(crate) const MAX_CHUNK_BYTES: usize = 1024;

/// A part of a record's chunk: the chunk among those in parts, and the
/// part's number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Part {
    chunk: u64,
    number: u16,
}

/// What a part's datagram outcome means for its record.
pub(super) enum PartOutcome {
    /// It arrived; another part hasn't yet (or the record was given up).
    Waiting,
    /// It arrived, the last of its parts: the record arrived (its first
    /// part, which carries the record's other fields).
    Arrived(Arc<Record>),
    /// It was lost.
    Lost,
}

struct Chunk {
    first: Arc<Record>,
    arrived: Vec<bool>,
}

/// The records on their way in parts.
#[derive(Default)]
pub(super) struct Parts {
    next: u64,
    chunks: HashMap<u64, Chunk>,
}

/// Per entity, its oldest chunk on its way in parts ([`Parts::holding`]).
pub(super) struct Holding(HashMap<u32, u64>);

impl Holding {
    /// Whether `record` waits: it carries a chunk of an entity that has an
    /// older one in parts on its way.
    pub(super) fn holds(&self, record: &Record) -> bool {
        record.update.chunk.is_some()
            && self
                .0
                .get(&record.update.entity)
                .is_some_and(|&chunk| record.part.is_none_or(|part| part.chunk != chunk))
    }
}

impl Parts {
    /// `record` as the queue takes it: whole when its chunk fits
    /// [`MAX_CHUNK_BYTES`], else its parts.
    pub(super) fn split(&mut self, record: Record) -> Result<Vec<Arc<Record>>, WireError> {
        let Some(parts) = record
            .update
            .chunk
            .as_ref()
            .map(|chunk| chunk_parts(chunk, MAX_CHUNK_BYTES))
            .transpose()?
            .flatten()
        else {
            return Ok(vec![Arc::new(record)]);
        };
        let chunk = self.next;
        self.next += 1;
        let Record {
            update, delivery, ..
        } = record;
        let EntityUpdate {
            entity,
            operation,
            movement,
            ch2,
            ..
        } = update;
        let mut first = Some((operation, movement, ch2));
        let records: Vec<_> = (0..)
            .zip(parts)
            .map(|(number, part)| {
                let (operation, movement, ch2) = first.take().unwrap_or_default();
                Arc::new(Record {
                    update: EntityUpdate {
                        entity,
                        operation,
                        movement,
                        chunk: Some(part),
                        ch2,
                    },
                    delivery,
                    part: Some(Part { chunk, number }),
                })
            })
            .collect();
        self.chunks.insert(
            chunk,
            Chunk {
                first: Arc::clone(&records[0]),
                arrived: vec![false; records.len()],
            },
        );
        Ok(records)
    }

    /// Whether `part`'s record is still on its way: not arrived, and not
    /// given up.
    pub(super) fn current(&self, part: Part) -> bool {
        self.chunks.contains_key(&part.chunk)
    }

    /// `part`'s datagram arrived or was lost.
    pub(super) fn delivered(&mut self, part: Part, arrived: bool) -> PartOutcome {
        if !arrived {
            return PartOutcome::Lost;
        }
        let Some(chunk) = self.chunks.get_mut(&part.chunk) else {
            return PartOutcome::Waiting;
        };
        chunk.arrived[usize::from(part.number)] = true;
        if !chunk.arrived.iter().all(|&arrived| arrived) {
            return PartOutcome::Waiting;
        }
        let chunk = self.chunks.remove(&part.chunk).expect("the chunk in parts");
        PartOutcome::Arrived(chunk.first)
    }

    /// `part`'s record won't go again: its other parts go no more.
    pub(super) fn give_up(&mut self, part: Part) {
        self.chunks.remove(&part.chunk);
    }

    /// Keeps the records in parts that are `current` (a newer chunk
    /// replaced the others before they went, or an acknowledged one
    /// covers them).
    pub(super) fn retain(&mut self, current: impl Fn(&Record) -> bool) {
        self.chunks.retain(|_, chunk| current(&chunk.first));
    }

    /// Each entity's oldest chunk in parts whose record is `current`.
    pub(super) fn holding(&self, current: impl Fn(&Record) -> bool) -> Holding {
        let mut oldest = HashMap::new();
        for (&chunk, item) in &self.chunks {
            if !current(&item.first) {
                continue;
            }
            oldest
                .entry(item.first.update.entity)
                .and_modify(|old: &mut u64| *old = (*old).min(chunk))
                .or_insert(chunk);
        }
        Holding(oldest)
    }

    pub(super) fn clear(&mut self) {
        self.chunks.clear();
    }
}
