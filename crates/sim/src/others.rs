//! The other players of a match as one client has them.
//!
//! Follows ow174/game/match.py (`_show_players`, `_show_possession`,
//! `remote_body_create`, `ensure_remote_body`, `remote_movements`,
//! `destroy_for_others`), ow174/game/observers.py (other bodies' H=0
//! statescript) and the 20308 lines of ow174/game/notices.py.
//!
//! Every tick the match builds a read-only [`PlayerView`] of each player in
//! the room from the peers, and each client's [`Others`] brings its client up
//! to date from those views:
//! - each other player's card (20308) and controller entity, his team list
//!   entry (288D) after the entity arrived and again when his pick changes,
//!   and his possession (44/75) once the client has his entity and body;
//! - each other player's body: its create, a movement record in every send
//!   frame from the frame after the create, its statescript as H=0 frames
//!   once the create arrived (a full frame, then deltas), and its destroy
//!   when he switches heroes or leaves;
//! - "left the game" and the controller entity's destroy when he leaves.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io, mem,
    time::Duration,
};

use crate::{
    body::{BodyCreate, BodyView, body_create, remote_movement},
    notices::{Switch, card_message, card_record, joined_message, left_message},
    pools::PoolCatalog,
    replication::{BODY_REUSE_DELAY, BodyId, Lifetime, kill_feed_fresh},
    scene::{PlayerAppearance, PlayerCard, Possession, player_create, player_team_bit},
};
use orpheus_content::{heroes::Hero, maps::GameMap};
use orpheus_core::bits::BitWriter;
use orpheus_script::{
    codecs::{Codec, CodecTable},
    graph::{GraphStore, Node},
    snapshot::{ProjectedSnapshot, SnapshotEncoder},
    stream::{BodyStream, DeltaKind, ScriptChunk, Stream},
    vitals::Vitals,
    vm::Snapshot,
    wire::WireError,
};
use orpheus_wire::{
    entity::{EntityUpdate, MAX_PAYLOAD, Movement, Operation, SkinTheme, frame},
    jam::codec::Fields,
    messages::Message,
};

/// A card is about 90 bytes, and a datagram's payload at most 1230: a few
/// new players per tick.
const NEW_PLAYERS_PER_TICK: usize = 3;
/// Each card goes once more 2 s later, which redraws the team list after the
/// portrait frames loaded.
const CARD_AGAIN_TICKS: u32 = 125;
/// A delta of values only (no state on or off) goes at most this often.
const LAZY_TICKS: u32 = 8;

/// What must reach the other clients at once (observers.py `_activity`): the
/// instances, the remote states that are on, and the StEC states, whose
/// counters start them again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObserverActivity {
    instances: BTreeSet<u16>,
    on: BTreeSet<(u16, u16)>,
    stec: BTreeSet<((u16, u16), bool, u16, u64)>,
}

impl ObserverActivity {
    /// The remote-synced states as an observer's projection keeps them: no
    /// HUD presenters, which no other client draws for this body.
    pub(crate) fn of(snapshot: &Snapshot, graphs: &GraphStore, codecs: &CodecTable) -> Self {
        let mut activity = Self::default();
        for (&id, instance) in &snapshot.instances {
            activity.instances.insert(id);
            let Some(graph) = graphs.graph(instance.graph) else {
                continue;
            };
            for (&raw, state) in &instance.states {
                let Some(node) = graph.state(usize::from(raw)) else {
                    continue;
                };
                if !node.is_networked()
                    || presenter(node)
                    || !graph.remote_positions().contains(&node.pos)
                {
                    continue;
                }
                if state.active {
                    activity.on.insert((id, raw));
                }
                if codecs.codec(&node.class).is_some_and(Codec::stec)
                    && (state.active || state.activation != 0)
                {
                    activity.stec.insert((
                        (id, raw),
                        state.active,
                        state.activation,
                        state.start_ms,
                    ));
                }
            }
        }
        activity
    }
}

/// A HUD presenter (UXPresenter, or a screen m_45216F79): the owner's HUD.
fn presenter(node: &Node) -> bool {
    node.class == "STUStatescriptStateUXPresenter" || node.fields.get("m_45216F79").is_some()
}

/// One player of the room as the match sees him this tick.
pub(crate) struct PlayerView<'a> {
    pub(crate) entity: u32,
    pub(crate) slot: i8,
    pub(crate) team: u8,
    pub(crate) card: &'a PlayerCard,
    /// The tick he came into the world; None while he loads or after he left.
    pub(crate) entered: Option<u32>,
    /// His controller frame went out (match.py `steps_done >= 4`).
    pub(crate) presented: bool,
    /// His body, once he picked a hero, while he is in the world.
    pub(crate) body: Option<RemoteBody<'a>>,
    /// His last hero switch.
    pub(crate) switch: Option<Switch>,
}

impl<'a> PlayerView<'a> {
    /// A player whose game has not connected, or has left.
    pub(crate) fn absent(player: &PlayerAppearance<'a>) -> Self {
        Self {
            entity: player.entity,
            slot: player.slot,
            team: player.team,
            card: player.card,
            entered: None,
            presented: false,
            body: None,
            switch: None,
        }
    }
}

pub(crate) struct RemoteBody<'a> {
    pub id: BodyId,
    pub hero: &'a Hero,
    pub skin: SkinTheme,
    /// The server's copy of the body's movement state.
    pub pose: Movement,
    /// Its current pools; None = the hero's own.
    pub vitals: Option<&'a Vitals>,
    /// Counts the changes of its pools: a client that has another count
    /// gets them (combat.py `_send_health`).
    pub health: u64,
    /// Its death, while it is dead.
    pub death: Option<RemoteDeath>,
    /// Its statescript, on send ticks, when another client takes its frames.
    pub script: Option<ObservedScript<'a>>,
}

/// A body's death as the kill feed needs it (combat.py `Vitals.death_tick`,
/// `killer`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RemoteDeath {
    pub tick: u32,
    pub killer: u32,
}

/// What one client can show besides the other players (combat.py
/// `kill_feed_allowed`, server.py `combat_identity_ready`): whether its mode
/// UI arrived, its own current body, and the other entities it has.
#[derive(Clone, Copy, Default)]
pub(crate) struct ViewerReadiness<'a> {
    pub mode_ready: bool,
    pub own_body: Option<u32>,
    pub created: Option<&'a HashSet<u32>>,
}

impl ViewerReadiness<'_> {
    /// The viewer's own body or an entity it has; None for anything else
    /// (another player's body, which [`Others`] answers).
    fn ready(&self, entity: u32) -> bool {
        self.own_body == Some(entity)
            || self
                .created
                .is_some_and(|created| created.contains(&entity))
    }
}

/// A body's statescript as other clients get it (observers.py `snapshot`):
/// projected once per tick for full frames, and from each client's
/// acknowledged baseline for its deltas.
pub(crate) struct ObservedScript<'a> {
    pub encoder: SnapshotEncoder<'a>,
    pub snapshot: &'a Snapshot,
    pub projected: ProjectedSnapshot,
    pub activity: Option<ObserverActivity>,
    pub frame_ms: u32,
}

/// What the match knows for every client's update this tick.
pub(crate) struct ShowContext<'a> {
    pub map: &'a GameMap,
    pub pools: &'a PoolCatalog,
    /// 288D's plain full frame for a pick (heroselect.py `team_entry_frame`).
    pub entry: &'a dyn Fn(Option<u64>) -> Result<BitWriter, WireError>,
    pub tick: u32,
    /// This tick sends datagrams: statescript frames and movement records.
    pub frames: bool,
    /// The mode posts hero switches in the chat (notices.py).
    pub switch_notices: bool,
}

#[derive(Default)]
pub(crate) struct ShowOutput {
    /// Entity records in queue order.
    pub records: Vec<(EntityUpdate, OtherDelivery)>,
    /// 20308 messages for the reliable channel.
    pub cards: Vec<Message>,
    /// Teammates' hero switches to show in the chat.
    pub switches: Vec<Switch>,
}

/// The (body, hero, skin theme) another player's 44 and 75 name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Held {
    body: BodyId,
    hero: u64,
    skin: u64,
}

/// The delivery of a record about another player: each one stays current
/// only for the lifetime it was made for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OtherDelivery {
    Controller {
        serial: u64,
    },
    ControllerDestroy {
        serial: u64,
    },
    Entry {
        serial: u64,
        last: u32,
    },
    Possession {
        serial: u64,
        held: Held,
    },
    Body(BodyId),
    BodyDestroy(BodyId),
    Script {
        body: BodyId,
        last: u32,
    },
    /// The body's pools (combat.py `_send_health`), current while no newer
    /// change went out.
    Health {
        body: BodyId,
        revision: u64,
    },
}

/// Another player's controller entity on this client (match.py ShownController).
struct Shown {
    serial: u64,
    card: Fields,
    create_acked: bool,
    /// Its statescript on this client: plain (H=0) full frames of 288D.
    entry: Stream,
    /// The pick its last entry frame carried.
    pick: Option<Option<u64>>,
    card_again: Option<u32>,
    possession: Option<Held>,
    /// The body of the possession this client acknowledged (server.py
    /// `_confirmed_possessions`).
    confirmed: Option<BodyId>,
    /// The serial of his last hero switch this client took.
    switch: Option<u64>,
}

/// Another player's body on this client (match.py `viewer.seen`).
struct Seen {
    id: BodyId,
    lifetime: Lifetime,
    /// Its create went out: movement records may follow.
    sent: bool,
    /// Its create arrived: possession and statescript may follow.
    arrived: bool,
    watch: Option<Watch>,
    /// The pool changes this client got (the create's, then updates).
    health: u64,
}

/// The body's statescript on this client (observers.py Watch).
struct Watch {
    stream: BodyStream<ProjectedSnapshot>,
    sent: Option<ObserverActivity>,
    next_lazy: u32,
}

#[derive(Default)]
pub(crate) struct Others {
    card_sent: bool,
    card_again: Option<u32>,
    shown: BTreeMap<u32, Shown>,
    /// Controller entities whose destroy has not arrived, by its serial.
    destroyed: HashMap<u32, u64>,
    seen: HashMap<u32, Seen>,
    serial: u64,
    /// This send tick's movement records of the other bodies.
    movements: Vec<EntityUpdate>,
}

impl Others {
    pub(crate) fn movements(&self) -> &[EntityUpdate] {
        &self.movements
    }

    /// Whether this client may still reference a body under this entity
    /// (server.py `body_id_available`): until 30 s after its destroy arrived.
    pub(crate) fn retains(&self, entity: u32, now: Duration) -> bool {
        self.seen
            .get(&entity)
            .is_some_and(|seen| match seen.lifetime {
                Lifetime::ReusableAt(at) => now < at,
                Lifetime::Live | Lifetime::Retiring => true,
            })
    }

    pub(crate) fn retained(&self, owner: u32, now: Duration) -> impl Iterator<Item = u32> + '_ {
        self.seen
            .keys()
            .copied()
            .filter(move |&entity| entity & !0xff == owner && self.retains(entity, now))
    }

    pub(crate) fn current(&self, delivery: OtherDelivery, entity: u32) -> bool {
        let shown = |serial| {
            self.shown
                .get(&entity)
                .is_some_and(|shown| shown.serial == serial)
        };
        let seen = |id: BodyId, lifetime| {
            self.seen
                .get(&id.entity)
                .is_some_and(|seen| seen.id == id && seen.lifetime == lifetime)
        };
        match delivery {
            OtherDelivery::Controller { serial } | OtherDelivery::Entry { serial, .. } => {
                shown(serial)
            }
            OtherDelivery::ControllerDestroy { serial } => {
                self.destroyed.get(&entity) == Some(&serial)
            }
            OtherDelivery::Possession { serial, held } => self
                .shown
                .get(&entity)
                .is_some_and(|shown| shown.serial == serial && shown.possession == Some(held)),
            OtherDelivery::Body(id) => seen(id, Lifetime::Live),
            OtherDelivery::BodyDestroy(id) => seen(id, Lifetime::Retiring),
            OtherDelivery::Script { body, .. } => {
                seen(body, Lifetime::Live) && self.seen[&body.entity].watch.is_some()
            }
            OtherDelivery::Health { body, revision } => {
                seen(body, Lifetime::Live) && self.seen[&body.entity].health == revision
            }
        }
    }

    /// A body's statescript chunk goes once its create arrived.
    pub(crate) fn ready(&self, delivery: OtherDelivery) -> bool {
        match delivery {
            OtherDelivery::Script { body, last } => self
                .watch(body)
                .is_some_and(|watch| watch.stream.ready_to_send(last)),
            _ => true,
        }
    }

    /// The record went out in a sent datagram (match.py `sent_updates`).
    pub(crate) fn sent(&mut self, delivery: OtherDelivery) {
        match delivery {
            OtherDelivery::Body(id) => {
                if let Some(seen) = self.seen.get_mut(&id.entity).filter(|seen| seen.id == id) {
                    seen.sent = true;
                }
            }
            OtherDelivery::Script { body, last } => {
                if let Some(watch) = self.watch_mut(body) {
                    watch.stream.sent(last);
                }
            }
            _ => {}
        }
    }

    /// Apply a current record's transport outcome. Returns whether to send it
    /// again.
    pub(crate) fn delivered(
        &mut self,
        delivery: OtherDelivery,
        entity: u32,
        arrived: bool,
        now: Duration,
    ) -> bool {
        if let OtherDelivery::Script { body, last } = delivery {
            let Some(watch) = self.watch_mut(body) else {
                return false;
            };
            return if arrived {
                watch.stream.arrived(last);
                false
            } else {
                watch.stream.lost(last).retry
            };
        }
        if !arrived {
            return true;
        }
        match delivery {
            OtherDelivery::Controller { .. } => {
                if let Some(shown) = self.shown.get_mut(&entity) {
                    shown.create_acked = true;
                }
            }
            OtherDelivery::ControllerDestroy { .. } => {
                self.destroyed.remove(&entity);
            }
            OtherDelivery::Entry { last, .. } => {
                if let Some(shown) = self.shown.get_mut(&entity) {
                    shown.entry.arrived(last);
                }
            }
            OtherDelivery::Possession { serial, held } => {
                if let Some(shown) = self
                    .shown
                    .get_mut(&entity)
                    .filter(|shown| shown.serial == serial)
                {
                    shown.confirmed = Some(held.body);
                }
            }
            OtherDelivery::Script { .. } | OtherDelivery::Health { .. } => {}
            OtherDelivery::Body(id) => {
                if let Some(seen) = self.seen.get_mut(&id.entity) {
                    seen.arrived = true;
                    if let Some(watch) = &mut seen.watch {
                        watch.stream.create_arrived();
                    }
                }
            }
            OtherDelivery::BodyDestroy(id) => {
                if let Some(seen) = self.seen.get_mut(&id.entity) {
                    seen.lifetime = Lifetime::ReusableAt(now.saturating_add(BODY_REUSE_DELAY));
                }
            }
        }
        false
    }

    /// Whether this client takes the body's statescript frames: its create
    /// arrived. Only then does the match project the body for observers.
    pub(crate) fn observes(&self, body: BodyId) -> bool {
        self.seen
            .get(&body.entity)
            .is_some_and(|seen| seen.id == body && seen.arrived && seen.watch.is_some())
    }

    fn watch(&self, body: BodyId) -> Option<&Watch> {
        self.seen
            .get(&body.entity)
            .filter(|seen| seen.id == body && seen.lifetime == Lifetime::Live)?
            .watch
            .as_ref()
    }

    fn watch_mut(&mut self, body: BodyId) -> Option<&mut Watch> {
        self.seen
            .get_mut(&body.entity)
            .filter(|seen| seen.id == body && seen.lifetime == Lifetime::Live)?
            .watch
            .as_mut()
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// Whether this client may name another player's body in a combat
    /// notice (server.py `combat_identity_ready`: once the player's
    /// possession of it arrived). None for an entity that is not another
    /// player's body.
    pub(crate) fn possession_confirmed(&self, entity: u32) -> Option<bool> {
        self.shown
            .get(&(entity & !0xff))
            .map(|shown| shown.confirmed.is_some_and(|body| body.entity == entity))
    }

    fn identity_ready(&self, entity: u32, readiness: &ViewerReadiness<'_>) -> bool {
        self.possession_confirmed(entity)
            .unwrap_or_else(|| readiness.ready(entity))
    }

    /// Bring the client of `room[viewer]` up to date for this tick, in the
    /// order of match.py `update`: what the players did since the last tick
    /// (switches and leaves), the bodies' statescript, the cards and
    /// controller entities, then the bodies and their health.
    pub(crate) fn show(
        &mut self,
        context: &ShowContext<'_>,
        room: &[PlayerView<'_>],
        viewer: usize,
        readiness: &ViewerReadiness<'_>,
    ) -> io::Result<ShowOutput> {
        let mut output = ShowOutput::default();
        self.movements.clear();
        let me = &room[viewer];
        let Some(entered) = me.entered else {
            return Ok(output);
        };
        let others = || {
            room.iter()
                .enumerate()
                .filter(move |&(slot, _)| slot != viewer)
                .map(|(_, other)| other)
        };
        for other in others() {
            self.departed(other, &mut output);
        }
        if context.frames {
            for other in others() {
                self.observe(context, other, readiness, &mut output);
            }
        }
        if !self.card_sent {
            self.card_sent = true;
            self.card_again = Some(context.tick.saturating_add(CARD_AGAIN_TICKS));
            output.cards.push(card_message(card_record(me.card)));
        } else if self.card_again.is_some_and(|again| context.tick >= again) {
            self.card_again = None;
            output.cards.push(card_message(card_record(me.card)));
        }
        let mut cards = 0;
        for other in others() {
            self.show_player(context, me, entered, other, &mut cards, &mut output)?;
        }
        for other in others() {
            self.ensure_body(context, other, &mut output)?;
        }
        for other in others() {
            self.send_health(other, &mut output);
        }
        if context.frames {
            self.movements = others()
                .filter_map(|other| {
                    let body = other.body.as_ref()?;
                    let seen = self.seen.get(&body.id.entity)?;
                    (seen.id == body.id && seen.lifetime == Lifetime::Live && seen.sent).then(
                        || EntityUpdate {
                            entity: body.id.entity,
                            movement: Some(remote_movement(body.pose, context.tick)),
                            ..EntityUpdate::default()
                        },
                    )
                })
                .collect();
        }
        Ok(output)
    }

    /// A player who left: "left the game" (notices.py `left`); and a body that
    /// is no longer his current one goes (match.py `destroy_for_others`).
    fn departed(&mut self, other: &PlayerView<'_>, output: &mut ShowOutput) {
        if other.entered.is_none() {
            if let Some(shown) = self.shown.get(&other.entity) {
                output.cards.push(left_message(shown.card.clone()));
            }
        }
        let current = other
            .body
            .as_ref()
            .filter(|_| other.entered.is_some())
            .map(|body| body.id);
        let mut retired: Vec<_> = self
            .seen
            .values_mut()
            .filter(|seen| {
                seen.id.entity & !0xff == other.entity
                    && seen.lifetime == Lifetime::Live
                    && Some(seen.id) != current
            })
            .map(|seen| {
                seen.lifetime = Lifetime::Retiring;
                if let Some(watch) = &mut seen.watch {
                    watch.stream.retire();
                }
                seen.watch = None;
                seen.id
            })
            .collect();
        retired.sort_unstable_by_key(|id| id.entity);
        for id in retired {
            if let Some(shown) = self.shown.get_mut(&other.entity) {
                if shown.possession.is_some_and(|held| held.body == id) {
                    shown.possession = None;
                }
            }
            output.records.push((
                EntityUpdate {
                    entity: id.entity,
                    operation: Some(Operation::Destroy),
                    ..EntityUpdate::default()
                },
                OtherDelivery::BodyDestroy(id),
            ));
        }
    }

    /// The H=0 statescript frames of another player's body (observers.py
    /// `Observers._frames`): a full frame once its create arrived, then a
    /// delta from what this client acknowledged, at once when the activity
    /// changed, else at most every LAZY_TICKS.
    fn observe(
        &mut self,
        context: &ShowContext<'_>,
        other: &PlayerView<'_>,
        readiness: &ViewerReadiness<'_>,
        output: &mut ShowOutput,
    ) {
        let Some((body, script)) = other
            .body
            .as_ref()
            .and_then(|body| Some((body.id, body.script.as_ref()?)))
        else {
            return;
        };
        // A dead body's kill feed pulse waits for what this client can show
        // (observers.py `_frames`: `kill_feed_snapshot`, combat.py
        // `kill_feed_allowed`).
        let kill_feed = other
            .body
            .as_ref()
            .and_then(|body| body.death)
            .map(|death| {
                readiness.mode_ready
                    && kill_feed_fresh(context.tick, death.tick)
                    && self.identity_ready(body.entity, readiness)
                    && self.identity_ready(death.killer, readiness)
            });
        let Some(seen) = self
            .seen
            .get_mut(&body.entity)
            .filter(|seen| seen.id == body && seen.lifetime == Lifetime::Live && seen.arrived)
        else {
            return;
        };
        let Some(watch) = &mut seen.watch else {
            return;
        };
        let tick = context.tick;
        let chunk = if watch.stream.last() == 0 {
            script
                .encoder
                .queue_full(&mut watch.stream, &script.projected)
                .map(Some)
        } else {
            let Some((_, base)) = watch.stream.base() else {
                return; // the full frame is still on its way
            };
            if script
                .activity
                .as_ref()
                .is_some_and(|activity| watch.sent.as_ref() == Some(activity))
                && tick < watch.next_lazy
            {
                return;
            }
            script
                .encoder
                .project_next(script.snapshot, base)
                .map(|next| match kill_feed {
                    Some(allowed) => next.gate_kill_feed(&watch.stream, allowed),
                    None => next,
                })
                .and_then(|next| {
                    script.encoder.queue_delta(
                        &mut watch.stream,
                        &next,
                        DeltaKind::Observer { resend: false },
                        script.frame_ms,
                    )
                })
        };
        let chunk: Result<Option<ScriptChunk>, String> = chunk.map_err(|error| error.to_string());
        match chunk.and_then(|chunk| {
            let Some(chunk) = chunk else {
                return Ok(None);
            };
            let packet = frame(u32::MAX, u32::MAX, &[], &[], &[&chunk.update])
                .map_err(|error| error.to_string())?;
            if packet.len() > MAX_PAYLOAD {
                return Err("chunk exceeds the game datagram budget".into());
            }
            Ok(Some(chunk))
        }) {
            Ok(Some(chunk)) => {
                watch.sent = script.activity.clone();
                watch.next_lazy = tick.saturating_add(LAZY_TICKS);
                output.records.push((
                    chunk.update,
                    OtherDelivery::Script {
                        body,
                        last: chunk.last,
                    },
                ));
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "body {:08X}'s frames for another client failed ({error})",
                    body.entity
                );
                watch.stream.retire();
                seen.watch = None;
            }
        }
    }

    /// match.py `_show_players` for one other player: his card and entity,
    /// a frame with his team list entry and pick after the entity arrived,
    /// sent again when the pick changes, and his possession. A player who
    /// left goes.
    fn show_player(
        &mut self,
        context: &ShowContext<'_>,
        me: &PlayerView<'_>,
        entered: u32,
        other: &PlayerView<'_>,
        cards: &mut usize,
        output: &mut ShowOutput,
    ) -> io::Result<()> {
        let Some(other_entered) = other.entered else {
            if let Some(shown) = self.shown.remove(&other.entity) {
                self.destroyed.insert(other.entity, shown.serial);
                output.records.push((
                    EntityUpdate {
                        entity: other.entity,
                        operation: Some(Operation::Destroy),
                        ..EntityUpdate::default()
                    },
                    OtherDelivery::ControllerDestroy {
                        serial: shown.serial,
                    },
                ));
            }
            return Ok(());
        };
        if !self.shown.contains_key(&other.entity) {
            if *cards == NEW_PLAYERS_PER_TICK {
                return Ok(());
            }
            *cards += 1;
            let create = player_create(
                context.map,
                &PlayerAppearance {
                    entity: other.entity,
                    slot: other.slot,
                    team: other.team,
                    spawn: 0,
                    card: other.card,
                },
                None,
                None,
            )?;
            let serial = self.next_serial();
            let card = card_record(other.card);
            // "Joined the game" for a player who came into the world after
            // this one (notices.py `card`).
            output.cards.push(if other_entered > entered {
                joined_message(card.clone())
            } else {
                card_message(card.clone())
            });
            self.destroyed.remove(&other.entity);
            self.shown.insert(
                other.entity,
                Shown {
                    serial,
                    card,
                    create_acked: false,
                    entry: Stream::new(other.entity),
                    pick: None,
                    card_again: Some(context.tick.saturating_add(CARD_AGAIN_TICKS)),
                    possession: None,
                    confirmed: None,
                    switch: other.switch.map(|switch| switch.serial),
                },
            );
            output
                .records
                .push((create, OtherDelivery::Controller { serial }));
            return Ok(());
        }
        let shown = self.shown.get_mut(&other.entity).expect("shown player");
        if shown
            .card_again
            .is_some_and(|again| context.tick >= again && *cards < NEW_PLAYERS_PER_TICK)
        {
            *cards += 1;
            shown.card_again = None;
            output.cards.push(card_message(shown.card.clone()));
        }
        let pick = other.body.as_ref().map(|body| body.hero.guid);
        if shown.create_acked && (shown.entry.latest_data() == 0 || shown.pick != Some(pick)) {
            let payload = (context.entry)(pick).map_err(invalid)?;
            let chunk = shown.entry.full(&payload).map_err(invalid)?;
            shown.pick = Some(pick);
            output.records.push((
                chunk.update,
                OtherDelivery::Entry {
                    serial: shown.serial,
                    last: chunk.last,
                },
            ));
        }
        if let Some(switch) = other.switch {
            if shown.switch != Some(switch.serial) {
                shown.switch = Some(switch.serial);
                // notices.py `hero_switched`: teammates whose clients have his
                // entity, once their own controller went out.
                if context.switch_notices
                    && !context.map.free_for_all
                    && me.team == other.team
                    && me.presented
                {
                    output.switches.push(switch);
                }
            }
        }
        // match.py `_show_possession`: once this client has his entity and
        // body, and again after a switch.
        let Some(body) = other.body.as_ref() else {
            return Ok(());
        };
        if !shown.create_acked
            || !self.seen.get(&body.id.entity).is_some_and(|seen| {
                seen.id == body.id && seen.lifetime == Lifetime::Live && seen.arrived
            })
        {
            return Ok(());
        }
        let held = Held {
            body: body.id,
            hero: body.hero.guid,
            skin: body.skin.guid,
        };
        if shown.possession == Some(held) {
            return Ok(());
        }
        shown.possession = Some(held);
        let possession = Possession {
            body: body.id.entity,
            hero: body.hero.guid,
            skin: (body.skin.guid != 0).then_some(body.skin.guid),
        };
        output.records.push((
            EntityUpdate {
                entity: other.entity,
                operation: Some(Operation::Update(possession.components().into())),
                ..EntityUpdate::default()
            },
            OtherDelivery::Possession {
                serial: shown.serial,
                held,
            },
        ));
        Ok(())
    }

    /// match.py `ensure_remote_body`: his current body's create, once. It is
    /// placed by its transform; movement records move it from the next frame
    /// on (a movement state in the create would sit at frame 0xFFFFFFFE, and
    /// the client refuses every later record that is not newer).
    fn ensure_body(
        &mut self,
        context: &ShowContext<'_>,
        other: &PlayerView<'_>,
        output: &mut ShowOutput,
    ) -> io::Result<()> {
        let Some(body) = other.body.as_ref().filter(|_| other.entered.is_some()) else {
            return Ok(());
        };
        if let Some(seen) = self.seen.get(&body.id.entity) {
            // An entity the client may still hold under an older lifetime
            // waits; the match picks body ids no client retains.
            if seen.id == body.id || matches!(seen.lifetime, Lifetime::Live | Lifetime::Retiring) {
                return Ok(());
            }
        }
        let create = body_create(
            BodyCreate {
                entity: body.id.entity,
                hero: body.hero,
                team_bit: player_team_bit(context.map, other.team)?,
                position: body.pose.position,
                yaw: body.pose.yaw,
                skin: body.skin,
                health: body.vitals.map(Vitals::component),
            },
            BodyView::Observer,
            context.pools,
        );
        self.seen.insert(
            body.id.entity,
            Seen {
                id: body.id,
                lifetime: Lifetime::Live,
                sent: false,
                arrived: false,
                health: body.health,
                watch: Some(Watch {
                    stream: BodyStream::new(body.id.entity),
                    sent: None,
                    next_lazy: 0,
                }),
            },
        );
        output.records.push((create, OtherDelivery::Body(body.id)));
        Ok(())
    }

    /// combat.py `_send_health`: the body's pools when they changed since
    /// what this client has, once its create arrived.
    fn send_health(&mut self, other: &PlayerView<'_>, output: &mut ShowOutput) {
        let Some(body) = other.body.as_ref().filter(|_| other.entered.is_some()) else {
            return;
        };
        let Some(vitals) = body.vitals else {
            return;
        };
        let Some(seen) = self
            .seen
            .get_mut(&body.id.entity)
            .filter(|seen| seen.id == body.id && seen.lifetime == Lifetime::Live && seen.arrived)
        else {
            return;
        };
        if seen.health == body.health {
            return;
        }
        seen.health = body.health;
        output.records.push((
            EntityUpdate {
                entity: body.id.entity,
                operation: Some(Operation::Update(vec![vitals.component()])),
                ..EntityUpdate::default()
            },
            OtherDelivery::Health {
                body: body.id,
                revision: body.health,
            },
        ));
    }

    /// Everything this client had of the others is gone with its world.
    pub(crate) fn clear(&mut self) {
        let serial = self.serial;
        *self = Self {
            serial,
            ..Self::default()
        };
    }

    /// Take the bookkeeping out while the room's views borrow the peers.
    pub(crate) fn take(&mut self) -> Self {
        mem::take(self)
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_content::heroes::SOLDIER;
    use orpheus_wire::{entity::NO_INPUT, jam::codec::Value};

    const VIEWER: u32 = 0xa000_0100;
    const OTHER: u32 = 0xa000_0200;

    fn hero() -> Hero {
        Hero {
            guid: SOLDIER,
            name: "Soldier: 76".into(),
            role: "Damage".into(),
            body: 0x0400_0000_0000_03cf,
        }
    }

    fn card(tag: &str) -> PlayerCard {
        PlayerCard {
            battle_tag: tag.into(),
            ..PlayerCard::default()
        }
    }

    fn view<'a>(
        entity: u32,
        slot: i8,
        card: &'a PlayerCard,
        entered: Option<u32>,
    ) -> PlayerView<'a> {
        PlayerView {
            entity,
            slot,
            team: 0,
            card,
            entered,
            presented: true,
            body: None,
            switch: None,
        }
    }

    fn body(hero: &Hero, suffix: u32, generation: u64) -> RemoteBody<'_> {
        RemoteBody {
            id: BodyId {
                entity: OTHER | suffix,
                generation,
            },
            hero,
            skin: SkinTheme::default(),
            pose: Movement::default(),
            vitals: None,
            health: 0,
            death: None,
            script: None,
        }
    }

    fn summary(output: &ShowOutput) -> Vec<(u32, &'static str)> {
        output
            .records
            .iter()
            .map(|(update, delivery)| {
                let kind = match delivery {
                    OtherDelivery::Controller { .. } => "create",
                    OtherDelivery::ControllerDestroy { .. } => "destroy",
                    OtherDelivery::Entry { .. } => "entry",
                    OtherDelivery::Possession { .. } => "possession",
                    OtherDelivery::Body(_) => "body",
                    OtherDelivery::BodyDestroy(_) => "body destroy",
                    OtherDelivery::Script { .. } => "script",
                    OtherDelivery::Health { .. } => "health",
                };
                (update.entity, kind)
            })
            .collect()
    }

    fn arrive(others: &mut Others, output: &ShowOutput) {
        for (update, delivery) in &output.records {
            assert!(others.current(*delivery, update.entity));
            others.sent(*delivery);
            others.delivered(*delivery, update.entity, true, Duration::ZERO);
        }
    }

    fn flag(message: &Message) -> bool {
        message.fields[&0x78] == Value::Bool(true)
    }

    #[test]
    fn another_player_comes_switches_and_leaves_in_match_order() {
        let map = GameMap::practice_range();
        let pools = PoolCatalog::default();
        let entry = |_| Ok(BitWriter::new());
        let mut context = ShowContext {
            map: &map,
            pools: &pools,
            entry: &entry,
            tick: 1010,
            frames: true,
            switch_notices: false,
        };
        let soldier = hero();
        let (mine, theirs) = (card("viewer#1"), card("other#2"));
        let mut room = [
            view(VIEWER, 0, &mine, Some(1000)),
            view(OTHER, 1, &theirs, Some(1005)),
        ];
        room[1].body = Some(body(&soldier, 1, 1));
        let mut others = Others::default();
        let ready = ViewerReadiness::default();

        // The own card, the other's card ("joined": he came later), his
        // entity and his body; no movement before the body's create went out.
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        assert_eq!(output.cards.len(), 2);
        assert!(!flag(&output.cards[0]) && flag(&output.cards[1]));
        assert_eq!(summary(&output), [(OTHER, "create"), (OTHER | 1, "body")]);
        assert!(others.movements().is_empty());
        arrive(&mut others, &output);

        // Both arrived: his team list entry and his possession; a movement
        // record from now on.
        context.tick += 3;
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        assert!(output.cards.is_empty());
        assert_eq!(summary(&output), [(OTHER, "entry"), (OTHER, "possession")]);
        let [movement] = others.movements() else {
            panic!("one movement record");
        };
        assert_eq!(movement.entity, OTHER | 1);
        assert_eq!(movement.movement.unwrap().input_frame, Some(NO_INPUT));
        arrive(&mut others, &output);
        context.tick += 3;
        assert!(summary(&others.show(&context, &room, 0, &ready).unwrap()).is_empty());

        // A switch: the old body goes, the new one comes, and once it arrived
        // its possession and the new pick.
        room[1].body = Some(body(&soldier, 2, 2));
        context.tick += 3;
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        assert_eq!(
            summary(&output),
            [(OTHER | 1, "body destroy"), (OTHER | 2, "body")]
        );
        assert!(others.retains(OTHER | 1, Duration::ZERO));
        arrive(&mut others, &output);
        assert!(others.retains(OTHER | 1, Duration::ZERO));
        assert!(!others.retains(OTHER | 1, BODY_REUSE_DELAY));
        context.tick += 3;
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        assert_eq!(summary(&output), [(OTHER, "possession")]);
        arrive(&mut others, &output);

        // He leaves: "left the game", then his body and his entity go.
        room[1].entered = None;
        context.tick += 3;
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        let [left] = &output.cards[..] else {
            panic!("one left notice");
        };
        assert_eq!(left.fields[&0x79], Value::Bool(false));
        assert_eq!(
            summary(&output),
            [(OTHER | 2, "body destroy"), (OTHER, "destroy")]
        );
        assert!(others.movements().is_empty());
        arrive(&mut others, &output);
        context.tick += 3;
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        assert!(output.cards.is_empty() && output.records.is_empty());
    }

    #[test]
    fn teammates_take_each_hero_switch_once() {
        let map = GameMap::practice_range();
        let pools = PoolCatalog::default();
        let entry = |_| Ok(BitWriter::new());
        let context = ShowContext {
            map: &map,
            pools: &pools,
            entry: &entry,
            tick: 1000,
            frames: false,
            switch_notices: true,
        };
        let switch = |serial| Switch {
            old: 1,
            new: 2,
            player: OTHER,
            serial,
        };
        let (mine, theirs) = (card("viewer#1"), card("other#2"));
        let mut room = [
            view(VIEWER, 0, &mine, Some(1000)),
            view(OTHER, 1, &theirs, Some(1000)),
        ];
        // A switch before this client had him is not shown.
        room[1].switch = Some(switch(1));
        let mut others = Others::default();
        let ready = ViewerReadiness::default();
        assert!(
            others
                .show(&context, &room, 0, &ready)
                .unwrap()
                .switches
                .is_empty()
        );
        room[1].switch = Some(switch(2));
        assert_eq!(
            others.show(&context, &room, 0, &ready).unwrap().switches,
            [switch(2)]
        );
        assert!(
            others
                .show(&context, &room, 0, &ready)
                .unwrap()
                .switches
                .is_empty()
        );
        // Enemies get no line.
        room[1].team = 1;
        room[1].switch = Some(switch(3));
        assert!(
            others
                .show(&context, &room, 0, &ready)
                .unwrap()
                .switches
                .is_empty()
        );
    }

    #[test]
    fn new_players_wait_for_a_card_slot_and_cards_go_again() {
        let map = GameMap::practice_range();
        let pools = PoolCatalog::default();
        let entry = |_| Ok(BitWriter::new());
        let mut context = ShowContext {
            map: &map,
            pools: &pools,
            entry: &entry,
            tick: 1000,
            frames: false,
            switch_notices: false,
        };
        let cards: Vec<_> = (0..6).map(|slot| card(&format!("player#{slot}"))).collect();
        let room: Vec<_> = cards
            .iter()
            .enumerate()
            .map(|(slot, card)| {
                view(
                    0xa000_0000 | ((slot as u32 + 1) << 8),
                    slot as i8,
                    card,
                    Some(1000),
                )
            })
            .collect();
        let mut others = Others::default();
        let ready = ViewerReadiness::default();
        let output = others
            .show(&context, &room, 0, &ViewerReadiness::default())
            .unwrap();
        // The own card and three others; the other two the next tick.
        assert_eq!(output.cards.len(), 4);
        assert!(output.cards.iter().all(|card| !flag(card)));
        context.tick += 1;
        assert_eq!(
            others.show(&context, &room, 0, &ready).unwrap().cards.len(),
            2
        );
        context.tick = 1000 + CARD_AGAIN_TICKS;
        assert_eq!(
            others.show(&context, &room, 0, &ready).unwrap().cards.len(),
            4
        );
        context.tick += 1;
        assert_eq!(
            others.show(&context, &room, 0, &ready).unwrap().cards.len(),
            2
        );
    }
}
