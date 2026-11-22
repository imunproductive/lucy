//! A room's messages between its entities, the scopes and plays they open
//! on each other's bodies, what each client learns of the other players,
//! and the voice lines the players' bodies say.

use std::time::Duration;

use lucy_script::vm::{PlayRequest, ScopeChange};
use lucy_sim::{
    combat::{Combat, PlayerEvent},
    messages::{Players, Recipients, deliver, deliver_held, deliver_updates},
    mode_script::ModeRunner,
    replication::{RoomPlayer, WorldDelivery, show_players},
    voice_lines::VoiceLine,
};

use super::{RoomState, combat::room_peers};

impl RoomState {
    /// The game messages the room's entities sent since the last tick
    /// (`lucy_sim::messages`): from the players' and AI heroes' bodies
    /// and controllers, the mode graph and the match flow in its place
    /// (`Room::mail`), the placed scripts, the hits' modifier notices
    /// (`Combat::take_messages`) and the Biotic Fields, to the entity each
    /// names or to every entity that runs graphs, the Training Bots too.
    /// After the players' steps, so a pick's hero limit broadcasts reach
    /// the other controllers in its tick.
    pub(super) fn route_messages(&mut self, tick: u32) {
        let (changes, opened, played) = {
            let room = &mut self.room;
            let next = room.next_dummy();
            let Some(world) = &mut room.world else {
                return;
            };
            world.scene.set_next_dummy(Some(next));
            let mut peers = room_peers(&mut self.peers, &self.seats, room.players.len());
            let mut worlds: Vec<&mut WorldDelivery> = peers
                .iter_mut()
                .flatten()
                .map(|peer| &mut peer.world)
                .collect();
            // The Training Partner console's scope 12 on a player's body
            // makes the re-picked dummy there (`265F` node 30).
            for world in &mut worlds {
                world.set_next_dummy(Some(next));
            }
            let mut players = Players::new(&mut worlds, tick);
            let mut messages = players.take_messages();
            let player_ends = players.take_message_ends();
            let mut updates = players.take_message_updates();
            messages.append(&mut room.mail);
            let (placed, mut ends) = world.scene.take_placed_messages();
            messages.extend(placed);
            ends.extend(player_ends);
            if let Some(mode) = &mut room.mode {
                ends.extend(mode.take_message_ends());
                updates.extend(mode.take_message_updates());
            }
            if let Some(combat) = &mut world.combat {
                messages.extend(combat.take_messages());
            }
            let mut spawned = world.combat.as_mut().map(Combat::spawned_mut);
            if let Some(spawned) = &mut spawned {
                let (sent, ended) = spawned.take_messages();
                messages.extend(sent);
                ends.extend(ended);
            }
            // The mode entity's variables the placeables read (the spawn
            // walls' `0FA8` st5 the attacking team #10795e), as the mode's
            // graph left them this tick, before its messages reach them.
            if let Some(variables) = room.mode.as_ref().and_then(ModeRunner::entity_variables) {
                world.scene.set_placed_mode_variables(variables);
            }
            let held = lucy_sim::mode_script::held(room.mode.as_ref());
            let (mut bots, placed) = world.scene.message_recipients(tick);
            let mut recipients: Vec<&mut dyn Recipients> = vec![&mut players, &mut bots, placed];
            if let Some(spawned) = spawned {
                recipients.push(spawned);
            }
            if let Some(mode) = &mut room.mode {
                recipients.push(mode);
            }
            deliver_held(&held, &mut room.held_reach, &mut recipients);
            deliver(&mut room.backlog, messages, ends, &mut recipients);
            deliver_updates(&updates, &mut recipients);
            let mut changes = world.scene.take_dummy_changes();
            drop(players);
            let mut opened = Vec::new();
            let mut writes = Vec::new();
            let mut played = Vec::new();
            for player in &mut worlds {
                changes.extend(player.take_dummy_changes());
                opened.extend(player.take_scope_changes());
                writes.extend(player.take_parent_writes());
                played.extend(player.take_body_play_requests());
            }
            // A body's scope instance sets its console's variables
            // (`STU_D49BBD03`: the console's `#940` on a re-pick).
            for write in &writes {
                world.scene.set_parent_variable(write);
            }
            (changes, opened, played)
        };
        self.change_dummies(changes);
        // The players' bodies' scope states (the console's scope 197 from
        // its scope 12 on a body, on the dummy a re-pick made), after the
        // dummies came.
        self.change_scopes(tick, opened);
        // A hero ability's victim graphs on the bodies it struck (Rocket
        // Punch's `0D07`), after the room's messages.
        self.play_scripts(tick, played);
    }

    /// The graphs the players' ability graphs played on other bodies
    /// (`lucy_script` `vm/play.rs` `Plays::Others`: Rocket Punch's
    /// `0D07`, Soundwave's `0ECA`) play in the struck player's body script
    /// or the struck bot's; a target nobody runs gets none. Then the
    /// knockbacks the bodies' graphs reported (`STU_AFBB59E9`, `0D07`
    /// rid63) between the room's players go to its Workshop rules
    /// (Player Dealt / Received Knockback); a bot's are dropped.
    fn play_scripts(&mut self, tick: u32, requests: Vec<PlayRequest>) {
        let room = &mut self.room;
        let Some(world) = &mut room.world else {
            return;
        };
        let mut peers = room_peers(&mut self.peers, &self.seats, room.players.len());
        for request in requests {
            let player = peers
                .iter_mut()
                .flatten()
                .find(|peer| peer.world.runs(request.target));
            match player {
                Some(player) => {
                    player.world.play_on_body(&request);
                }
                None => {
                    world.scene.play_on_bot(&request, tick);
                }
            }
        }
        world.scene.take_bot_knockbacks();
        let bodies: Vec<Option<u32>> = peers
            .iter()
            .map(|peer| peer.as_ref().and_then(|peer| peer.world.current_body()))
            .collect();
        let player_of = |body: u32| {
            bodies
                .iter()
                .position(|&current| current == Some(body))
                .map(|slot| room.players[slot].entity)
        };
        let mut events = Vec::new();
        for peer in peers.iter_mut().flatten() {
            for knockback in peer.world.take_knockbacks() {
                let (Some(attacker), Some(victim)) =
                    (player_of(knockback.attacker), player_of(knockback.victim))
                else {
                    continue;
                };
                events.push(PlayerEvent::Knockback {
                    attacker,
                    victim,
                    push: knockback.direction,
                    loadout: knockback.loadout,
                });
            }
        }
        if let Some(run) = room
            .workshop
            .as_mut()
            .and_then(|workshop| workshop.run.as_mut())
        {
            run.player_events(&events);
        }
    }

    /// The scope instances of the placed scripts' and the players' bodies'
    /// scope states (`lucy_script` `vm/scopes.rs`: the Training Partner
    /// console's scope 12 on the bodies near it, scope 126 on every
    /// controller, scope 197 from a body's scope 12 on a re-picked dummy)
    /// open or close in the target's player's scripts; an opened one goes
    /// back to its scope state for its frame record. A target no player
    /// runs gets none.
    pub(super) fn change_scopes(&mut self, tick: u32, changes: Vec<ScopeChange>) {
        let room = &mut self.room;
        let Some(world) = &mut room.world else {
            return;
        };
        let mut peers = room_peers(&mut self.peers, &self.seats, room.players.len());
        for change in changes {
            let target = change.target();
            let Some(player) = peers
                .iter_mut()
                .flatten()
                .find(|peer| peer.world.runs(target))
            else {
                continue;
            };
            let (Some(instance), ScopeChange::Open { source, .. }) =
                (player.world.change_scope(tick, &change), &change)
            else {
                continue;
            };
            if world.scene.placed_scripts().runs(source.entity) {
                world.scene.scope_opened(*source, target, instance);
            } else if let Some(owner) = peers
                .iter_mut()
                .flatten()
                .find(|peer| peer.world.runs(source.entity))
            {
                owner.world.scope_opened(*source, target, instance);
            }
        }
    }

    /// Every client of the room learns about the other players this tick
    /// (ow174/game/match.py `update`); their cards go reliably.
    pub(super) fn show_players(&mut self, tick: u32, now: Duration) {
        let room = &self.room;
        let Some(world) = &room.world else {
            return;
        };
        let mut connections = vec![None; room.players.len()];
        let mut players: Vec<_> = (0..room.players.len())
            .map(|slot| RoomPlayer {
                appearance: room.appearance(slot).expect("seated player"),
                world: None,
                dummy: room.players[slot].dummy,
            })
            .collect();
        for (&connection, peer) in &mut self.peers {
            let slot = self.seats[&connection];
            connections[slot] = Some(connection);
            players[slot].world = Some(&mut peer.world);
        }
        if connections.iter().all(Option::is_none) {
            return;
        }
        let cards = show_players(&world.scene, &mut players, tick, now);
        drop(players);
        for (connection, messages) in connections.into_iter().zip(cards) {
            let Some(peer) = connection.and_then(|connection| self.peers.get_mut(&connection))
            else {
                continue;
            };
            for message in messages {
                let _ = peer.reliable(&self.schemas, message);
            }
        }
    }

    /// ClientInVoice 27500 for the lines the players' bodies said this tick
    /// (the communication wheel's and the pain lines,
    /// `lucy_sim::voice_lines`): each goes to every client of the room
    /// whose body its audience lists (all for the wheel's; the hurt body
    /// and its attacker for a pain line) and is within the stimulus's
    /// radius of the speaker's (35 m for both), the speaker's own client
    /// too. Assumption: the client doesn't apply the radius itself; the
    /// bots' lines go out the same way.
    pub(super) fn say_player_lines(&mut self, tick: u32) {
        let Some(world) = self.room.world.as_ref() else {
            return;
        };
        let mut said: Vec<(VoiceLine, [f64; 3])> = Vec::new();
        for peer in self.peers.values_mut() {
            let lines = peer.world.take_voice_lines(&world.scene, tick);
            if let Some(position) = peer.world.hearing_position() {
                said.extend(lines.into_iter().map(|line| (line, position)));
            }
        }
        if said.is_empty() {
            return;
        }
        for peer in self.peers.values_mut() {
            if peer.ai.is_some() {
                continue;
            }
            let (Some(body), Some(position)) =
                (peer.world.current_body(), peer.world.hearing_position())
            else {
                continue;
            };
            for (line, speaker) in &said {
                if line.heard(body, *speaker, position) {
                    peer.notice(line.notice(body));
                }
            }
        }
    }
}
