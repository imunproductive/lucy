//! A room's match flow: the mode graph or the flow that stands in for it,
//! the Workshop's rules, the phases, the respawns they ask for, the end of
//! the match and its results, and the board's stats and medals.

use std::time::Duration;

use lucy_content::stats::{BRONZE, GOLD, Medals, SILVER, StatsCatalog};
use lucy_script::{
    mode::ModePlayer,
    vm::{GameMessage, TrackedStat},
};
use lucy_sim::{
    instance::FREE_FOR_ALL_TEAM,
    match_flow::{FlowEvent, Mode, Phase, Standing},
    objectives::Participant,
    scene_assets::PreparedScene,
    workshop::WorkshopPlayer,
};
use lucy_wire::entity::{Component as WorldComponent, GAME_MODE_ENTITY};

use super::{MatchResult, Outcome, Player, PlayerResult, Room, RoomState, graph_frame};

/// The game mode entity gets a frame with the match flow's clock every
/// second, and at each phase change (ow174/game/match.py `COUNTDOWN_EVERY`).
const MODE_FRAME_EVERY: Duration = Duration::from_secs(1);

impl RoomState {
    /// A PvP room's match flow this tick (lucy_sim::match_flow): it
    /// advances with where the players' bodies stand, the game mode
    /// entity's frame is made when due, hero select closes once the heroes
    /// are assembled, the phase messages and the mode graph's go to the
    /// players' bodies, a new side, Control round or Elimination round puts
    /// everyone back in spawn, overtime lengthens respawns, a
    /// decided match goes to the lobby's results, and once its end is over
    /// everyone goes home.
    pub(super) fn update_flows(&mut self, tick: u32, now: Duration) {
        let mut home = Vec::new();
        let mut dummies = Vec::new();
        let id = self.id;
        'room: {
            let room = &mut self.room;
            room.assembled = false;
            room.mode_frame = None;
            let mut connections: Vec<(u32, usize)> = self
                .peers
                .keys()
                .map(|&connection| (connection, self.seats[&connection]))
                .collect();
            connections.sort_by_key(|&(_, slot)| slot);
            // The mode graph's players: those in the world, each with his
            // controller, body and team (a free-for-all's players are on
            // its one team, as their filter bits say).
            let players: Vec<ModePlayer> = connections
                .iter()
                .filter(|&&(connection, slot)| {
                    !room.players[slot].gone && self.peers[&connection].world.world_started()
                })
                .map(|&(connection, slot)| ModePlayer {
                    controller: room.players[slot].entity,
                    body: self.peers[&connection].world.current_body(),
                    hero: self.peers[&connection].world.current_hero(),
                    team: if room.map.free_for_all {
                        i64::from(FREE_FOR_ALL_TEAM)
                    } else {
                        i64::from(room.players[slot].info.team)
                    },
                })
                .collect();
            // The mode graph reads its objectives' entity variables (their
            // times, a point's progress and owner) as they stand now.
            if let (Some(mode), Some(world)) = (room.mode.as_mut(), room.world.as_ref()) {
                mode.set_map_variables(&world.scene.placed_scripts().entity_variables());
            }
            // The Range (no match flow): its graph advances with the room,
            // and its frame goes to every client each tick, as what changed
            // since the frame he acknowledged (`lucy_sim::mode_view`):
            // `0CBB` st58's timed ChaseVar (#19530e toward 99999) changes it
            // every tick, and the whole frame (about 920 bytes) crowded the
            // movement records out of the datagrams.
            if room.flow.is_none() {
                if let (Some(mode), Some(_)) = (room.mode.as_mut(), room.world.as_ref()) {
                    let variables = lucy_script::wire::Variables::new();
                    room.mail.extend(mode.advance(tick, &variables, players));
                    room.graph_frame = graph_frame(mode, room.graph_frame.take());
                    // No flow stands in for the graph's own entries here.
                    if let Some(script) = mode.script_mut() {
                        script.take_own_messages();
                    }
                }
                break 'room;
            }
            // The player the next dummy bot a Workshop action makes gets.
            let next_dummy = room.next_dummy();
            let (Some(flow), Some(world)) = (room.flow.as_mut(), room.world.as_ref()) else {
                break 'room;
            };
            let participants: Vec<Participant> = connections
                .iter()
                .filter(|&&(_, slot)| !room.players[slot].gone)
                .map(|&(connection, slot)| Participant {
                    slot,
                    team: room.players[slot].info.team,
                    position: self.peers[&connection].world.body_position(),
                })
                .collect();
            let seconds = now.saturating_sub(room.flow_at).as_secs_f64();
            room.flow_at = now;
            let was = flow.phase();
            // The mode's own graph drives the match (`ModeRunner::holds_rooms`):
            // it keeps its phase, clock, banner, sides and objective name
            // itself and holds its phase messages to every entity, so the
            // flow only follows its phase and clock (`MatchFlow::follow`).
            // Else the flow drives, and its variables go into the graph.
            let mut messages;
            let events;
            match room.mode.as_mut() {
                Some(mode) if mode.holds_rooms() => {
                    let variables = lucy_script::wire::Variables::new();
                    messages = mode.advance(tick, &variables, players);
                    events = match mode.match_state() {
                        Some(state) => flow.follow(state),
                        None => Vec::new(),
                    };
                }
                mode => {
                    events = flow.advance(seconds, &participants);
                    let variables = flow.variables();
                    messages =
                        mode.map_or_else(Vec::new, |mode| mode.advance(tick, &variables, players));
                }
            }
            // The Workshop's rules after the mode entity's frame, against
            // the players in the world. The dummy bots its actions make or
            // remove are AI players of the room (`ai.rs`).
            if let (Some(run), Some(mode)) = (
                room.workshop
                    .as_mut()
                    .and_then(|workshop| workshop.run.as_mut()),
                room.mode.as_mut(),
            ) {
                let mut players: Vec<WorkshopPlayer<'_>> = self
                    .peers
                    .iter_mut()
                    .filter_map(|(connection, peer)| {
                        let &(_, slot) = connections
                            .iter()
                            .find(|(seated, _)| seated == connection)?;
                        let player = &room.players[slot];
                        // The Workshop's slots count within a team (Players
                        // In Slot(0, Team 2) is Team 2's first player); a
                        // dummy bot is in the slot he was made for.
                        let dummy = room
                            .dummies
                            .iter()
                            .find(|dummy| dummy.player == slot)
                            .and_then(|dummy| usize::try_from(dummy.slot).ok());
                        let team_slot = dummy.unwrap_or_else(|| {
                            room.players[..slot]
                                .iter()
                                .filter(|other| other.info.team == player.info.team)
                                .count()
                        });
                        (!player.gone && peer.world.world_started()).then_some(WorkshopPlayer {
                            controller: player.entity,
                            slot: team_slot,
                            team: player.info.team,
                            world: &mut peer.world,
                        })
                    })
                    .collect();
                if let Some(script) = mode.script_mut() {
                    script.set_next_dummy(Some(next_dummy));
                }
                run.step(tick, &world.scene, mode, &mut players);
                if let Some(script) = mode.script_mut() {
                    script.set_next_dummy(None);
                    dummies.extend(script.take_dummy_changes());
                }
            }
            // The flow stands in for the mode graph's entries: what the
            // mode entity's graphs sent it (the Workshop's Set Match Time),
            // unless the mode's own graph took them itself, or has no such
            // entry (Lúcioball's `0F9D`).
            if let Some(mode) = room.mode.as_mut() {
                let stand_in = !mode.holds_rooms()
                    && lucy_sim::mode_script::takes_set_match_time(room.map.mode_guid);
                if let Some(script) = mode.script_mut() {
                    for message in script.take_own_messages() {
                        if stand_in {
                            flow.receive(message.message, &message.parameters);
                        }
                    }
                }
            }
            let changed = events
                .iter()
                .any(|event| matches!(event, FlowEvent::Phase(_)));
            // A graph that drives the mode entity gives its frame each tick
            // (`lucy_sim::mode_view`); else the hand-built one goes once a
            // second and at each phase.
            match room.mode.as_mut() {
                Some(mode) if mode.graph_driven() => {
                    room.graph_frame = graph_frame(mode, room.graph_frame.take());
                }
                mode => {
                    room.graph_frame = None;
                    if changed || now >= room.next_mode_frame {
                        if mode.is_some() {
                            match world.scene.mode_frame(&flow.variables()) {
                                Ok(frame) => room.mode_frame = Some(frame),
                                Err(error) => tracing::warn!("[game] no game mode frame: {error}"),
                            }
                        }
                        room.next_mode_frame = now.saturating_add(MODE_FRAME_EVERY);
                    }
                }
            }
            // The end of the assembly closes hero select by the server's
            // hand only where the flow drives the match: the mode graph's
            // close is its own (`0F4A` st9's `0692` ends, `288B` st36's stack
            // with it).
            room.assembled =
                !flow.follows_graph() && was == Phase::Assemble && flow.phase() != Phase::Assemble;
            let mut respawn = None;
            for event in &events {
                match event {
                    FlowEvent::Phase(phase) => {
                        tracing::info!(
                            "[game] match {:04X}: {phase:?}, {:.1} s",
                            id[0] & 0xFFFF,
                            flow.clock()
                        );
                        // A new side (after the first) puts everyone back in
                        // his side's spawn room and reopens hero select there
                        // (the mode graph's assembly reopens it itself); a
                        // new Control round after the first and every
                        // Elimination round put everyone back in spawn.
                        let sided = flow.attackers().is_some();
                        match phase {
                            Phase::Assemble if sided && flow.round() > 0 => {
                                respawn = Some(!flow.follows_graph());
                            }
                            Phase::Round
                                if flow.mode() == Mode::Elimination
                                    || (flow.mode() == Mode::Control && flow.round() > 1) =>
                            {
                                respawn = Some(false);
                            }
                            _ => {}
                        }
                    }
                    FlowEvent::Message {
                        message,
                        parameters,
                    } => messages.extend(connections.iter().filter_map(|(connection, _)| {
                        Some(GameMessage {
                            sender: GAME_MODE_ENTITY,
                            target: self.peers[connection].world.current_body()?,
                            message: *message,
                            parameters: parameters.clone(),
                        })
                    })),
                    FlowEvent::Objective(objective) => tracing::info!(
                        "[game] match {:04X}: objective {objective} taken",
                        id[0] & 0xFFFF
                    ),
                    FlowEvent::Overtime(on) => {
                        tracing::info!("[game] match {:04X}: overtime {on}", id[0] & 0xFFFF)
                    }
                    FlowEvent::RoundOver(winner) => tracing::info!(
                        "[game] match {:04X}: round {} won by {winner:?}",
                        id[0] & 0xFFFF,
                        flow.round()
                    ),
                    FlowEvent::MatchOver => {
                        room.ended = true;
                        // The AI heroes get no result: the slots of the
                        // result count the players (the matchmaker's seats)
                        // only.
                        let players = room
                            .players
                            .iter()
                            .enumerate()
                            .filter(|(_, player)| !player.ai)
                            .enumerate()
                            .map(|(seat, (slot, player))| PlayerResult {
                                slot: seat,
                                name: player.info.name.clone(),
                                account: player.card.account,
                                team: player.info.team,
                                outcome: match flow.standing(slot, player.info.team) {
                                    Some(Standing::Won) => Outcome::Win,
                                    Some(Standing::Drew) => Outcome::Draw,
                                    _ => Outcome::Loss,
                                },
                                finished: !player.gone
                                    && connections.iter().any(|&(_, seated)| seated == slot),
                                medals: medal_counts(&team_medals(
                                    &room.players,
                                    room.map.free_for_all,
                                    room.map.mode_guid,
                                    slot,
                                    world.scene.stats(),
                                )),
                            })
                            .collect();
                        tracing::info!("[game] match {:04X} is over", id[0] & 0xFFFF);
                        self.events.result(MatchResult {
                            id,
                            map: room.map.map_guid,
                            mode: room.map.mode_guid,
                            players,
                        });
                    }
                    FlowEvent::Finished => {
                        home.extend(connections.iter().map(|&(connection, _)| connection));
                    }
                }
            }
            let overtime = flow.in_overtime();
            room.mail.extend(messages);
            if let Some(select) = respawn {
                let world = room.world.as_ref().expect("room with a world");
                for &(connection, slot) in &connections {
                    let viewer = room.appearance(slot).expect("seated player");
                    let peer = self.peers.get_mut(&connection).expect("room peer");
                    match peer.world.spawn_again(
                        &world.scene,
                        &viewer,
                        tick,
                        now,
                        room.players[slot].quanta(),
                        select,
                    ) {
                        Ok(true) => {
                            if let Some(ai) = &mut peer.ai {
                                ai.revived();
                            }
                        }
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("{:08X} could not respawn ({error})", viewer.entity);
                        }
                    }
                }
            }
            for (connection, _) in &connections {
                if let Some(peer) = self.peers.get_mut(connection) {
                    peer.world.set_overtime(overtime);
                }
            }
        }
        self.change_dummies(dummies);
        for connection in home {
            if !self.links.cancel_handoff(connection) && self.peers.contains_key(&connection) {
                self.send_home(connection, now);
            }
        }
    }
}

/// The stats a player's graphs counted (TrackStat, TrackStatTime;
/// `lucy_script::vm` `stats`) go to the stat table of each target's
/// player, as the catalog keeps each ([`StatsCatalog::credit`]: a "Best"
/// stat its best single amount): the target is a player's controller or
/// one of his bodies (the body ids share the controller's high bits). A
/// target outside the room's players (a Training Bot, an entity of the
/// map) keeps no stats.
pub(super) fn credit_tracked_stats(
    players: &mut [Player],
    catalog: &StatsCatalog,
    tracked: Vec<TrackedStat>,
) {
    for stat in tracked {
        for target in stat.targets {
            let Some(player) = players
                .iter_mut()
                .find(|player| player.entity == target & !0xff)
            else {
                continue;
            };
            for &counted in &stat.stats {
                catalog.credit(&mut player.stats, counted, stat.amount);
            }
        }
    }
}

/// match.py `_board`: the player's board numbers, with their medals among
/// his teammates (every player in a free-for-all) and his hero's stats.
pub(super) fn board(
    room: &Room,
    slot: usize,
    scene: &PreparedScene,
    hero: Option<u64>,
) -> WorldComponent {
    let mode = room.map.mode_guid;
    let medals = team_medals(
        &room.players,
        room.map.free_for_all,
        mode,
        slot,
        scene.stats(),
    );
    scene
        .stats()
        .component(&room.players[slot].stats, mode, hero, Some(&medals))
}

/// The player's medals among his teammates still in the match (every
/// player in a free-for-all), as the Tab board shows them.
fn team_medals(
    players: &[Player],
    free_for_all: bool,
    mode: u64,
    slot: usize,
    catalog: &StatsCatalog,
) -> Medals {
    let me = &players[slot];
    let mut team: Vec<usize> = (0..players.len())
        .filter(|&other| {
            let player = &players[other];
            !player.gone && (free_for_all || player.info.team == me.info.team)
        })
        .collect();
    if !team.contains(&slot) {
        team.push(slot);
    }
    let stats: Vec<_> = team.iter().map(|&other| &players[other].stats).collect();
    let index = team
        .iter()
        .position(|&other| other == slot)
        .expect("the player is on his team");
    catalog.medals(&stats, mode).swap_remove(index)
}

/// Medals counted by tier: gold, silver, bronze.
fn medal_counts(medals: &Medals) -> [u32; 3] {
    let mut counts = [0; 3];
    for &medal in medals.values() {
        match medal {
            GOLD => counts[0] += 1,
            SILVER => counts[1] += 1,
            BRONZE => counts[2] += 1,
            _ => {}
        }
    }
    counts
}
