//! State script chunk numbering, acknowledged baselines, and transport retry state.
//!
//! Controller streams follow ow174/game/statescript.py. Body streams follow
//! ow174/game/script/driver.py and leave graph projection to the caller.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    time::Duration,
};

use crate::wire::{WireError, chunk, owner_ack};
use orpheus_core::bits::BitWriter;
use orpheus_core::tick::MAX_LEASE_FRAME;
use orpheus_wire::commands::{FRAME_HALF, frame_distance};
use orpheus_wire::entity::EntityUpdate;

pub const OWNER_SETTLE_FRAMES: u32 = 16;
const OWNER_ACK_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamError {
    Wire(WireError),
    ChunkExhausted,
    CommandLimit(u32),
    CommandWrap { previous: u32, incoming: u32 },
    BodyRetired,
    BaselinePending,
    OwnerOrder,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire(error) => error.fmt(f),
            Self::ChunkExhausted => f.write_str("state script chunk numbering exhausted"),
            Self::CommandLimit(frame) => write!(f, "command frame {frame} exceeds the lease limit"),
            Self::CommandWrap { previous, incoming } => {
                write!(f, "command frame wrapped from {previous} to {incoming}")
            }
            Self::BodyRetired => f.write_str("state script body has retired"),
            Self::BaselinePending => f.write_str("state script baseline awaits acknowledgement"),
            Self::OwnerOrder => f.write_str("owner delta is outside the current stream phase"),
        }
    }
}

impl Error for StreamError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WireError> for StreamError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

#[derive(Debug, PartialEq)]
pub struct ScriptChunk {
    pub update: EntityUpdate,
    pub last: u32,
    /// Retry on loss by default. Body streams also retain the exact initial
    /// owner delta after its first successful transmission.
    pub resend: bool,
}

fn next(last: u32) -> Result<u32, StreamError> {
    last.checked_add(1).ok_or(StreamError::ChunkExhausted)
}

fn update(
    entity: u32,
    payload: &BitWriter,
    first: u32,
    last: u32,
    resend: bool,
) -> Result<ScriptChunk, StreamError> {
    let span = last.checked_sub(first).ok_or(StreamError::ChunkExhausted)?;
    Ok(ScriptChunk {
        update: EntityUpdate {
            entity,
            chunk: Some(chunk(payload, first, span)?),
            ..EntityUpdate::default()
        },
        last,
        resend,
    })
}

fn validate_frame(previous: u32, incoming: u32) -> Result<(), StreamError> {
    if incoming > MAX_LEASE_FRAME {
        return Err(StreamError::CommandLimit(incoming));
    }
    if incoming < previous && frame_distance(incoming, previous) < FRAME_HALF {
        return Err(StreamError::CommandWrap { previous, incoming });
    }
    Ok(())
}

#[derive(Debug)]
pub struct Stream {
    entity: u32,
    last: u32,
    delivered: u32,
    data_last: u32,
    cmfd: u32,
    next_ack: Duration,
}

impl Stream {
    pub fn new(entity: u32) -> Self {
        Self {
            entity,
            last: 0,
            delivered: 0,
            data_last: 0,
            cmfd: 0,
            next_ack: Duration::ZERO,
        }
    }

    /// A stream of `entity` that goes on from chunk `last` of another one:
    /// the client drops a chunk numbered at or below the last it applied.
    pub fn after(entity: u32, last: u32) -> Self {
        Self {
            last,
            ..Self::new(entity)
        }
    }

    pub fn delivered(&self) -> u32 {
        self.delivered
    }

    pub fn command_frame(&self) -> u32 {
        self.cmfd
    }

    pub fn latest_data(&self) -> u32 {
        self.data_last
    }

    pub fn data_in_flight(&self) -> bool {
        self.delivered < self.data_last
    }

    pub fn full(&mut self, payload: &BitWriter) -> Result<ScriptChunk, StreamError> {
        let last = next(self.last)?;
        let result = update(self.entity, payload, 0, last, true)?;
        self.last = last;
        self.data_last = last;
        Ok(result)
    }

    /// Build an owner's full frame with a strictly increasing CmFD. A writer
    /// error leaves both frame counters available for the next attempt.
    pub fn full_owner<E: From<StreamError>>(
        &mut self,
        newest: u32,
        encode: impl FnOnce(u32) -> Result<BitWriter, E>,
    ) -> Result<ScriptChunk, E> {
        validate_frame(self.cmfd, newest)?;
        let cmfd = newest.max(self.cmfd + 1);
        validate_frame(self.cmfd, cmfd)?;
        let result = self.full(&encode(cmfd)?)?;
        self.cmfd = cmfd;
        Ok(result)
    }

    /// After the data has arrived, ACK-only chunks may cumulatively cover
    /// earlier ACK-only chunks that the peer has missed.
    pub fn owner_ack(
        &mut self,
        now: Duration,
        newest: Option<u32>,
    ) -> Result<Option<ScriptChunk>, StreamError> {
        if self.data_last == 0 || now < self.next_ack {
            return Ok(None);
        }
        let next_ack = now.saturating_add(OWNER_ACK_EVERY);
        let Some(newest) = newest.filter(|_| !self.data_in_flight()) else {
            self.next_ack = next_ack;
            return Ok(None);
        };
        validate_frame(self.cmfd, newest)?;
        if newest <= self.cmfd {
            self.next_ack = next_ack;
            return Ok(None);
        }
        let first = next(self.delivered)?;
        let last = next(self.last)?;
        let result = update(self.entity, &owner_ack(newest)?, first, last, false)?;
        self.last = last;
        self.cmfd = newest;
        self.next_ack = next_ack;
        Ok(Some(result))
    }

    pub fn arrived(&mut self, last: u32) -> bool {
        if last <= self.delivered || last > self.last {
            return false;
        }
        self.delivered = last;
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ChangeKey {
    Instance(u16),
    Variable(u16, u32),
    State(u16, u16),
    VolleyIntroduction(u16, u16),
    /// A frame action (a wheel callout) and the chunk number it first went
    /// out in, which it keeps as its stamp until acknowledged.
    Action {
        serial: u32,
        stamp: u32,
    },
}

impl ChangeKey {
    fn carries_forward(self) -> bool {
        !matches!(self, Self::VolleyIntroduction(..))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeltaKind {
    Observer { resend: bool },
    InitialOwner(u32),
    Correction(u32),
}

impl DeltaKind {
    fn command_frame(self) -> Option<u32> {
        match self {
            Self::Observer { .. } => None,
            Self::InitialOwner(frame) | Self::Correction(frame) => Some(frame),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingKind {
    Full,
    Delta(DeltaKind),
}

#[derive(Debug)]
struct Pending<S> {
    snapshot: S,
    keys: BTreeSet<ChangeKey>,
    kind: PendingKind,
    sent: bool,
}

impl<S> Pending<S> {
    fn retry(&self) -> bool {
        match self.kind {
            PendingKind::Full => true,
            PendingKind::Delta(DeltaKind::Observer { resend }) => resend,
            PendingKind::Delta(DeltaKind::InitialOwner(_)) => self.sent,
            PendingKind::Delta(DeltaKind::Correction(_)) => false,
        }
    }
}

pub struct SentOwner<'a, S> {
    pub command_frame: u32,
    pub snapshot: &'a S,
    pub changes: &'a BTreeSet<ChangeKey>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BodyDelivery {
    pub retry: bool,
    pub advanced: bool,
    /// An individually transmitted owner report is resolved independently of
    /// cumulative baseline changes. True means its own packet arrived.
    pub owner_report: Option<bool>,
}

#[derive(Debug)]
pub struct BodyStream<S> {
    entity: u32,
    create_delivered: bool,
    retired: bool,
    last: u32,
    cmfd: u32,
    first_owner_frame: Option<u32>,
    base: Option<S>,
    base_last: u32,
    pending: BTreeMap<u32, Pending<S>>,
    sent_reports: BTreeSet<u32>,
    lost_keys: BTreeMap<ChangeKey, u32>,
    latest_lost: Option<S>,
    initial_owner: Option<u32>,
    /// The newest frame (server tick) of a datagram that carried the full
    /// chunk ([`Self::full_sent`]).
    full_frame: u32,
}

impl<S> BodyStream<S> {
    pub fn new(entity: u32) -> Self {
        Self {
            entity,
            create_delivered: false,
            retired: false,
            last: 0,
            cmfd: 0,
            first_owner_frame: None,
            base: None,
            base_last: 0,
            pending: BTreeMap::new(),
            sent_reports: BTreeSet::new(),
            lost_keys: BTreeMap::new(),
            latest_lost: None,
            initial_owner: None,
            full_frame: 0,
        }
    }

    /// A stream of `entity` that goes on from chunk `last` of another one
    /// (see [`Stream::after`]).
    pub fn after(entity: u32, last: u32) -> Self {
        Self {
            last,
            ..Self::new(entity)
        }
    }

    pub fn create_arrived(&mut self) {
        self.create_delivered = true;
    }

    pub fn ready_to_send(&self, last: u32) -> bool {
        self.create_delivered && !self.retired && self.pending.contains_key(&last)
    }

    pub fn last(&self) -> u32 {
        self.last
    }

    pub fn command_frame(&self) -> u32 {
        self.cmfd
    }

    pub fn base(&self) -> Option<(u32, &S)> {
        self.base
            .as_ref()
            .map(|snapshot| (self.base_last, snapshot))
    }

    pub fn pending(&self) -> impl Iterator<Item = (u32, &S, &BTreeSet<ChangeKey>)> {
        self.pending
            .iter()
            .map(|(&last, item)| (last, &item.snapshot, &item.keys))
    }

    pub fn latest_snapshot(&self) -> Option<&S> {
        self.pending
            .get(&self.last)
            .map(|item| &item.snapshot)
            .or(self.latest_lost.as_ref())
    }

    /// What the owner's client holds once the chunks already queued arrive:
    /// the newest queued chunk's snapshot, else the acknowledged one. Each
    /// chunk is cumulative from the acknowledged frame, so the newest covers
    /// every older one still in flight.
    pub fn expected(&self) -> Option<&S> {
        self.pending
            .get(&self.last)
            .map(|item| &item.snapshot)
            .or(self.base.as_ref())
    }

    /// The newest owner delta was lost and nothing went after it: its
    /// changes reach the client only in a new one ([`Self::lost`]).
    pub fn resend_due(&self) -> bool {
        self.latest_lost.is_some()
    }

    pub fn touched(&self) -> BTreeSet<ChangeKey> {
        self.lost_keys
            .keys()
            .chain(self.pending.values().flat_map(|item| &item.keys))
            .copied()
            .filter(|key| key.carries_forward())
            .collect()
    }

    /// The baseline may be prepared before the body's create arrives. Its
    /// record becomes sendable only after the create acknowledgement.
    pub fn full(&mut self, payload: &BitWriter, snapshot: S) -> Result<ScriptChunk, StreamError> {
        if self.retired {
            return Err(StreamError::BodyRetired);
        }
        let last = next(self.last)?;
        let result = update(self.entity, payload, 0, last, true)?;
        self.pending.insert(
            last,
            Pending {
                snapshot,
                keys: BTreeSet::new(),
                kind: PendingKind::Full,
                sent: false,
            },
        );
        self.last = last;
        self.latest_lost = None;
        Ok(result)
    }

    /// The full chunk went out in the datagram of `frame` (the server tick in
    /// its header). A full chunk carries no command frame: the client takes
    /// the frame of the datagram that brought it as the body's command frame
    /// (`0x7FF78991E3C0` keeps the chunk's frame, which the receive path set
    /// to the datagram's), and skips an owner frame whose command frame isn't
    /// past it as stale (`0x7FF78991E050` returns 2: the payload is read
    /// past, the chunk counts as applied). The initial owner delta, which
    /// begins every running state, waits for a command frame past the newest
    /// datagram that carried the full chunk ([`Self::owner_kind`]).
    pub fn full_sent(&mut self, frame: u32) {
        self.full_frame = self.full_frame.max(frame);
    }

    /// Find the kind of owner update due at this command frame. A caller may
    /// refresh an unsent projection at the same frame, or replace its initial
    /// projection before the old one reaches the transport.
    pub fn owner_kind(
        &self,
        frame: u32,
        refresh_unsent: bool,
        replace_initial: bool,
    ) -> Result<Option<DeltaKind>, StreamError> {
        if self.retired || self.base.is_none() {
            return Ok(None);
        }
        validate_frame(self.cmfd, frame)?;
        if frame == 0
            || frame < self.cmfd
            || self.initial_owner.is_some_and(|last| self.retry(last))
            || (frame == self.cmfd && !refresh_unsent)
        {
            return Ok(None);
        }
        let Some(first) = self.first_owner_frame.filter(|_| !replace_initial) else {
            return Ok((frame > self.full_frame).then_some(DeltaKind::InitialOwner(frame)));
        };
        Ok((frame.saturating_sub(first) >= OWNER_SETTLE_FRAMES)
            .then_some(DeltaKind::Correction(frame)))
    }

    pub fn delta(
        &mut self,
        payload: &BitWriter,
        snapshot: S,
        keys: BTreeSet<ChangeKey>,
        kind: DeltaKind,
    ) -> Result<ScriptChunk, StreamError> {
        if self.retired {
            return Err(StreamError::BodyRetired);
        }
        if self.base.is_none() {
            return Err(StreamError::BaselinePending);
        }
        if let Some(frame) = kind.command_frame() {
            validate_frame(self.cmfd, frame)?;
            if frame == 0 {
                return Err(WireError::ZeroCommandFrame.into());
            }
            let replacement = matches!(kind, DeltaKind::InitialOwner(_));
            if self.owner_kind(frame, true, replacement)? != Some(kind) {
                return Err(StreamError::OwnerOrder);
            }
        }
        let last = next(self.last)?;
        let first = next(self.base_last)?;
        let resend = matches!(kind, DeltaKind::Observer { resend: true });
        let result = update(self.entity, payload, first, last, resend)?;
        self.pending.insert(
            last,
            Pending {
                snapshot,
                keys,
                kind: PendingKind::Delta(kind),
                sent: false,
            },
        );
        if let Some(frame) = kind.command_frame() {
            self.cmfd = frame;
        }
        if let DeltaKind::InitialOwner(frame) = kind {
            self.first_owner_frame = Some(frame);
            self.initial_owner = Some(last);
        }
        self.last = last;
        self.latest_lost = None;
        Ok(result)
    }

    /// Remove a superseded queued delta that never reached the transport.
    pub fn discard_unsent(&mut self, last: u32) -> bool {
        if !self
            .pending
            .get(&last)
            .is_some_and(|item| !item.sent && matches!(item.kind, PendingKind::Delta(_)))
        {
            return false;
        }
        self.pending.remove(&last);
        if self.initial_owner == Some(last) {
            self.initial_owner = None;
        }
        true
    }

    /// Call after the selected datagram was actually sent. The first owner
    /// transmission exposes the snapshot and keys for its own report.
    pub fn sent(&mut self, last: u32) -> Option<SentOwner<'_, S>> {
        if !self.ready_to_send(last) {
            return None;
        }
        let item = self.pending.get_mut(&last)?;
        if item.sent {
            return None;
        }
        item.sent = true;
        let PendingKind::Delta(kind) = item.kind else {
            return None;
        };
        let command_frame = kind.command_frame()?;
        self.sent_reports.insert(last);
        Some(SentOwner {
            command_frame,
            snapshot: &item.snapshot,
            changes: &item.keys,
        })
    }

    pub fn retry(&self, last: u32) -> bool {
        !self.retired && self.pending.get(&last).is_some_and(Pending::retry)
    }

    pub fn arrived(&mut self, last: u32) -> BodyDelivery {
        if !self.create_delivered || self.retired {
            return BodyDelivery::default();
        }
        let owner_report = self.sent_reports.remove(&last).then_some(true);
        let mut delivery = BodyDelivery {
            owner_report,
            ..BodyDelivery::default()
        };
        let Some(item) = self.pending.remove(&last) else {
            return delivery;
        };
        self.base = Some(item.snapshot);
        self.base_last = last;
        self.pending.retain(|&number, _| number > last);
        self.lost_keys.retain(|_, number| *number > last);
        if self.initial_owner.is_some_and(|initial| initial <= last) {
            self.initial_owner = None;
        }
        delivery.advanced = true;
        delivery
    }

    pub fn lost(&mut self, last: u32) -> BodyDelivery {
        if self.retry(last) {
            return BodyDelivery {
                retry: true,
                ..BodyDelivery::default()
            };
        }
        let owner_report = self.sent_reports.remove(&last).then_some(false);
        let delivery = BodyDelivery {
            owner_report,
            ..BodyDelivery::default()
        };
        if self
            .pending
            .get(&last)
            .is_some_and(|item| matches!(item.kind, PendingKind::Delta(DeltaKind::Correction(_))))
        {
            let item = self
                .pending
                .remove(&last)
                .expect("checked pending owner delta");
            for key in item.keys.into_iter().filter(|key| key.carries_forward()) {
                let number = self.lost_keys.entry(key).or_default();
                *number = (*number).max(last);
            }
            if last == self.last {
                self.latest_lost = Some(item.snapshot);
            }
        }
        // A lost observer delta remains in the cumulative projection until a
        // later acknowledgement. Its instances may still need removal.
        delivery
    }

    pub fn retire(&mut self) {
        self.retired = true;
        self.pending.clear();
        self.sent_reports.clear();
        self.lost_keys.clear();
        self.latest_lost = None;
        self.initial_owner = None;
    }
}

#[cfg(test)]
mod tests;
