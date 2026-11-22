//! A seat's game as its room sees it: the reliable messages and frames over
//! its link, its loading steps, its world and its combat notices.

use std::{io, time::Duration};

use lucy_content::hero_settings;
use lucy_sim::{
    egress::{GameEgress, SendError},
    instance::ownership,
    match_flow::Timers,
    mode_script::ModeRunner,
    replication::WorldDelivery,
};
use lucy_wire::{
    combat_messages::CombatNotice,
    entity::EntityUpdate,
    jam::codec::Schemas,
    link::{ACK, KIND_FRAME, Packet},
    messages::{Message, encode},
};

use super::{CombatUi, Peer, Room, Transport, ai, invalid, leave_message, send};
use crate::{
    connection::GameConnections,
    ingress::{GameIngress, MapReadiness, MapReadyStep, WorldReadiness},
};

/// A client's combat notices wait for what they name at most this many at
/// a time; the oldest go first.
const MAX_PENDING_COMBAT_NOTICES: usize = 256;

impl Peer {
    pub(super) fn new(owner: u32) -> Self {
        Self {
            ingress: GameIngress::default(),
            egress: GameEgress::default(),
            loading: MapReadiness::default(),
            ownership: None,
            open: false,
            world: WorldDelivery::new(owner),
            combat_ui: CombatUi::default(),
            presented: false,
            ai: None,
            reported_latency: None,
            hero_settings: hero_settings::SentList::default(),
            send_failure: None,
            pongs: Vec::new(),
        }
    }

    /// An AI player's peer: a world no client receives, whose bodies his AI
    /// entity possesses, and the controller that plays it.
    pub(super) fn ai(owner: u32, seat: ai::AiSeat) -> Self {
        Self {
            world: WorldDelivery::headless(owner, seat.entity, seat.difficulty),
            ai: Some(Box::new(seat)),
            ..Self::new(owner)
        }
    }

    pub(super) fn clear_world(&mut self) {
        self.world.clear();
        self.combat_ui = CombatUi::default();
        self.presented = false;
    }

    /// A hit's notices for this client (the shooter's).
    pub(super) fn notice(&mut self, notice: CombatNotice) {
        if self.ai.is_some() {
            return;
        }
        let Some(notice) = DROPPED_NOTICES.filter(notice) else {
            return;
        };
        if self.combat_ui.notices.len() == MAX_PENDING_COMBAT_NOTICES {
            self.combat_ui.notices.pop_front();
        }
        self.combat_ui.notices.push_back(notice);
    }

    pub(super) fn flush_combat(&mut self, schemas: &Schemas, now: Duration) {
        for _ in 0..self.combat_ui.notices.len() {
            let notice = self
                .combat_ui
                .notices
                .pop_front()
                .expect("pending combat notice");
            let entity = notice.required_identity();
            if entity.is_some_and(|entity| !self.world.combat_identity_ready(entity))
                || (notice.needs_mode_ui() && !self.world.mode_ready())
            {
                self.combat_ui.notices.push_back(notice);
                continue;
            }
            let notice = match notice {
                CombatNotice::Hit {
                    target,
                    dealt,
                    critical,
                    killed,
                    shot_frame,
                } => {
                    let shot_frame = shot_frame.max(self.combat_ui.last_hit_frame);
                    self.combat_ui.last_hit_frame = shot_frame;
                    CombatNotice::Hit {
                        target,
                        dealt,
                        critical,
                        killed,
                        shot_frame,
                    }
                }
                notice => notice,
            };
            if let Ok(bytes) = notice.encode(schemas) {
                if let (Ok(ordinal), Some(entity)) = (self.egress.queue_reliable(bytes), entity) {
                    self.combat_ui.last_notice_by_entity.insert(entity, ordinal);
                }
            }
        }
        for _ in 0..self.combat_ui.replacements.len() {
            let (expires, replacement) = self
                .combat_ui
                .replacements
                .pop_front()
                .expect("pending bot replacement");
            let entity = replacement.retired;
            let has_notice = self
                .combat_ui
                .notices
                .iter()
                .any(|notice| notice.required_identity() == Some(entity));
            let waiting_for_reliable = self
                .combat_ui
                .last_notice_by_entity
                .get(&entity)
                .is_some_and(|&ordinal| !self.egress.delivered_through(ordinal));
            if now < expires
                && (has_notice
                    || waiting_for_reliable
                    || self.world.health_pending(entity)
                    || self.world.bot_frames_pending(entity))
            {
                self.world.hold_bot_destroy(entity);
                self.combat_ui
                    .replacements
                    .push_back((expires, replacement));
            } else {
                self.combat_ui
                    .notices
                    .retain(|notice| notice.required_identity() != Some(entity));
                self.combat_ui.last_notice_by_entity.remove(&entity);
                self.world.replace_bot(replacement);
            }
        }
    }

    pub(super) fn reliable(&mut self, schemas: &Schemas, message: Message) -> io::Result<u32> {
        let bytes = encode(schemas, message.id, &message.fields).map_err(invalid)?;
        let ordinal = self.egress.queue_reliable(bytes).map_err(invalid)?;
        tracing::debug!(
            target: "lucy::join",
            id = message.id,
            ordinal,
            "reliable message queued"
        );
        Ok(ordinal)
    }

    /// This tick's frame over the peer's link, which leaves at `sent`: the
    /// waiting pongs with that send time, the queued reliable messages and
    /// world records, with these movements. A frame that can't be made (a
    /// record that doesn't encode or fit) is logged when its reason
    /// changes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_frame(
        &mut self,
        schemas: &Schemas,
        links: &mut GameConnections,
        socket: &impl Transport,
        connection: u32,
        tick: u32,
        movements: Vec<EntityUpdate>,
        reliable_only: bool,
        sent: Duration,
    ) {
        for pong in self.pongs.drain(..) {
            let message = pong.sent(sent);
            if let Ok(bytes) = encode(schemas, message.id, &message.fields) {
                let _ = self.egress.queue_reliable(bytes);
            }
        }
        let outcome = self.world.send_frame(
            tick,
            movements,
            reliable_only,
            &mut self.egress,
            |payload| {
                let packet = Packet {
                    conn: connection,
                    kind: KIND_FRAME,
                    flags: ACK,
                    payload,
                    ..Packet::default()
                };
                let outgoing = links
                    .transmit(packet)
                    .map_err(|error| io::Error::other(format!("game link: {error:?}")))?;
                send(socket, outgoing)
            },
        );
        let failure = match outcome {
            Err(SendError::Select(error)) => Some(error.to_string()),
            _ => None,
        };
        if failure.is_some() && failure != self.send_failure {
            tracing::warn!(
                connection,
                tick,
                error = failure.as_deref().unwrap_or_default(),
                "the game frame can't be made; no records go until it can"
            );
        }
        self.send_failure = failure;
    }

    /// The due spawn steps; true when this one put the player in the world.
    fn advance_loading(
        &mut self,
        schemas: &Schemas,
        room: &Room,
        slot: usize,
        now: Duration,
        tick: u32,
    ) -> io::Result<bool> {
        let mut spawned = false;
        let player = &room.players[slot];
        let ready = WorldReadiness {
            ownership_delivered: self
                .ownership
                .is_some_and(|id| self.egress.delivered_through(id)),
            collision_ready: room.world.is_some(),
            bot_scripts_ready: room
                .world
                .as_ref()
                .is_some_and(|world| world.scene.bot_scripts_ready()),
        };
        while let Some(step) = self.loading.take_due(now, ready) {
            match step {
                MapReadyStep::Ownership => {
                    self.ownership = Some(self.reliable(schemas, ownership(player.entity))?);
                }
                MapReadyStep::Game => {
                    self.reliable(schemas, room.message(slot, false))?;
                }
                MapReadyStep::World => {
                    let world = room.world.as_ref().expect("map-entry prerequisites");
                    let viewer = room.appearance(slot).expect("prepared player");
                    self.world.start(&world.scene, &viewer, tick)?;
                    spawned = true;
                }
                MapReadyStep::Controller => {
                    self.world.presentation_due = true;
                    self.presented = true;
                }
            }
        }
        if self.world.presentation_due {
            let world = room.world.as_ref().expect("world started");
            let first = room.first_mode_frame(&world.scene);
            self.world.present(&world.scene, first, tick)?;
        }
        Ok(spawned)
    }

    /// This tick's steps of the player (match.py `update`): the mode
    /// entity's frame and the end of "assemble your team", then the spawn
    /// steps. The first spawn in a PvP mode starts the match flow
    /// (`_spawn_steps`).
    pub(super) fn update(
        &mut self,
        schemas: &Schemas,
        room: &mut Room,
        slot: usize,
        timers: Timers,
        now: Duration,
        tick: u32,
    ) -> io::Result<()> {
        if let (true, Some(frame)) = (self.presented, room.mode_frame_now()) {
            self.world.queue_mode_frame(frame, tick)?;
        }
        // The entities the mode entity's graphs made (the Workshop's
        // effects and icons), after the mode frame that names them.
        if let (true, Some(mode)) = (self.presented, room.mode.as_ref()) {
            self.world.update_mode_entities(mode);
        }
        if let (true, Some(world)) = (room.assembled && self.presented, &room.world) {
            self.world.end_assemble(&world.scene, tick)?;
        }
        self.world.set_assembling(room.assembling());
        self.world.set_mode_graph(room.follows_graph());
        self.world
            .set_mode_held(room.mode.as_ref().map_or(&[], ModeRunner::held));
        let spawned = self.advance_loading(schemas, room, slot, now, tick)?;
        if let Some(mut world) = room.world.take() {
            if spawned && !world.scene.controller().practice() {
                room.begin_flow(&mut world.scene, timers, now, tick);
            }
            room.world = Some(world);
        }
        Ok(())
    }

    /// The player leaves the world: his game goes back to the menu (20304)
    /// and his link closes after it.
    pub(super) fn leave(
        &mut self,
        links: &mut GameConnections,
        schemas: &Schemas,
        connection: u32,
        now: Duration,
    ) {
        self.clear_world();
        self.loading = MapReadiness::default();
        if links.begin_leaving(connection, now) {
            let _ = self.reliable(schemas, leave_message());
        }
    }
}

/// A debugging switch: `LUCY_DEBUG_DROP_NOTICES` lists combat notices the
/// server leaves out (`hit`, `kill-flag` (25001 sent without its kill bit),
/// `elimination` (25005), `death` (25004)), to find which one stalls the
/// client's own body after a kill (leafall903/ORPHEUS issue 14).
#[derive(Default)]
struct DroppedNotices {
    hit: bool,
    kill_flag: bool,
    elimination: bool,
    death: bool,
}

static DROPPED_NOTICES: std::sync::LazyLock<DroppedNotices> = std::sync::LazyLock::new(|| {
    let Ok(list) = std::env::var("LUCY_DEBUG_DROP_NOTICES") else {
        return DroppedNotices::default();
    };
    let mut dropped = DroppedNotices::default();
    for item in list
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        match item {
            "hit" => dropped.hit = true,
            "kill-flag" => dropped.kill_flag = true,
            "elimination" => dropped.elimination = true,
            "death" => dropped.death = true,
            other => tracing::warn!("LUCY_DEBUG_DROP_NOTICES: unknown notice {other:?}"),
        }
    }
    tracing::warn!("debugging: dropping combat notices {list:?}");
    dropped
});

impl DroppedNotices {
    fn filter(&self, notice: CombatNotice) -> Option<CombatNotice> {
        match notice {
            CombatNotice::Hit { .. } if self.hit => None,
            CombatNotice::Hit {
                target,
                dealt,
                critical,
                shot_frame,
                ..
            } if self.kill_flag => Some(CombatNotice::Hit {
                target,
                dealt,
                critical,
                killed: false,
                shot_frame,
            }),
            CombatNotice::Elimination { .. } if self.elimination => None,
            CombatNotice::Death { .. } if self.death => None,
            notice => Some(notice),
        }
    }
}
