//! Records whose statescript chunk is too big for one datagram, in parts.
//!
//! A record must fit one datagram by itself
//! (`orpheus_wire::entity::MAX_PAYLOAD`, 1230 bytes) beside the frame's
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
//! arrived once every part did.

use std::{collections::HashMap, sync::Arc};

use orpheus_script::wire::{WireError, chunk_parts};
use orpheus_wire::entity::EntityUpdate;

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
    /// It arrived; another part hasn't yet.
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
        let records: Vec<_> = (0..)
            .zip(parts)
            .map(|(number, part)| {
                let update = if number == 0 {
                    EntityUpdate {
                        entity: update.entity,
                        operation: update.operation.clone(),
                        movement: update.movement.clone(),
                        chunk: Some(part),
                        ch2: update.ch2.clone(),
                    }
                } else {
                    EntityUpdate {
                        entity: update.entity,
                        chunk: Some(part),
                        ..EntityUpdate::default()
                    }
                };
                Arc::new(Record {
                    update,
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

    /// The first part of each record on its way in parts.
    pub(super) fn records(&self) -> impl Iterator<Item = (Part, &Arc<Record>)> {
        self.chunks.iter().map(|(&chunk, item)| {
            (
                Part {
                    chunk,
                    number: 0,
                },
                &item.first,
            )
        })
    }

    pub(super) fn clear(&mut self) {
        self.chunks.clear();
    }
}
