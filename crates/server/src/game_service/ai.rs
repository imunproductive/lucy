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
//! His body is possessed by his AI entity ([`ai_entity`]), made from the
//! hero body's AI definition at the room's difficulty
//! (`orpheus_script::ai_definitions`): the body's graphs see an AI
//! possessor with component 92 and read his weapons' damage from its
//! difficulty profile (Soldier: 76's rifle 4, 10 or 20 for the player's 20),
//! as retail's weapon graphs are written to. The AI entity is the
//! server's: the clients see his controller possess the body, as a human's
//! (`docs/dev/loose-ends.md`).
//!
//! A dummy bot (`STU_FC3921B1`: the Practice Range's Training Partner,
//! made by the console `265F`) is such a player too, in the room's next
//! slot, whose seat gives his body no input: he stands where the graph put
//! him, facing its way. `STU_C1561031` takes him out as a leaving player.
//!
//! The Python reference server had no AI players: this is the Rust
//! server's own (`docs/notes/related-projects.md`, "AI heroes as
//! server-driven players"). Assumptions: an AI's name is its hero's
//! (retail's names are unknown); an AI whose hero's statescript can't
//! start plays Soldier: 76 instead; the AI heroes come into the world with
//! the first player (the match flow's start), not before.

use std::{io, time::Duration};

use orpheus_content::heroes::SOLDIER;
use orpheus_lobby::login::Identity;
use orpheus_script::{
    placed::{DummyChange, NextDummy},
    vm::DummyBot,
};
use orpheus_sim::{
    ai::{AiController, Difficulty, Goal, Random, Seen, Senses, pick_heroes, ticks},
    match_flow::{MatchFlow, Phase},
    navigate::PathBudget,
    replication::WorldDelivery,
    scene::PlayerCard,
    scene_assets::PreparedScene,
};
use orpheus_wire::commands::Command;

use super::{
    FIRST_AI_CONNECTION, MatchId, MatchPlayer, Peer, Player, PlayerStats, Room, State, invalid,
    pick_message, player_entity,
};

/// The AI entity of the AI in slot n is `AI_ENTITY + n`: in id class 0,
/// where the server's own entities are, between the map's placeables (at
/// most `0x80001250`) and the training bots' controllers (`bots.rs`).
const AI_ENTITY: u32 = 0x8000_7000;

/// The AI entity of the AI player in `slot`.
fn ai_entity(slot: usize) -> u32 {
    AI_ENTITY + slot as u32
}

/// What plays an AI player's body.
enum Plays {
    /// An AI hero: his controller's commands.
    Ai(Box<AiController>),
    /// A dummy bot (`STU_FC3921B1`: the Practice Range's Training
    /// Partner): no input, as a Workshop dummy bot; he faces `yaw` (1/65536
    /// turns), the way he was made.
    Dummy { yaw: i32 },
}

/// An AI player's seat: what plays him.
pub(super) struct AiSeat {
    plays: Plays,
    /// His AI entity ([`ai_entity`]), which possesses each body he plays.
    pub(super) entity: u32,
    /// The difficulty identifier his AI plays at.
    pub(super) difficulty: u64,
    /// His roster hero, by name, until the scene names its GUID.
    name: &'static str,
    hero: Option<u64>,
    /// When he may ask for his hero again, while his body isn't the one he
    /// asked for (a pick before his controller arrived goes nowhere).
    ask_again: u32,
}

/// A dummy bot of a team's slot and the room slot of his player.
pub(super) struct DummySeat {
    pub(super) team: i64,
    pub(super) slot: i64,
    pub(super) player: usize,
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
                        plays: Plays::Ai(Box::new(controller)),
                        entity: ai_entity(slot),
                        difficulty: difficulty.identifier(),
                        name,
                        hero: None,
                        ask_again: 0,
                    },
                ));
            }
        }
        seats
    }

    /// The player and first body the next dummy bot gets: the next slot's
    /// player, whose first body is the first of his ring (`replication`'s
    /// `next_body`).
    pub(super) fn next_dummy(&self) -> NextDummy {
        let player = player_entity(self.players.len());
        NextDummy {
            player,
            body: player | 1,
        }
    }
}

/// Where an AI of `team` goes in this phase: the objective in the round
/// (the nearest enemy without one); in setup, the defenders to the
/// objective while the attackers wait behind their doors (rule 3); in
/// hero select, once a round is over and without a match flow, nowhere.
fn goal(flow: Option<&MatchFlow>, team: u8) -> Goal {
    let Some(flow) = flow else {
        return Goal::Hold;
    };
    match flow.phase() {
        Phase::Round => flow.objective().map_or(Goal::Hunt, Goal::Objective),
        Phase::Setup if flow.attackers() != Some(team) => {
            flow.objective().map_or(Goal::Hold, Goal::Objective)
        }
        _ => Goal::Hold,
    }
}

impl AiSeat {
    /// A dummy bot's seat: he plays `hero`, facing `yaw`.
    fn dummy(slot: usize, hero: u64, yaw: i32) -> Self {
        Self {
            plays: Plays::Dummy { yaw },
            entity: ai_entity(slot),
            // Assumption: a dummy's AI plays at the lowest level (he never
            // fires, so its weapon profile goes unread).
            difficulty: Difficulty::Easy.identifier(),
            name: "",
            hero: Some(hero),
            ask_again: 0,
        }
    }

    /// His tick: into the world with the first player, his hero, then his
    /// command, which he gives once his body has taken the last one.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        delivery: &mut WorldDelivery,
        room: &Room,
        scene: &PreparedScene,
        flow: Option<&MatchFlow>,
        slot: usize,
        seen: &[Seen],
        paths: &PathBudget,
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
            delivery.present(scene, room.first_mode_frame(scene), tick)?;
            delivery.deliver_locally(tick, now);
        }
        delivery.set_assembling(room.assembling());
        delivery.set_mode_graph(room.follows_graph());
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
        // Soldier: 76 instead. A dummy bot never fires: he keeps the hero
        // his graph asked for.
        let playing = delivery.current_hero();
        let dummy = matches!(self.plays, Plays::Dummy { .. });
        let wanted = match playing {
            _ if dummy => hero,
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
        let controller = match &mut self.plays {
            Plays::Ai(controller) => controller,
            // A still body facing where he was made.
            Plays::Dummy { yaw } => {
                let still = Command {
                    frame: tick,
                    yaw: *yaw,
                    ..Command::default()
                };
                delivery.queue_commands(tick, &[still]);
                return Ok(());
            }
        };
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
        let body = playing
            .and_then(|hero| scene.heroes().get(hero))
            .map(|hero| hero.body);
        let walkable = scene.walk_test(body, team);
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
            walkable: &walkable,
        };
        let command = controller.command(&senses);
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

    /// The dummy bots the room's placed scripts made or removed
    /// (`STU_FC3921B1`, `STU_C1561031`: the Practice Range's Training
    /// Partner console `265F`), in order.
    pub(super) fn change_dummies(&mut self, id: MatchId, changes: Vec<DummyChange>) {
        for change in changes {
            match change {
                DummyChange::Remove { team, slot } => self.remove_dummy(id, team, slot),
                DummyChange::Create { dummy, next } => self.add_dummy(id, &dummy, next),
            }
        }
    }

    /// A dummy bot is an AI player in the room's next slot
    /// ([`Room::next_dummy`], which the scripts' graph read), with a seat
    /// that gives his body no input, and whose bodies come into the world
    /// where the graph put him (`WorldDelivery::place_bodies_at`;
    /// assumption: a dummy's respawns come back there). He is on the
    /// graph's team; his name and card are his hero's (assumption, as an
    /// AI hero's).
    fn add_dummy(&mut self, id: MatchId, dummy: &DummyBot, next: NextDummy) {
        let Some(room) = self.rooms.get_mut(&id) else {
            return;
        };
        let hero = room
            .world
            .as_ref()
            .and_then(|world| world.scene.heroes().get(dummy.hero))
            .map(|hero| (hero.guid, hero.name.clone()));
        let (Some((hero, name)), Ok(team)) = (hero, u8::try_from(dummy.team)) else {
            tracing::warn!(
                "[game] no dummy bot of hero {:016X} on team {}",
                dummy.hero,
                dummy.team
            );
            return;
        };
        if room.next_dummy() != next {
            tracing::warn!("[game] a dummy bot's player {:08X} is taken", next.player);
            return;
        }
        let slot = room.players.len();
        let account = Identity::for_account(u32::MAX - slot as u32).account_id();
        room.players.push(Player {
            info: MatchPlayer {
                name: name.clone(),
                team,
                tournament: false,
                ranked: false,
            },
            card: PlayerCard {
                account,
                battle_tag: name,
                ..PlayerCard::default()
            },
            hero,
            entity: next.player,
            gone: false,
            stats: PlayerStats::default(),
            ai: true,
        });
        room.dummies.push(DummySeat {
            team: dummy.team,
            slot: dummy.slot,
            player: slot,
        });
        let [x, _, z] = dummy.facing;
        let yaw = (x.atan2(z) * 65_536.0 / std::f64::consts::TAU).round() as i32;
        let mut peer = Peer::ai(next.player, AiSeat::dummy(slot, hero, yaw));
        peer.world.place_bodies_at(dummy.position, yaw);
        let connection = self.next_ai_seat();
        self.seats.insert(connection, (id, slot));
        self.peers.insert(connection, peer);
    }

    /// The dummy bot of a team's slot leaves the room, as a player who
    /// leaves: his peer goes, so the clients lose his body.
    fn remove_dummy(&mut self, id: MatchId, team: i64, slot: i64) {
        let Some(room) = self.rooms.get_mut(&id) else {
            return;
        };
        let Some(index) = room
            .dummies
            .iter()
            .position(|dummy| dummy.team == team && dummy.slot == slot)
        else {
            return;
        };
        let player = room.dummies.remove(index).player;
        room.players[player].gone = true;
        let seated = self
            .seats
            .iter()
            .find(|&(_, &seat)| seat == (id, player))
            .map(|(&connection, _)| connection);
        if let Some(connection) = seated {
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
                room_heal: false,
                room_hurt: false,
                support_tag_given: false,
                support_tag_delivered: (false, orpheus_script::wire::Value::Null),
                picker: None,
                console_button: false,
                picker_raised: false,
                spawn_effect: false,
                spawn_scope: Vec::new(),
                hold_to_sprint_delivered: orpheus_script::wire::Value::Null,
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
            body.room_heal = peer.world.body_state_active(0x0c8e, 13);
            body.room_hurt = peer.world.body_state_active(0x0c8e, 45);
            body.support_tag_given = peer.world.body_state_active(0x01cf, 117);
            body.support_tag_delivered = peer.world.body_delivered(
                0x01cf,
                117,
                orpheus_content::hero_settings::ALLIED_HEALTH_BARS as u32,
            );
            body.picker = peer.world.controller_scope_instance(126);
            body.console_button = peer.world.body_delivered(0x265f, 9, 25456).0;
            body.picker_raised = peer.world.body_delivered(0x265f, 17, 25456).0;
            body.spawn_effect = peer.world.body_state_active(0x265f, 21);
            body.spawn_scope = peer.world.body_scope_hits(0x265f, 20);
            body.hold_to_sprint_delivered = peer.world.body_delivered(0x01cf, 117, 0x1ddc).1;
        }
        bodies
    }

    /// Every AI player's tick, in the rooms with AI heroes whose match flow
    /// started and the rooms with dummy bots: what he sees (the living
    /// bodies of the room), then his command; the room's AI heroes share
    /// path work of a tick with its bots (`PreparedScene::path_budget`).
    pub(super) fn drive_ai(&mut self, tick: u32, now: Duration) {
        let Self {
            rooms,
            seats,
            peers,
            ..
        } = self;
        for (&id, room) in rooms.iter() {
            if room.ai.is_none() && room.dummies.is_empty() {
                continue;
            }
            let Some(world) = &room.world else {
                continue;
            };
            let flow = room.flow.as_ref();
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
            let paths = scene.path_budget();
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
                if match_id != id || (flow.is_none() && matches!(seat.plays, Plays::Ai(_))) {
                    continue;
                }
                let step = seat.step(delivery, room, scene, flow, slot, &seen, paths, tick, now);
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
    /// His body's mode body graph `0C8E` heals him in his team's spawn
    /// room (st13, on the room's `0723`) and hurts him in the other
    /// team's (st45, on its `0B7B`).
    pub room_heal: bool,
    pub room_hurt: bool,
    /// His body's `01CF` st117 is active: ALLIED HEALTH BARS at ALWAYS gave
    /// it the support tag (`orpheus_content::hero_settings`).
    pub support_tag_given: bool,
    /// The last owner frame his client acknowledged: whether st117 runs in
    /// it, and its ALLIED HEALTH BARS variable (`#8336e`).
    pub support_tag_delivered: (bool, orpheus_script::wire::Value),
    /// Its HOLD TO SPRINT variable (`#7644e`, Soldier: 76's `1DDC.01C`) in
    /// that frame.
    pub hold_to_sprint_delivered: orpheus_script::wire::Value,
    /// His controller's instance of the Training Partner console's scope
    /// 126, where its picker sends `1213` from.
    pub picker: Option<u16>,
    /// The last owner frame his client acknowledged runs the Training
    /// Partner console's scope 12 instance's H button (st9).
    pub console_button: bool,
    /// The last owner frame his client acknowledged runs the scope's
    /// StateStack rid40 (st17), which stacks the picker's condition
    /// `#25456e` = true (the client reads it from that state's payload).
    pub picker_raised: bool,
    /// His body runs the Training Partner console's scope 197 (`265F`
    /// st21: the 2 s Wait with the spawn effect `1F3F.00D`).
    pub spawn_effect: bool,
    /// His body's scope 12 instance runs scope 197's state (`265F` st20):
    /// its targets (the new partner's body) and the instance its frame
    /// record names on each.
    pub spawn_scope: Vec<(u32, Option<u16>)>,
}
