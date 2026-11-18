//! The game mode entity's own statescript: the mode graph (`0F4A` for
//! Assault, Escort and Hybrid, `0EC2` for Control, `0D76`, `0D0B`, `0F60`,
//! `0F73` for the Arcade modes; `0C5` → the script of entity `16B2`) run on
//! the server against a [`ModeWorld`], as retail runs it on the entity the
//! server creates (ow1-preservation `Assault flow on Anubis`).
//!
//! The server's match flow feeds it the mode entity's variables (the clock
//! v1303, the attacking team `2A2B`, ...) and routes the messages it sends.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value as Json;

use crate::{
    graph::GraphStore,
    map_tags::MapTags,
    vm::{
        Component, GameMessage, InitFault, MAX_INIT_WORK, MapRays, Root, Site, Snapshot, Unknowns,
        World,
    },
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// The target the worlds give a message a graph sent to every entity
/// ([`World::broadcast_message`]); entity 0 is never one. The match hands
/// it to every entity that runs graphs on the server but the sender, whose
/// own instances took it when it was sent (`orpheus_sim::messages`).
pub const EVERY_ENTITY: u32 = 0;

/// The game struct's team byte that stands for any team (`STU_A5B9EAAD`'s
/// fallback entry).
const ANY_TEAM: i64 = 5;

/// A team's size in the game struct's team entries (team, size): the
/// entry of `team`, else the "any team" entry 5 (0x7FF789CADB40); the
/// client's last fallback, a table of its own, is not known, so no entry
/// leaves it unanswered.
pub fn team_size(entries: &[(u8, u8)], team: i64) -> Option<i64> {
    let entry = |wanted: i64| {
        entries
            .iter()
            .find(|&&(entry, _)| i64::from(entry) == wanted)
            .map(|&(_, size)| i64::from(size))
    };
    entry(team).or_else(|| entry(ANY_TEAM))
}

/// A player of the match as the mode graph knows him: his controller, his
/// current body (none while he has none) and his team as the client reads
/// it from the entity's filter bits (a free-for-all player is on team 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModePlayer {
    pub controller: u32,
    pub body: Option<u32>,
    pub team: i64,
}

/// The native configs whose answers come from the match's players and its
/// start condition ([`ModeScript::set_players`]).
const PLAYER_NATIVES: [&str; 3] = ["STU_A5B9EAAD", "STU_A90795E2", "STU_81A9AB10"];

/// What the mode graph asks outside its own entity. The mode entity's
/// variables are its own (the server writes them into the component);
/// the bundle's mode variables (`0C5.m_3CE93B76`) answer the rest.
pub struct ModeWorld {
    pub graphs: Arc<GraphStore>,
    pub mode: u64,
    pub unknowns: Unknowns,
    /// The match's players (AI heroes too): the client counts a team's
    /// entities but those of definition `003:25CB` (`STU_A5B9EAAD`).
    pub players: Vec<ModePlayer>,
    /// Each team's size as the server sends it in the 20300/20301 game
    /// struct (`#0.#0.#0.#4`: byte `+0x60` the team, 5 any team, `+0x61`
    /// its size; `orpheus_sim::instance`).
    pub team_sizes: Vec<(u8, u8)>,
    /// The server's answer to `STU_81A9AB10`, which lets the match leave
    /// its waiting phase.
    pub ready: bool,
    /// The match's value of the mode entity's component 114 field 9, as
    /// the server sends it (`STU_B182C5C9`); none: unknown.
    pub mode_random: Option<f32>,
    /// The match's random table index (`STU_CBF6FF14`, in `[0, 16384)`),
    /// drawn by the server; none: unknown.
    pub match_seed: Option<u16>,
    /// The game modes the match's map supports (its header's
    /// `m_supportedGamemodes`), which `STU_16753B11` tests; none: unknown.
    pub map_modes: Option<Vec<u64>>,
    /// The map's tagged placeables the scene tag queries find; none:
    /// unknown.
    pub map_tags: Option<Arc<MapTags>>,
    /// The map's collision the graph's ray casts meet; none: no map.
    pub map_rays: Option<Arc<dyn MapRays>>,
    outbox: Vec<GameMessage>,
}

impl ModeWorld {
    pub fn new(graphs: Arc<GraphStore>, mode: u64, unknowns: Unknowns) -> Self {
        Self {
            graphs,
            mode,
            unknowns,
            players: Vec::new(),
            team_sizes: Vec::new(),
            ready: false,
            mode_random: None,
            match_seed: None,
            map_modes: None,
            map_tags: None,
            map_rays: None,
            outbox: Vec::new(),
        }
    }

    /// The players on `team`.
    fn team_players(&self, team: i64) -> i64 {
        self.players
            .iter()
            .filter(|player| player.team == team)
            .count() as i64
    }

    /// The player whose controller or body `entity` is.
    fn player_of(&self, entity: u32) -> Option<&ModePlayer> {
        self.players
            .iter()
            .find(|player| player.controller == entity || player.body == Some(entity))
    }

    /// The game messages the graph sent other entities, for the match to
    /// route (like `PlayerWorld::take_messages`); [`EVERY_ENTITY`] targets
    /// every entity but the mode entity.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }
}

impl World for ModeWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        match class {
            // As `PlayerWorld`: the tunable's authored value, not scaled
            // for the entity.
            "STU_082FA44E" => Some("the tunable's authored value"),
            "STU_81A9AB10" => Some("the server's own start condition (server-only)"),
            "STU_21A5BB52" => Some("false (server-only)"),
            "STU_A90795E2" => Some("the team's players (server-only)"),
            _ => None,
        }
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, 0, self.mode)
    }

    fn mode_random(&self) -> Option<f64> {
        self.mode_random.map(f64::from)
    }

    fn match_seed(&self) -> Option<i64> {
        self.match_seed.map(i64::from)
    }

    fn team_size(&self, team: i64) -> Option<i64> {
        team_size(&self.team_sizes, team)
    }

    fn map_supports_mode(&self, _site: Site, mode: u64) -> Option<bool> {
        Some(self.map_modes.as_ref()?.contains(&mode))
    }

    /// The mode entity has no team: the server gives it no component 25
    /// (`orpheus_sim::scene::mode_create`), so its filter word is empty.
    /// A player's controller and body are on his team.
    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        if entity == GAME_MODE_ENTITY {
            return Some(-1);
        }
        Some(self.player_of(entity)?.team)
    }

    fn entity_filter_flags(&self, _site: Site, entity: u32) -> Option<u64> {
        (entity == GAME_MODE_ENTITY).then_some(0)
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        self.map_rays.as_deref()
    }

    /// The map's placeables that carry the tag ([`MapTags::with_tag`]).
    fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
        self.map_tags.as_ref()?.with_tag(tag)
    }

    fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
        let placed = self.map_tags.as_ref()?.get(entity)?;
        Some(placed.tags.contains(&tag))
    }

    /// The mode entity, the players' controllers and bodies and the map's
    /// tagged placeables exist.
    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        let known = entity == GAME_MODE_ENTITY
            || self.player_of(entity).is_some()
            || self
                .map_tags
                .as_ref()
                .is_some_and(|tags| tags.get(entity).is_some());
        known.then_some(true)
    }

    fn entity_position(&self, _site: Site, entity: u32) -> Option<[f64; 3]> {
        Some(self.map_tags.as_ref()?.get(entity)?.position)
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.graphs
            .mode_variables(self.mode)
            .and_then(|variables| variables.get(&id))
            .copied()
            .map_or(Value::Null, Value::from)
    }

    /// `STU_A3DEF368` is the first entity with component 114, the mode
    /// entity itself (ow1-preservation: getter 0xcdd340). `STU_082FA44E`
    /// reads a tunable's authored value, as `PlayerWorld` does.
    ///
    /// `STU_A5B9EAAD` (`m_team`) is "the team is full", a bool: the count
    /// of the team's entities but those of definition `003:25CB`, at least
    /// the team's size ([`World::team_size`]). `Eval_STU_A5B9EAAD`
    /// 0x7FF789BF9E10, size `sub_7FF789CADB40` (ModeGraphGaps.md, "Client
    /// tools pass (2026-11-16)"). Its uses ask both teams 0 and 1 before
    /// the match leaves its waiting phase (`0F4A` nodes 14 and 419, `0EC2`
    /// 15 and 380, `0D0B` 237, `0F60` 27), or the free-for-all team 4
    /// (`0F73` 16), so the start rule `25CA` waits for full teams: a game
    /// with fewer players than its team sizes waits there (the start rule
    /// `25CB` waits for message `0789` instead). The evaluator makes the
    /// asking state depend on the game's player list (channel 2 of the
    /// game's dependency table, `sub_7FF789AEBA60`), so a player who joins
    /// re-checks the waiting state's transition ([`ModeScript::set_players`]).
    ///
    /// `STU_81A9AB10` (a bool with no fields) gates the same start in the
    /// default start rule (ruleset `#9672` = `25CA`; `0F4A` node 7: true
    /// waits for both teams, false waits for message `0789`), and Tutorial
    /// graph `091B` waits on it to send `01DB` to the mode entity. It is
    /// server-only (no client code: the base config's evaluate, `xor al,
    /// al`); the server answers [`ModeWorld::ready`], its own rule.
    ///
    /// `STU_21A5BB52` (a bool with no fields) picks, outside Competitive
    /// play, whether the payload's random speed factor #29093 comes from
    /// message `11BA` (`0F4A` uid 1299); `0F60` and `0F73` read it in
    /// `STU_F7216D4F`. It is server-only (no client code); the server
    /// answers false, its own rule.
    ///
    /// `STU_A90795E2` (`m_6CE7A900`, a team) is server-only too (the base
    /// config's evaluate). Its uses read it as the team's players: `1061`
    /// (Team Deathmatch and Deathmatch, on `06A6`) creates the team's size
    /// less it in AI heroes, `0D44` holds a count to it, and Elimination
    /// (`0D0B` rid181) gives the round to team 0 when it has players and
    /// team 1 none. The server answers [`ModeWorld::players`]' count.
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        let literal_team = |field: &str| {
            let team = config.get(field)?;
            if team.get("$")?.as_str()? != "STU_BD6CC023" {
                return None;
            }
            team.get("m_6CE7A900")?.as_i64()
        };
        match config.get("$")?.as_str()? {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            "STU_A5B9EAAD" => {
                let team = literal_team("m_team")?;
                Some(Value::Bool(
                    self.team_players(team) >= self.team_size(team)?,
                ))
            }
            "STU_A90795E2" => Some(Value::Integer(
                self.team_players(literal_team("m_6CE7A900")?),
            )),
            "STU_81A9AB10" => Some(Value::Bool(self.ready)),
            "STU_21A5BB52" => Some(Value::Bool(false)),
            _ => None,
        }
    }

    fn send_message(
        &mut self,
        _source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        self.outbox.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    fn broadcast_message(&mut self, _source: Site, message: u64, parameters: &Variables) -> bool {
        self.outbox.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target: EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }
}

/// The mode graph running on the mode entity.
pub struct ModeScript {
    component: Component,
    world: ModeWorld,
    frame: u32,
}

impl ModeScript {
    /// Start `graph` as the mode entity's instance 1 at `frame`, against
    /// `world`.
    /// The mode entity's own variables start as its mode's
    /// (`0C5.m_3CE93B76`, the bundle's `mode_variables`): the Practice
    /// Range `005A`'s `#27534` true makes `13C0`'s #27536 false, the board
    /// poster st6 and the open doors st18, where `0018`'s false takes the
    /// training branch (st11, `0BD6`).
    pub fn start(mut world: ModeWorld, graph: u32, frame: u32) -> Result<Self, InitFault> {
        let graphs = Arc::clone(&world.graphs);
        let variables: Variables = graphs
            .mode_variables(world.mode)
            .into_iter()
            .flatten()
            .map(|(&id, &value)| (id, Value::from(value)))
            .collect();
        let mut component = Component::start(
            graphs,
            GAME_MODE_ENTITY,
            variables,
            vec![Root::new(1, graph)],
            frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        component.advance_frame(frame, &mut world)?;
        Ok(Self {
            component,
            world,
            frame,
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    pub fn unknowns(&self) -> Unknowns {
        self.world.unknowns
    }

    /// Whether the graph ran only on what the VM implements: no gap was
    /// refused (a diagnostic) and no answer was an assumption.
    pub fn complete(&self) -> bool {
        self.snapshot().is_some_and(|snapshot| {
            snapshot.initialization_complete() && snapshot.assumptions.is_empty()
        })
    }

    /// The classes the VM refused or answered by assumption, with how
    /// often, by graph: what keeps the graph from driving the match.
    pub fn gaps(&self) -> BTreeMap<(u32, String), usize> {
        let mut gaps = BTreeMap::new();
        let Some(snapshot) = self.snapshot() else {
            return gaps;
        };
        for assumption in &snapshot.assumptions {
            *gaps
                .entry((assumption.site.graph, assumption.class.to_string()))
                .or_default() += 1;
        }
        for diagnostic in &snapshot.diagnostics {
            *gaps
                .entry((diagnostic.site.graph, format!("{:?}", diagnostic.issue)))
                .or_default() += 1;
        }
        gaps
    }

    /// Write the mode entity's variables the server owns.
    pub fn set_variables(&mut self, variables: &Variables) -> Result<(), InitFault> {
        for (&id, value) in variables {
            self.component
                .set_entity_variable(id, value.clone(), &mut self.world)?;
        }
        Ok(())
    }

    /// The match's players and its start condition now. When either
    /// changed, the states that asked about them take the change
    /// ([`Component::natives_changed`]): a waiting state tries its
    /// transition again, as the client's does when a player joins.
    pub fn set_players(&mut self, players: Vec<ModePlayer>, ready: bool) -> Result<(), InitFault> {
        if self.world.players == players && self.world.ready == ready {
            return Ok(());
        }
        self.world.players = players;
        self.world.ready = ready;
        self.component
            .natives_changed(&PLAYER_NATIVES, &mut self.world)
    }

    /// Advance to a later server frame; false for an old one.
    pub fn advance(&mut self, frame: u32) -> Result<bool, InitFault> {
        if frame <= self.frame {
            return Ok(false);
        }
        self.frame = frame;
        self.component.advance_frame(frame, &mut self.world)?;
        Ok(true)
    }

    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        self.world.take_messages()
    }

    /// A game message from `sender` to the mode entity (or to every
    /// entity): the graph's entries for it run now. Returns how many took
    /// it.
    pub fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        self.component
            .receive_message(sender, message, parameters, &mut self.world)
    }

    /// A persistent sender (`sender`) let its message to the mode entity
    /// go ([`Component::end_message`]).
    pub fn end_message(&mut self, sender: u32, message: u64) -> Result<(), InitFault> {
        self.component.end_message(sender, message, &mut self.world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codecs::CodecTable,
        snapshot::{SnapshotEncoder, View},
        timeline::FRAME_MS,
    };

    const ASSAULT: u64 = 0x0230_0000_0000_0014;
    const ESCORT: u64 = 0x0230_0000_0000_0015;
    const TEAM_DEATHMATCH: u64 = 0x0230_0000_0000_001d;
    /// The mode graph's phase (v59 on its instance 1).
    const PHASE: u32 = 59;

    fn graphs() -> Arc<GraphStore> {
        let data = orpheus_testkit::runtime_dir().join("data");
        Arc::new(GraphStore::load_runtime(data, "game_graphs_174.json.gz").unwrap())
    }

    /// One player on each of teams 0 and 1.
    fn one_a_team() -> Vec<ModePlayer> {
        (0..2)
            .map(|team| ModePlayer {
                controller: 0x100 + team as u32,
                body: Some(0x200 + team as u32),
                team,
            })
            .collect()
    }

    /// Team Deathmatch's `0F60` waits in phase 0 (#59e) while its teams of
    /// one aren't full, and leaves it when the players join, without the
    /// waiting state beginning again: the default start rule `25CA` (st7's
    /// transition, rid82 → rid240 `STU_81A9AB10` → rid21 both teams'
    /// `STU_A5B9EAAD` → rid19 #59e = 1) is tried again when the players
    /// change. A change posts the check at its frame's time, so the next
    /// frame runs it: one player at frame 100 keeps phase 0 at 101 (team 1
    /// isn't full), both at 101 make phase 1 (assemble) at 102.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn a_waiting_mode_graph_starts_when_the_players_join() {
        let mut world = ModeWorld::new(graphs(), TEAM_DEATHMATCH, Unknowns::Tolerate);
        world.team_sizes = vec![(0, 1), (1, 1)];
        world.ready = true;
        let mut script = ModeScript::start(world, 0x0f60, 1).unwrap();
        let phase = |script: &ModeScript| script.snapshot().unwrap().variables.get(&PHASE).cloned();
        for frame in 2..=100 {
            script.advance(frame).unwrap();
        }
        assert_eq!(phase(&script), Some(Value::Float(0.0)));
        let mut half = one_a_team();
        half.truncate(1);
        script.set_players(half, true).unwrap();
        script.advance(101).unwrap();
        assert_eq!(phase(&script), Some(Value::Float(0.0)));
        script.set_players(one_a_team(), true).unwrap();
        script.advance(102).unwrap();
        assert_eq!(phase(&script), Some(Value::Integer(1)));
    }

    /// What a mode graph did in a match: its phase changes and the first
    /// time each gap was met, by frame.
    struct Run {
        script: ModeScript,
        phases: Vec<(u32, Value)>,
        gaps: Vec<(u32, u32, Option<u16>, String)>,
        messages: Vec<(u32, u64, u32)>,
    }

    /// Run `graph` for `mode` from frame 1 with one player a team and the
    /// start condition true, tolerating gaps, for `frames` frames.
    fn play(graphs: &Arc<GraphStore>, mode: u64, graph: u32, frames: u32) -> Run {
        let mut world = ModeWorld::new(Arc::clone(graphs), mode, Unknowns::Tolerate);
        world.players = one_a_team();
        world.team_sizes = vec![(0, 1), (1, 1)];
        world.ready = true;
        let mut run = Run {
            script: ModeScript::start(world, graph, 1).unwrap(),
            phases: Vec::new(),
            gaps: Vec::new(),
            messages: Vec::new(),
        };
        let mut seen = 0;
        for frame in 1..=frames {
            if frame > 1 {
                run.script.advance(frame).unwrap();
            }
            let snapshot = run.script.snapshot().unwrap();
            for gap in &snapshot.assumptions[seen..] {
                run.gaps
                    .push((frame, gap.site.graph, gap.site.node, gap.class.clone()));
            }
            seen = snapshot.assumptions.len();
            let phase = snapshot.instances[&1]
                .variables
                .get(&PHASE)
                .cloned()
                .unwrap_or(Value::Null);
            if run.phases.last().is_none_or(|(_, last)| *last != phase) {
                run.phases.push((frame, phase));
            }
            for message in run.script.take_messages() {
                run.messages.push((frame, message.message, message.target));
            }
        }
        run
    }

    /// How far the mode graph `0F4A` runs an Assault and an Escort match
    /// today, and the first gap it meets: the measure of what keeps it from
    /// driving the match instead of `match_flow.rs`. Debug builds refuse it.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn the_mode_graph_runs_assault_and_escort_until_its_first_gap() {
        let graphs = graphs();
        let refused = ModeScript::start(
            ModeWorld::new(Arc::clone(&graphs), ASSAULT, Unknowns::Refuse),
            0x0f4a,
            1,
        );
        assert!(refused.map_or(true, |script| !script.complete()));
        let seconds = |frame: u32| f64::from(frame) * FRAME_MS as f64 / 1000.0;
        for mode in [ASSAULT, ESCORT] {
            let run = play(&graphs, mode, 0x0f4a, 60_000);
            println!("== mode {mode:016X}");
            for (frame, phase) in &run.phases {
                println!(
                    "phase {phase:?} at frame {frame} ({:.2} s)",
                    seconds(*frame)
                );
            }
            for (frame, graph, node, class) in &run.gaps {
                println!("gap {graph:04X} node {node:?} {class} at frame {frame}");
            }
            for (frame, message, target) in run.messages.iter().take(60) {
                println!("message {message:X} to {target:X} at frame {frame}");
            }
            let snapshot = run.script.snapshot().unwrap();
            let codecs = CodecTable::client();
            let encoder = SnapshotEncoder::new(&graphs, &codecs).with_unknowns(Unknowns::Tolerate);
            let projected = encoder.project(snapshot, View::Observer).unwrap();
            assert!(encoder.full(&projected).is_ok());
        }
    }
}
