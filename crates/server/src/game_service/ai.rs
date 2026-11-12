//! The AI heroes of a room (Practice vs AI, and matches the matchmaker
//! fills with AI): players without a game. Each has a seat with an id no
//! game link has ([`FIRST_AI_CONNECTION`] and up) and a peer whose world no
//! client receives ([`WorldDelivery::headless`]): his records arrive as soon
//! as they are ready, so his hero's body lives as a connected player's does
//! (the player body factory, its statescript, mover, combat, death and
//! respawn, the match flow's teams and objectives). The other clients see
//! him through the same path as any other player (`others.rs`): his card,
//! his entry in the team list, his body and its observer frames. His
//! commands come from an [`AiController`] (`orpheus_sim::ai`) once a tick.
//!
//! The Python reference server had no AI players: this is the Rust
//! server's own (`docs/notes/related-projects.md`, "AI heroes as
//! server-driven players"). Assumptions: an AI's name is its hero's
//! (retail's names are unknown); an AI whose hero's statescript can't
//! start plays Soldier: 76 instead; the AI heroes come into the world with
//! the first player (the match flow's start), not before.

use std::{cell::Cell, io, time::Duration};

use orpheus_content::heroes::SOLDIER;
use orpheus_lobby::login::Identity;
use orpheus_sim::{
    ai::{
        AiController, Difficulty, Goal, PATHS_PER_TICK, Random, Seen, Senses, pick_heroes, ticks,
    },
    combat::PlayerHit,
    match_flow::{MatchFlow, Phase},
    replication::WorldDelivery,
    scene::PlayerCard,
    scene_assets::PreparedScene,
};

use super::{
    FIRST_AI_CONNECTION, MatchId, MatchPlayer, Peer, Player, PlayerStats, Room, State, invalid,
    pick_message, player_entity,
};

/// An AI player's seat: what plays him.
pub(super) struct AiSeat {
    controller: AiController,
    /// His roster hero, by name, until the scene names its GUID.
    name: &'static str,
    hero: Option<u64>,
    /// When he may ask for his hero again, while his body isn't the one he
    /// asked for (a pick before his controller arrived goes nowhere).
    ask_again: u32,
}

/// A seed for a room's AI heroes.
pub(super) fn seed() -> u64 {
    let mut bytes = [0; 8];
    let _ = getrandom::fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

impl Room {
    /// AI heroes of `difficulty` in the places the players leave free on
    /// each team (`map.team_sizes`), in the slots after theirs; a team's AI
    /// heroes are each a roster hero of their own (`pick_heroes`). Their
    /// seats, by slot. A free-for-all gets none.
    pub(super) fn add_ai(
        &mut self,
        difficulty: Difficulty,
        random: &mut Random,
    ) -> Vec<(usize, AiSeat)> {
        self.ai = Some(difficulty);
        if self.map.free_for_all {
            return Vec::new();
        }
        let mut seats = Vec::new();
        for (team, &size) in self.map.team_sizes.clone().iter().enumerate() {
            let team = u8::try_from(team).unwrap_or(u8::MAX);
            let players = self
                .players
                .iter()
                .filter(|player| player.info.team == team)
                .count();
            let missing = usize::from(size).saturating_sub(players);
            for (rank, name) in pick_heroes(missing, &[], random).into_iter().enumerate() {
                let slot = self.players.len();
                let account = Identity::for_account(u32::MAX - slot as u32).account_id();
                self.players.push(Player {
                    info: MatchPlayer {
                        name: name.into(),
                        team,
                        tournament: false,
                        ranked: false,
                    },
                    card: PlayerCard {
                        account,
                        battle_tag: name.into(),
                        ..PlayerCard::default()
                    },
                    hero: SOLDIER,
                    entity: player_entity(slot),
                    gone: false,
                    stats: PlayerStats::default(),
                    ai: true,
                });
                let controller = AiController::new(difficulty, rank, missing, random.next_u64());
                seats.push((
                    slot,
                    AiSeat {
                        controller,
                        name,
                        hero: None,
                        ask_again: 0,
                    },
                ));
            }
        }
        seats
    }
}

/// Rule 123: an AI hero deals its difficulty's share of his weapons'
/// damage, to players and AI heroes alike. His heals stay whole
/// (assumption: the rule speaks of damage alone).
pub(super) fn scale_damage(hits: &mut [PlayerHit], players: &[Player], difficulty: Difficulty) {
    for hit in hits.iter_mut().filter(|hit| !hit.heal) {
        let shooter = hit.shooter.source & !0xff;
        if players
            .iter()
            .any(|player| player.ai && player.entity == shooter)
        {
            hit.amount *= difficulty.damage_scale();
        }
    }
}

/// Where an AI of `team` goes in this phase: the objective in the round
/// (the nearest enemy without one); in setup, the defenders to the
/// objective while the attackers wait behind their doors (rule 3); in
/// hero select and once a round is over, nowhere.
fn goal(flow: &MatchFlow, team: u8) -> Goal {
    match flow.phase() {
        Phase::Round => flow.objective().map_or(Goal::Hunt, Goal::Objective),
        Phase::Setup if flow.attackers() != Some(team) => {
            flow.objective().map_or(Goal::Hold, Goal::Objective)
        }
        _ => Goal::Hold,
    }
}

impl AiSeat {
    /// His tick: into the world with the first player, his hero, then his
    /// command, which he gives once his body has taken the last one.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        delivery: &mut WorldDelivery,
        room: &Room,
        scene: &PreparedScene,
        flow: &MatchFlow,
        slot: usize,
        seen: &[Seen],
        paths: &Cell<usize>,
        tick: u32,
        now: Duration,
    ) -> io::Result<()> {
        let viewer = room
            .appearance(slot)
            .ok_or_else(|| invalid("an AI hero without a slot"))?;
        let team = viewer.team;
        if !delivery.world_started() {
            // In the world since the first player came in: his game shows
            // the AI heroes as there from the start, not as joining.
            delivery.start(scene, &viewer, room.flow_tick)?;
            delivery.deliver_locally(tick, now);
            delivery.present(scene, tick)?;
            delivery.deliver_locally(tick, now);
        }
        delivery.set_assembling(room.assembling());
        if room.assembled {
            delivery.end_assemble(scene, tick)?;
        }
        let hero = *self.hero.get_or_insert_with(|| {
            scene
                .heroes()
                .find_hero(self.name)
                .map_or(SOLDIER, |hero| hero.guid)
        });
        // A hero whose statescript could not start has no weapons:
        // Soldier: 76 instead.
        let playing = delivery.current_hero();
        let wanted = match playing {
            Some(current) if current != SOLDIER && !delivery.combat_body_active() => SOLDIER,
            Some(current) if current == SOLDIER && hero != SOLDIER => SOLDIER,
            _ => hero,
        };
        if playing != Some(wanted) && delivery.takes_game_messages() && tick >= self.ask_again {
            delivery.queue_game_message(tick, now, pick_message(viewer.entity, wanted));
            self.ask_again = tick + ticks(1.0);
        }
        // The server takes a body's commands between its ticks, later than
        // they're given when it is busy: he decides from where his body
        // stands, so his next command waits until his body has taken the
        // last (a command a tick when the server keeps up; frames between
        // two commands are the body's last input held, as for a client's
        // late commands).
        if delivery.has_input_work() {
            return Ok(());
        }
        let me = delivery.combat_target(scene, team);
        let others: Vec<Seen> = seen
            .iter()
            .filter(|other| other.slot != slot)
            .copied()
            .collect();
        let feet = delivery.body_position();
        let home = room
            .map
            .spawn_point_in(team, viewer.spawn, viewer.room)
            .map_or_else(|| feet.unwrap_or_default(), |(point, _)| point);
        let sight = |from, to| scene.line_of_sight(from, to);
        let senses = Senses {
            tick,
            team,
            feet,
            body: delivery.current_body(),
            eye: me.map_or(1.5, |me| me.pose.eye),
            health: delivery.health_share().unwrap_or(1.0),
            others: &others,
            goal: goal(flow, team),
            home,
            navigation: scene.navigation(),
            paths,
            sight: &sight,
        };
        let command = self.controller.command(&senses);
        delivery.queue_commands(tick, &[command]);
        Ok(())
    }
}

impl State {
    /// The AI heroes' seats and peers, once their room is in.
    pub(super) fn seat_ai(&mut self, id: MatchId, seats: Vec<(usize, AiSeat)>) {
        for (slot, seat) in seats {
            let connection = self.next_ai_seat();
            let entity = self.rooms[&id].players[slot].entity;
            self.seats.insert(connection, (id, slot));
            self.peers.insert(connection, Peer::ai(entity, seat));
        }
    }

    fn next_ai_seat(&mut self) -> u32 {
        loop {
            let connection = self.next_ai_connection;
            self.next_ai_connection = connection.checked_add(1).unwrap_or(FIRST_AI_CONNECTION);
            if !self.seats.contains_key(&connection) {
                return connection;
            }
        }
    }

    /// The room is gone with its last player: its AI heroes go too.
    pub(super) fn unseat_ai(&mut self, id: MatchId) {
        let seated: Vec<u32> = self
            .seats
            .iter()
            .filter(|(_, seat)| seat.0 == id)
            .map(|(&connection, _)| connection)
            .collect();
        for connection in seated {
            self.seats.remove(&connection);
            self.peers.shift_remove(&connection);
        }
    }

    /// Tests: every player of the match and his body, in slot order.
    #[cfg(test)]
    pub(super) fn bodies(&self, id: MatchId) -> Vec<BodyState> {
        let Some(room) = self.rooms.get(&id) else {
            return Vec::new();
        };
        let mut bodies: Vec<BodyState> = room
            .players
            .iter()
            .map(|player| BodyState {
                name: player.info.name.clone(),
                team: player.info.team,
                ai: player.ai,
                hero: None,
                scripted: false,
                position: None,
                health: None,
                spawn_room: None,
                support_tag_given: false,
                support_tag_delivered: (false, orpheus_script::wire::Value::Null),
            })
            .collect();
        for (connection, peer) in &self.peers {
            let (match_id, slot) = self.seats[connection];
            if match_id != id {
                continue;
            }
            let body = &mut bodies[slot];
            body.hero = peer.world.current_hero();
            body.scripted = peer.world.combat_body_active();
            body.position = peer.world.body_position();
            body.health = peer.world.health_share();
            body.spawn_room = peer.world.in_spawn_room();
            body.support_tag_given = peer.world.body_state_active(0x01cf, 117);
            body.support_tag_delivered = peer.world.body_delivered(
                0x01cf,
                117,
                orpheus_content::hero_settings::ALLIED_HEALTH_BARS as u32,
            );
        }
        bodies
    }

    /// Every AI hero's tick, in the rooms whose match flow started: what
    /// he sees (the living bodies of the room), then his command; the
    /// room's AI heroes share [`PATHS_PER_TICK`] new walks a tick.
    pub(super) fn drive_ai(&mut self, tick: u32, now: Duration) {
        let Self {
            rooms,
            seats,
            peers,
            ..
        } = self;
        for (&id, room) in rooms.iter() {
            if room.ai.is_none() {
                continue;
            }
            let (Some(world), Some(flow)) = (&room.world, &room.flow) else {
                continue;
            };
            let scene = &world.scene;
            let mut seen = Vec::new();
            for (connection, peer) in peers.iter() {
                let (match_id, slot) = seats[connection];
                if match_id != id {
                    continue;
                }
                let team = room.players[slot].info.team;
                if let Some(target) = peer
                    .world
                    .combat_target(scene, team)
                    .filter(|target| !target.dead)
                {
                    seen.push(Seen {
                        slot,
                        team,
                        feet: target.pose.feet,
                        height: target.pose.height,
                    });
                }
            }
            let paths = Cell::new(PATHS_PER_TICK);
            for (connection, peer) in peers.iter_mut() {
                let (match_id, slot) = seats[connection];
                let Peer {
                    world: delivery,
                    ai: Some(seat),
                    ..
                } = peer
                else {
                    continue;
                };
                if match_id != id {
                    continue;
                }
                let step = seat.step(delivery, room, scene, flow, slot, &seen, &paths, tick, now);
                if let Err(error) = step {
                    tracing::warn!(
                        "[game] AI {} stopped: {error}",
                        room.players[slot].info.name
                    );
                }
            }
        }
    }
}

/// Tests: a player of a match and his body.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct BodyState {
    pub name: String,
    pub team: u8,
    pub ai: bool,
    pub hero: Option<u64>,
    /// His body runs its statescript (it has weapons).
    pub scripted: bool,
    /// His living body's feet.
    pub position: Option<[f64; 3]>,
    pub health: Option<f64>,
    /// His living body stands in his team's spawn room (the server's
    /// component 74 field 6 flag), if the map has one.
    pub spawn_room: Option<bool>,
    /// His body's `01CF` st117 is active: ALLIED HEALTH BARS at ALWAYS gave
    /// it the support tag (`orpheus_content::hero_settings`).
    pub support_tag_given: bool,
    /// The last owner frame his client acknowledged: whether st117 runs in
    /// it, and its ALLIED HEALTH BARS variable (`#8336e`).
    pub support_tag_delivered: (bool, orpheus_script::wire::Value),
}
