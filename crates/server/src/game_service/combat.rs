//! A room's input and combat: its players' commands and shots, the bots'
//! weapons and rockets, the projectiles, auras and the hits they make.

use std::{collections::HashMap, sync::Arc, time::Duration};

use indexmap::IndexMap;
use lucy_script::{ultimate::Dealt, vm::AuraChange};
use lucy_sim::{
    bot_script::{BotShotKind, SeenRocket},
    combat::{
        BotHit, Combat, CombatPlayer, PosedShot, Shooter, ShotContext, apply_player_hits,
        apply_player_pushes, deliver_projectiles, finish_bot_rockets, show_bot_rockets, show_fired,
    },
    instance::RETAIL_QUANTA,
    match_flow::MatchFlow,
    mode_script::ModeRunner,
    projectiles::FiredProjectile,
    replication::{MAX_PENDING_INPUT, WorldDelivery, blockers},
    scene_assets::{FellOut, PreparedScene},
};
use lucy_wire::combat_messages::CombatNotice;

use super::{Peer, Room, RoomState, WorldStart, player_entity};

/// A pass runs at most this many of a player's commands, and of all its
/// players' together, so a busy peer cannot take the next one's turn.
const INPUT_PER_PEER: usize = 16;
const INPUT_PER_TURN: usize = 64;

impl RoomState {
    /// The commands of the tick that is due, `ticks` of them with the ones
    /// the clock skipped: one a player, from his input buffer
    /// (`WorldDelivery::release_input`). An AI player's run as he gives
    /// them.
    pub(super) fn release_input(&mut self, ticks: u32) {
        if self.clock.draining() {
            return;
        }
        for peer in self.peers.values_mut() {
            peer.world.release_input(ticks);
        }
    }

    /// Bounded round-robin input work between receive batches and tick checks.
    /// A busy peer cannot consume the next peer's whole turn.
    pub(super) fn advance_input(&mut self, now: Duration) -> bool {
        let count = self.peers.len();
        if count == 0 || self.clock.draining() {
            return false;
        }
        let start = self.next_input_peer % count;
        let tick = self.clock.frame();
        let mut remaining = INPUT_PER_TURN;
        let mut struck = Vec::new();
        let mut fired = Vec::new();
        for offset in 0..count {
            let index = (start + offset) % count;
            let (&connection, peer) = self.peers.get_index(index).expect("active peer index");
            if !peer.world.has_input_work() {
                continue;
            }
            let slot = self.seats[&connection];
            let retained = self.retained_elsewhere(connection, now);
            // The living enemy bodies of his room, the other players' and
            // the training bots', block his body's moves.
            let blocking = {
                let room = &self.room;
                let world = room.world.as_ref().expect("prepared world with input");
                let others = self
                    .peers
                    .iter()
                    .filter(|&(&other, _)| other != connection)
                    .map(|(other, other_peer)| {
                        (room.players[self.seats[other]].info.team, &other_peer.world)
                    });
                blockers(&world.scene, room.players[slot].info.team, others)
            };
            let (_, peer) = self.peers.get_index_mut(index).expect("active peer index");
            peer.world.retain_elsewhere(retained);
            peer.world.set_blockers(blocking);
            let room = &mut self.room;
            let world = room.world.as_ref().expect("prepared world with input");
            let viewer = room.appearance(slot).expect("prepared player");
            let team = viewer.team;
            let context = ShotContext {
                quanta: room.players[slot].quanta(),
                tick,
                now,
            };
            let budget = remaining.min(INPUT_PER_PEER);
            // His body's searches that rewind see the bodies by the room's
            // history, as his shots do.
            let combat = world.combat.as_ref();
            let name = &room.players[slot].info.name;
            let id = self.id[0] & 0xFFFF;
            let (used, leave) = match peer.world.advance_input(
                &world.scene,
                &viewer,
                context.quanta,
                budget,
                combat,
            ) {
                Ok((used, leave)) => {
                    if leave {
                        // `InputQueue::push`: more than `MAX_PENDING_INPUT`
                        // waited, or a command the queue can't take in turn.
                        tracing::warn!(
                            "[game] match {id:04X}: {name} is sent home: his input queue closed"
                        );
                    }
                    (used, leave)
                }
                Err(error) => {
                    tracing::warn!(
                        "[game] match {id:04X}: {name} is sent home: his input failed: {error}"
                    );
                    (budget, true)
                }
            };
            remaining -= used;
            self.next_input_peer = (index + 1) % count;
            let shots = peer.world.take_shots();
            let projectiles = peer.world.take_fired();
            if !projectiles.is_empty() {
                fired.push(projectiles);
            }
            if (!leave && peer.world.combat_body_active()) || !shots.is_empty() {
                let world = room.world.as_mut().expect("prepared world with combat");
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                // combat.py `Combat.command`: the shots, then the graphs'
                // health changes (quick melee).
                let mut hits = Vec::new();
                for shot in shots {
                    match shot {
                        PosedShot::Hitscan(shot) => {
                            let frame = shot.command.frame;
                            let visible = |entity| peer.world.bot_shown(entity, frame);
                            let shooter = Shooter {
                                source: shot.source,
                                team,
                                bot: false,
                            };
                            if let Ok(Some(hit)) =
                                combat.hitscan(&mut world.scene, shot, shooter, context, visible)
                            {
                                hits.push(hit);
                            }
                        }
                        PosedShot::Health(change) => {
                            let frame = change.frame;
                            let visible = |entity| peer.world.bot_shown(entity, frame);
                            hits.extend(combat.modify_health(
                                &mut world.scene,
                                &change,
                                context,
                                visible,
                            ));
                        }
                        PosedShot::Projectile(_) => {}
                    }
                }
                struck.push((Some(connection), hits));
            }
            if leave {
                peer.leave(&mut self.links, &self.schemas, connection, now);
            }
            if remaining == 0 {
                break;
            }
        }
        for projectiles in fired {
            self.show_fired(&projectiles, tick);
        }
        for (shooter, hits) in struck {
            self.deliver_bot_hits(shooter, hits);
            self.resolve_player_hits(tick, now);
        }
        self.input_waits()
    }

    /// [`Self::advance_input`] until the released input is done
    /// (`ready`: some waits). The released commands are at most a full
    /// queue each (`MAX_PENDING_INPUT`), so the passes are bounded too.
    pub(super) fn advance_released_input(&mut self, now: Duration, mut ready: bool) {
        let passes = self.peers.len() * MAX_PENDING_INPUT / INPUT_PER_TURN + 1;
        for _ in 0..passes {
            if !ready {
                break;
            }
            ready = self.advance_input(now);
        }
    }

    /// Input in hand that [`RoomState::advance_input`] runs: the next pass
    /// of the loop runs at once.
    pub(super) fn input_waits(&self) -> bool {
        !self.clock.draining() && self.peers.values().any(|peer| peer.world.has_input_work())
    }

    /// projectiles.py `_show_others`: the other clients of the room see the
    /// projectiles a player fired.
    fn show_fired(&mut self, projectiles: &[FiredProjectile], tick: u32) {
        let Room { players, world, .. } = &mut self.room;
        let Some(world) = world.as_mut() else {
            return;
        };
        let combat = world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene));
        let mut peers = room_peers(&mut self.peers, &self.seats, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        show_fired(combat, projectiles, &mut worlds, tick);
    }

    /// The room's combat and its clients' worlds, by slot, for the bots'
    /// rocket copies (projectiles.py `_show_others`, `flown`).
    fn bot_rockets(
        &mut self,
        tick: u32,
        apply: impl FnOnce(&mut Combat, &mut [Option<&mut WorldDelivery>], u32),
    ) {
        let Room { players, world, .. } = &mut self.room;
        let Some(combat) = world.as_mut().and_then(|world| world.combat.as_mut()) else {
            return;
        };
        let mut peers = room_peers(&mut self.peers, &self.seats, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        apply(combat, &mut worlds, tick);
    }

    /// The bodies of the player's ring another client of the room still
    /// holds: a new body takes none of them (ow174/game/match.py
    /// `_next_body_id`).
    fn retained_elsewhere(&self, connection: u32, now: Duration) -> Vec<u32> {
        let owner = player_entity(self.seats[&connection]);
        self.peers
            .iter()
            .filter(|&(&other, _)| other != connection)
            .flat_map(|(_, other)| other.world.retained_bodies(owner, now))
            .collect()
    }

    /// combat.py `_damage` for bots and `_send_health`: the shooter's
    /// client gets the notices, every client of the room the bot's pools.
    /// The shooter's ultimate charges for the damage (assumption: training
    /// bots charge it as heroes do) and for the healing of allied bots
    /// (combat.py `_heal`); his board counts both with the room's other
    /// native stats (`Combat::take_tracked_stats`). A bot he kills counts
    /// for his multikills, in the Practice Range too
    /// (`lucy_sim::announcer`).
    fn deliver_bot_hits(&mut self, shooter: Option<u32>, hits: Vec<BotHit>) {
        if let Some(world) = self.room.world.as_mut() {
            for hit in &hits {
                bot_hit_script(&mut world.scene, hit);
            }
        }
        // Each hit to the shooter's body as damage dealt or healing done:
        // its `0B8B` charges the ultimate, its `01C7` plays the hit's sound.
        let scene = self.room.world.as_ref().map(|world| &world.scene);
        if let (Some(scene), Some(peer)) = (
            scene,
            shooter.and_then(|shooter| self.peers.get_mut(&shooter)),
        ) {
            let graphs = scene.shared_graphs();
            for hit in &hits {
                let body = scene
                    .bots()
                    .iter()
                    .find(|bot| bot.entity == hit.bot)
                    .map(|bot| bot.spawn.body);
                for notice in hit.notices.iter().flatten() {
                    if let CombatNotice::Hit {
                        dealt, critical, ..
                    } = *notice
                    {
                        let kind = Dealt::Damage { critical };
                        peer.world
                            .dealt(&graphs, hit.bot, body, dealt, &hit.tags, kind);
                        if let Some(source) = hit.source {
                            peer.world
                                .source_dealt(source, hit.bot, dealt, &hit.tags, kind);
                        }
                    }
                }
                peer.world.dealt(
                    &graphs,
                    hit.bot,
                    body,
                    hit.healed,
                    &hit.tags,
                    Dealt::Healing,
                );
                if let Some(source) = hit.source {
                    peer.world
                        .source_dealt(source, hit.bot, hit.healed, &hit.tags, Dealt::Healing);
                }
            }
        }
        let graphs = scene.map(|scene| scene.shared_graphs());
        for hit in hits {
            for (&connection, peer) in &mut self.peers {
                if peer.ai.is_some() {
                    continue;
                }
                peer.world.queue_health(hit.health.clone());
                if shooter == Some(connection) {
                    if let (Some(death), Some(scene), Some(graphs)) = (hit.death, scene, &graphs) {
                        let body = scene
                            .bots()
                            .iter()
                            .find(|bot| bot.entity == death.bot)
                            .map(|bot| bot.spawn.body);
                        peer.world.eliminated(
                            graphs,
                            death.tick,
                            death.bot,
                            body,
                            &death.tags,
                            true,
                        );
                    }
                    for notice in hit.notices.into_iter().flatten() {
                        peer.notice(notice);
                    }
                }
            }
        }
    }

    /// The hits on players' bodies of a room (combat.py `_damage`, `_died`).
    /// What they did between the players goes to the room's Workshop rules
    /// (`WorkshopRun::player_events`).
    fn resolve_player_hits(&mut self, tick: u32, now: Duration) {
        let Room {
            players,
            world,
            flow,
            workshop,
            ..
        } = &mut self.room;
        let Some(WorldStart {
            scene,
            combat: Some(combat),
        }) = world.as_mut()
        else {
            return;
        };
        let hits = combat.take_player_hits();
        let pushes = combat.take_player_pushes();
        let teams: Vec<u8> = players.iter().map(|player| player.info.team).collect();
        let mut peers = room_peers(&mut self.peers, &self.seats, players.len());
        let notices = {
            let mut room: Vec<_> = players
                .iter_mut()
                .zip(peers.iter_mut())
                .map(|(player, peer)| CombatPlayer {
                    entity: player.entity,
                    world: peer.as_deref_mut().map(|peer| &mut peer.world),
                    stats: &mut player.stats,
                })
                .collect();
            combat.apply_environment_credits(scene, &mut room, now);
            let (notices, mut events) = apply_player_hits(scene, hits, &mut room, tick, now);
            events.extend(apply_player_pushes(pushes, &mut room, now));
            if let Some(run) = workshop.as_mut().and_then(|workshop| workshop.run.as_mut()) {
                run.player_events(&events);
            }
            notices
        };
        // The kills count for the match flow's score (Deathmatch, Team
        // Deathmatch): the victim's death notice names the killer's body.
        if let Some(flow) = flow {
            for (victim, notices) in notices.iter().enumerate() {
                for notice in notices {
                    if let CombatNotice::Death { source, killed_by } = *notice {
                        let killer = killed_by
                            .then(|| {
                                players
                                    .iter()
                                    .position(|player| player.entity == source & !0xff)
                            })
                            .flatten();
                        flow.kill(
                            killer.map(|slot| (slot, teams[slot])),
                            (victim, teams[victim]),
                        );
                    }
                }
            }
        }
        // Each victim's hurt notices (25000) first: a killing hit's goes
        // before his death notice.
        for (peer, notices) in peers.into_iter().zip(notices) {
            if let Some(peer) = peer {
                for notice in peer.world.take_hurt_notices().into_iter().chain(notices) {
                    peer.notice(notice);
                }
            }
        }
    }

    /// The auras the tick's hits put on bodies (Biotic Grenade's explosion,
    /// `lucy_script::vm` `auras`) and the auras the bodies' ApplyAura
    /// states put on bodies or took off: each to its body's script, a
    /// bot's or a player's. Then the health changes the bots' graphs made
    /// (their auras' heals and damage), applied as hits by their
    /// instigators.
    fn apply_auras(&mut self, tick: u32, now: Duration) {
        let room = &mut self.room;
        let players = room.players.len();
        let Some(world) = room.world.as_mut() else {
            return;
        };
        let combat = world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene));
        let mut peers = room_peers(&mut self.peers, &self.seats, players);
        let mut changes: Vec<AuraChange> = combat
            .take_auras()
            .into_iter()
            .map(|aura| AuraChange::Apply {
                source: None,
                target: aura.body,
                aura: aura.aura,
                instigator: aura.instigator,
                creator: aura.instigator,
                parameters: Vec::new(),
            })
            .collect();
        for peer in peers.iter_mut().flatten() {
            changes.extend(peer.world.take_body_aura_changes());
        }
        changes.extend(world.scene.take_bot_aura_changes());
        changes.extend(world.scene.take_placed_aura_changes());
        changes.extend(combat.take_field_auras());
        for change in &changes {
            let target = change.target();
            if world.scene.bots().iter().any(|bot| bot.entity == target) {
                world.scene.change_bot_aura(change, tick);
            } else if let Some(peer) = peers
                .iter_mut()
                .flatten()
                .find(|peer| peer.world.current_body() == Some(target))
            {
                peer.world.change_aura(change);
            }
        }
        let context = ShotContext {
            quanta: RETAIL_QUANTA,
            tick,
            now,
        };
        let mut struck: Vec<(Option<u32>, BotHit)> = Vec::new();
        // The bots' graphs' changes, the server placeables' (a health
        // pack heals the body that walked onto it, `0046` st0) and the mode
        // entity's (the Workshop's Kill `1482`).
        let mut changes = world.scene.take_bot_health_changes();
        changes.extend(world.scene.take_placed_health_changes());
        if let Some(script) = room.mode.as_mut().and_then(ModeRunner::script_mut) {
            changes.extend(script.take_health_changes());
        }
        for change in changes {
            let shooter = self
                .peers
                .iter()
                .find(|(_, peer)| peer.world.current_body() == Some(change.instigator))
                .map(|(&connection, _)| connection);
            if let Some(hit) = combat.modify_health(&mut world.scene, &change, context, |_| true) {
                struck.push((shooter, hit));
            }
        }
        for (shooter, hit) in struck {
            self.deliver_bot_hits(shooter, vec![hit]);
        }
    }

    /// combat.py `Combat.update`, once the room has a world: where the
    /// bodies are, the bots' rockets, the players' projectiles and what they
    /// ask of the room (projectiles.py `Projectiles.update`), the hits on
    /// players, the Biotic Fields' heals and the messages between bodies,
    /// then the players' respawns.
    pub(super) fn update_combat(&mut self, tick: u32, now: Duration) {
        if self.room.world.is_none() {
            return;
        }
        let bot_hits = {
            let Room { players, world, .. } = &mut self.room;
            let world = world.as_mut().expect("room with a world");
            let targets: Vec<_> = room_peers(&mut self.peers, &self.seats, players.len())
                .into_iter()
                .enumerate()
                .filter_map(|(slot, peer)| {
                    peer?
                        .world
                        .combat_target(&world.scene, players[slot].info.team)
                })
                .collect();
            let combat = world
                .combat
                .get_or_insert_with(|| Combat::new(&world.scene));
            combat.record_poses(&world.scene, &targets, tick);
            // The bodies the statescript's target searches (`TrackTargets`)
            // find until the next update, as placed this tick.
            let candidates = combat.target_candidates(&world.scene);
            world.scene.set_bot_target_candidates(&candidates);
            world.scene.set_placed_target_candidates(&candidates);
            // The health packs the bodies' searches find (`0033` st11),
            // and the placeables' entity variables their graphs read.
            let pickups = world.scene.pickups();
            let map_variables: Arc<[(u32, lucy_script::wire::Variables)]> =
                world.scene.placed_scripts().entity_variables().into();
            for peer in self.peers.values_mut() {
                peer.world.set_target_candidates(&candidates);
                peer.world.set_pickups(&pickups);
                peer.world.set_map_variables(&map_variables);
            }
            let context = ShotContext {
                quanta: RETAIL_QUANTA,
                tick,
                now,
            };
            combat
                .step_bot_projectiles(&mut world.scene, context)
                .unwrap_or_default()
        };
        self.deliver_bot_hits(None, bot_hits);
        // projectiles.py `flown`: the copies of the rockets that landed.
        self.bot_rockets(tick, finish_bot_rockets);
        // combat.py `_bot_weapons`: the firing bots' scripts after the
        // rockets flew; their shots at the players and the other team's
        // bots.
        let (bot_hits, rockets) = {
            let world = self.room.world.as_mut().expect("room with a world");
            let combat = world
                .combat
                .get_or_insert_with(|| Combat::new(&world.scene));
            let mut hits = Vec::new();
            let mut rockets = Vec::new();
            for shot in world.scene.bot_weapons(tick) {
                let shooter = Shooter {
                    source: shot.source,
                    team: shot.team,
                    bot: true,
                };
                let seen = SeenRocket::of(0, &shot);
                match shot.kind {
                    BotShotKind::Projectile(projectile) => {
                        let fired = combat.fire_bot_projectile(shooter, projectile, shot.tick);
                        if let Some((rocket, mut seen)) = fired.zip(seen) {
                            seen.rocket = rocket;
                            rockets.push(seen);
                        }
                    }
                    BotShotKind::Hitscan {
                        origin,
                        direction,
                        reach,
                        hit,
                        ray_profile,
                        ..
                    } => {
                        let context = ShotContext {
                            quanta: RETAIL_QUANTA,
                            tick: shot.tick,
                            now,
                        };
                        if let Ok(Some(hit)) = combat.bot_hitscan(
                            &mut world.scene,
                            shooter,
                            origin,
                            direction,
                            reach,
                            hit,
                            ray_profile,
                            context,
                        ) {
                            hits.push(hit);
                        }
                    }
                }
            }
            (hits, rockets)
        };
        self.deliver_bot_hits(None, bot_hits);
        self.bot_rockets(tick, |combat, worlds, tick| {
            show_bot_rockets(combat, &rockets, worlds, tick);
        });
        let mut struck = Vec::new();
        for (&connection, peer) in &mut self.peers {
            let room = &mut self.room;
            let player = &room.players[self.seats[&connection]];
            let team = player.info.team;
            let context = ShotContext {
                quanta: player.quanta(),
                tick,
                now,
            };
            let world = room.world.as_mut().expect("room with a world");
            let mut hits = Vec::new();
            let _ = peer.world.advance_projectiles(tick, |source, flight| {
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                let shooter = Shooter {
                    source,
                    team,
                    bot: false,
                };
                combat.step_projectile(&mut world.scene, shooter, flight, context, &mut hits)
            });
            struck.push((connection, hits));
        }
        for (connection, hits) in struck {
            self.deliver_bot_hits(Some(connection), hits);
        }
        self.apply_auras(tick, now);
        // The environment kills a living body whose capsule centre is in
        // a death zone (`lucy_sim::map_areas`), with the tick's hits.
        {
            let Room { players, world, .. } = &mut self.room;
            let world = world.as_mut().expect("room with a world");
            let combat = world
                .combat
                .get_or_insert_with(|| Combat::new(&world.scene));
            for (slot, peer) in room_peers(&mut self.peers, &self.seats, players.len())
                .into_iter()
                .enumerate()
            {
                let Some(peer) = peer else { continue };
                if peer.world.body_dead() {
                    continue;
                }
                let fell = peer.world.body_centre().and_then(|(centre, above_feet)| {
                    let fell = world.scene.fell_out(centre, above_feet)?;
                    if fell == FellOut::BelowMap {
                        tracing::warn!(
                            "[game] a body fell {} m below {} ({:#x}) at {centre:?}: \
                             the map's data has no volume that kills there",
                            lucy_sim::map_areas::FALL_LIMIT,
                            world.scene.map().name,
                            world.scene.map().map_guid,
                        );
                    }
                    Some(fell)
                });
                if let Some(body) = peer.world.current_body().filter(|_| fell.is_some()) {
                    combat.environment_kill(body, players[slot].info.team, tick);
                }
            }
        }
        self.deliver_projectiles(tick);
        self.resolve_player_hits(tick, now);
        let connections: Vec<u32> = self.peers.keys().copied().collect();
        for connection in connections {
            let retained = self.retained_elsewhere(connection, now);
            let slot = self.seats[&connection];
            let room = &self.room;
            let world = room.world.as_ref().expect("room with a world");
            let viewer = room.appearance(slot).expect("seated player");
            let peer = self.peers.get_mut(&connection).expect("room peer");
            peer.world.retain_elsewhere(retained);
            // The match flow holds the dead within an Elimination round
            // and once the match is decided.
            let respawns = room.flow.as_ref().is_none_or(MatchFlow::respawns);
            if let Err(error) = respawns
                .then(|| {
                    peer.world.respawn_due(
                        &world.scene,
                        &viewer,
                        tick,
                        now,
                        room.players[slot].quanta(),
                    )
                })
                .transpose()
            {
                eprintln!("{:08X} could not respawn ({error})", viewer.entity);
            }
            // A hero the controller's graph forced on him (the
            // Workshop's Start Forcing Player To Be Hero).
            if let Err(error) = peer.world.change_to_forced_hero(
                &world.scene,
                &viewer,
                tick,
                now,
                room.players[slot].quanta(),
            ) {
                eprintln!("{:08X} could not change hero ({error})", viewer.entity);
            }
            peer.world.area_step(&world.scene, &viewer);
        }
    }

    /// projectiles.py `Projectiles.update` for a room once its players'
    /// shots stepped: the other clients' copies and the Biotic Fields
    /// (`lucy_sim::spawned`).
    fn deliver_projectiles(&mut self, tick: u32) {
        let Room { players, world, .. } = &mut self.room;
        let Some(world) = world.as_mut() else {
            return;
        };
        let combat = world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene));
        let mut peers = room_peers(&mut self.peers, &self.seats, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        deliver_projectiles(&world.scene, combat, &mut worlds, tick);
    }
}

/// A hit on a bot reaches its script (combat.py `_damage`): for a killing
/// hit its damage message `0011.025` with the kill's fields, then the death
/// message (`_bot_died`, [`PreparedScene::bot_died`]), else its new health
/// (`_sync_body_health`) and, for damage, the damage message `0011.025`
/// (its flinch and pain line).
fn bot_hit_script(scene: &mut PreparedScene, hit: &BotHit) {
    match hit.death {
        Some(death) => scene.bot_died(&death),
        None => {
            scene.bot_health(hit.bot, hit.remaining);
            if let Some(damage) = &hit.damage {
                scene.bot_hurt(hit.bot, damage);
            }
        }
    }
}

/// The room's connected players (its AI players too), by slot.
pub(super) fn room_peers<'p>(
    peers: &'p mut IndexMap<u32, Peer>,
    seats: &HashMap<u32, usize>,
    players: usize,
) -> Vec<Option<&'p mut Peer>> {
    let mut room: Vec<_> = (0..players).map(|_| None).collect();
    for (connection, peer) in peers.iter_mut() {
        room[seats[connection]] = Some(peer);
    }
    room
}
