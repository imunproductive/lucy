//! Outgoing game message queues and the delivery of successfully sent frames.

use std::{borrow::Borrow, collections::VecDeque, error::Error, fmt, mem};

use orpheus_wire::entity::{
    EntityUpdate, FrameCandidates, SelectedFrame, WorldError, select_frame,
};
use orpheus_wire::link::delivery;

type Reliable = (u32, Vec<u8>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinalExhausted;

impl fmt::Display for OrdinalExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("game reliable message ordinals exhausted")
    }
}

impl Error for OrdinalExhausted {}

#[derive(Debug)]
pub enum SendError<E> {
    Select(WorldError),
    Transport(E),
}

impl<E: fmt::Display> fmt::Display for SendError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Select(error) => write!(f, "game frame rejected: {error}"),
            Self::Transport(error) => write!(f, "game frame send failed: {error}"),
        }
    }
}

impl<E: Error + 'static> Error for SendError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Select(error) => Some(error),
            Self::Transport(error) => Some(error),
        }
    }
}

pub struct FrameRecords<'a, Queued = EntityUpdate> {
    pub tick: u32,
    /// The match supplies the records eligible to send in dependency order.
    pub queued: &'a [Queued],
    pub movements: &'a [EntityUpdate],
    pub owner_correction: bool,
    pub reliable_only: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordSource {
    Queued(usize),
    Movement(usize),
}

#[derive(Debug, Eq, PartialEq)]
pub struct SentFrame {
    pub sequence: u32,
    /// Records actually sent, including best-effort movement, in wire order.
    pub records: Vec<RecordSource>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PacketDelivery<T> {
    pub sequence: u32,
    pub arrived: bool,
    pub records: Vec<T>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DeliveryReport<T> {
    pub requeued_reliable: usize,
    /// Resolved tracked datagrams in send order. The match checks record
    /// lifetimes before applying delivery or requeueing a lost record.
    pub packets: Vec<PacketDelivery<T>>,
}

struct InFlight<T> {
    sequence: u32,
    reliable: Vec<Reliable>,
    records: Vec<T>,
}

pub struct GameEgress<T> {
    next_reliable: u64,
    reliable: VecDeque<Reliable>,
    unreliable: VecDeque<Vec<u8>>,
    in_flight: VecDeque<InFlight<T>>,
}

impl<T> Default for GameEgress<T> {
    fn default() -> Self {
        Self {
            next_reliable: 0,
            reliable: VecDeque::new(),
            unreliable: VecDeque::new(),
            in_flight: VecDeque::new(),
        }
    }
}

impl<T> GameEgress<T> {
    pub fn queue_reliable(&mut self, message: Vec<u8>) -> Result<u32, OrdinalExhausted> {
        let ordinal = u32::try_from(self.next_reliable).map_err(|_| OrdinalExhausted)?;
        self.reliable.push_back((ordinal, message));
        self.next_reliable += 1;
        Ok(ordinal)
    }

    pub fn queue_unreliable(&mut self, message: Vec<u8>) {
        self.unreliable.push_back(message);
    }

    pub fn oldest_pending_reliable(&self) -> u64 {
        self.reliable
            .front()
            .into_iter()
            .chain(
                self.in_flight
                    .iter()
                    .filter_map(|packet| packet.reliable.first()),
            )
            .map(|(ordinal, _)| u64::from(*ordinal))
            .min()
            .unwrap_or(self.next_reliable)
    }

    /// Ownership and other prerequisites take effect after the full reliable
    /// prefix, even if the datagram containing the prerequisite arrived first.
    pub fn delivered_through(&self, ordinal: u32) -> bool {
        self.oldest_pending_reliable() > u64::from(ordinal)
    }

    /// Select and attempt one frame. The transport owns packet sequence
    /// allocation and returns the sequence only after the datagram is sent.
    /// Record associations and queue removal follow that successful send.
    pub fn send_frame<E, Queued: Borrow<EntityUpdate>>(
        &mut self,
        records: FrameRecords<'_, Queued>,
        ack_for_records: impl Fn(&[&EntityUpdate]) -> u32,
        mut identify: impl FnMut(RecordSource) -> T,
        send: impl FnOnce(Vec<u8>) -> Result<u32, E>,
    ) -> Result<SentFrame, SendError<E>> {
        let SelectedFrame {
            payload,
            reliable_count,
            unreliable_count,
            entities,
        } = select_frame(
            FrameCandidates {
                tick: records.tick,
                reliable: self.reliable.make_contiguous(),
                unreliable: self.unreliable.make_contiguous(),
                queued: records.queued,
                movements: records.movements,
                owner_correction: records.owner_correction,
                reliable_only: records.reliable_only,
            },
            ack_for_records,
        )
        .map_err(SendError::Select)?;
        let sequence = send(payload).map_err(SendError::Transport)?;

        let mut sent = Vec::with_capacity(entities.len());
        let mut tracked = Vec::new();
        for (index, entity) in entities.into_iter().enumerate() {
            let source = match index.checked_sub(records.queued.len()) {
                None => RecordSource::Queued(index),
                Some(index) => {
                    let index = if records
                        .movements
                        .get(index)
                        .is_some_and(|candidate| std::ptr::eq(entity, candidate))
                    {
                        index
                    } else {
                        // Budget selection may replace the tail of the movement
                        // prefix with the final record, the owner's correction.
                        records.movements.len() - 1
                    };
                    RecordSource::Movement(index)
                }
            };
            sent.push(source);
            if entity.operation.is_some() || entity.chunk.is_some() || entity.ch2.is_some() {
                tracked.push(identify(source));
            }
        }
        self.unreliable.drain(..unreliable_count);
        if reliable_count != 0 || !tracked.is_empty() {
            self.in_flight.push_back(InFlight {
                sequence,
                reliable: self.reliable.drain(..reliable_count).collect(),
                records: tracked,
            });
        }
        Ok(SentFrame {
            sequence,
            records: sent,
        })
    }

    /// Interpret one authenticated ACK window. Resolved packets leave flight
    /// once, so a duplicate ACK cannot deliver or retry their records again.
    pub fn acknowledge(&mut self, ack: u32, ack_bits: u64) -> DeliveryReport<T> {
        let mut lost = Vec::new();
        let mut packets = Vec::new();
        self.in_flight.retain_mut(|packet| {
            let Some(arrived) = delivery(packet.sequence, ack, ack_bits) else {
                return true;
            };
            if !arrived {
                lost.append(&mut packet.reliable);
            }
            packets.push(PacketDelivery {
                sequence: packet.sequence,
                arrived,
                records: mem::take(&mut packet.records),
            });
            false
        });
        let requeued_reliable = lost.len();
        if requeued_reliable != 0 {
            lost.extend(self.reliable.drain(..));
            lost.sort_unstable_by_key(|(ordinal, _)| *ordinal);
            self.reliable = lost.into();
        }
        DeliveryReport {
            requeued_reliable,
            packets,
        }
    }
}

#[cfg(test)]
mod tests;
