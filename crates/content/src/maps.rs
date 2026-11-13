//! Match map selection and spawn transforms for the 104319 runtime catalog.
//!
//! Ported from the map portion of ow174/game/content.py.

use std::{collections::HashMap, fs, io, path::Path};

use indexmap::IndexMap;
use serde::Deserialize;

use orpheus_core::guid::parse_guid;

pub const PRACTICE_RANGE_MAP: u64 = 0x0800_0000_0000_0688;
/// The group Practice Range (`005A`): both teams have the PvP controller
/// `0C90` and the blue team takes up to six players. Mode `0018` (the solo
/// Range, controller `13C1` for one player) has the same map, rulesets,
/// placeables and bots; Python ran `0018`, retail's Training > Practice
/// Range is open to the whole party (docs/notes/behavior.md).
pub const PRACTICE_RANGE_MODE: u64 = 0x0230_0000_0000_005a;
/// Python's Practice Range mode (`0018`), for the tests that compare our
/// output with the Python reference's.
pub const PYTHON_PRACTICE_RANGE_MODE: u64 = 0x0230_0000_0000_0018;
/// The Tutorial game mode (`TUTORIAL_MODE` in `content.py` of
/// leafall903/ORPHEUS, commit c239a84).
pub const TUTORIAL_MODE: u64 = 0x0230_0000_0000_0019;
pub const SPAWN_SPACING: f64 = 1.5;
/// The map catalog in the runtime `data` folder (the `game_maps` derivation).
pub const MAPS_FILE: &str = "game_maps_174.json";
/// The game modes' teams and loading rule (the `game_modes` derivation).
pub const MODES_FILE: &str = "game_modes_174.json";

/// The Practice Range's name in the install (`09F` 0688), for the test
/// fixture [`GameMap::practice_range`].
const PRACTICE_RANGE_NAME: &str = "Practice Range";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Spawn {
    pub team: Option<u8>,
    pub position: [f64; 3],
    #[serde(default)]
    pub yaw: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameMap {
    pub name: String,
    pub map_guid: u64,
    pub mode_guid: u64,
    pub placeables: Vec<u32>,
    pub spawns: Vec<Spawn>,
    pub mode_name: String,
    pub team_sizes: Vec<u8>,
    pub free_for_all: bool,
    /// The map's objectives in this mode, as derived from their statescript
    /// instances (`game_maps` `objectives`): capture points in their order,
    /// the payload and its checkpoints, the Control stages.
    pub objectives: Vec<MapObjective>,
    /// The teams' spawn rooms with the objectives or the Control stage
    /// they serve (`game_maps` `objectives` and `stage`), in source order.
    pub rooms: Vec<SpawnRoom>,
}

/// A spawn point of a team's spawn room and when the mode uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnRoom {
    pub spawn: Spawn,
    /// Assault, Escort and Hybrid: the objectives (their order, 0x2934)
    /// during which the room is the defenders' (false) or the attackers'
    /// (true): its target tag is the objective's 0x2935 or 0x2936.
    pub objectives: Vec<(u32, bool)>,
    /// Control: the stage whose round it serves (tags 0x310/0x341 stage 0,
    /// 0x30D/0x3CC stage 1, 0x3CE/0x3CD stage 2).
    pub stage: Option<u32>,
}

/// Which spawn rooms a team uses now (the match flow's choice).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpawnChoice {
    /// The first round's rooms ([`GameMap::spawns`]).
    #[default]
    First,
    /// The attackers' or the defenders' rooms of an objective.
    Objective { objective: u32, attack: bool },
    /// The team's room of a Control stage.
    Stage(u32),
}

/// What an objective entity of the map is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectiveKind {
    /// A capture point of Assault or Hybrid (graphs 0F4C and 0E8E).
    Point,
    /// A payload checkpoint of Escort or Hybrid (0F4C with var 0x2931).
    Checkpoint,
    /// The payload itself (graph 1225).
    Payload,
    /// A Control stage's point (0E8E with tag 0x41B..0x41D).
    ControlPoint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapObjective {
    pub kind: ObjectiveKind,
    /// The objective's order (0x2934), or the Control stage; 0 for the
    /// payload.
    pub order: u32,
    /// The point's display name (0x1DDB, or 0x6D9 for Control).
    pub name: Option<String>,
    /// The entity placeable's flat index.
    pub placeable: u32,
    pub position: [f64; 3],
}

impl GameMap {
    /// The objectives of a kind in their order.
    pub fn objectives_of(&self, kind: ObjectiveKind) -> impl Iterator<Item = &MapObjective> {
        let mut chosen: Vec<_> = self
            .objectives
            .iter()
            .filter(|objective| objective.kind == kind)
            .collect();
        chosen.sort_by_key(|objective| objective.order);
        chosen.into_iter()
    }

    /// A copy of the Practice Range record, for tests that run without the
    /// install: the record of `ow174/game/content.py` (`PRACTICE_RANGE`).
    /// The server uses [`MapCatalog::practice_range`], read from the install;
    /// the game-tier test `the_practice_range_is_the_derived_record` checks
    /// that this copy is that record (its spawn as Python rounded it). The
    /// map has four server-owned placeables (the ones the running client
    /// has), and its mode has six players on the blue team and none on the
    /// red one.
    pub fn practice_range() -> Self {
        Self {
            name: PRACTICE_RANGE_NAME.into(),
            map_guid: PRACTICE_RANGE_MAP,
            mode_guid: PRACTICE_RANGE_MODE,
            placeables: vec![0x88, 0x22e, 0x234, 0x289],
            spawns: vec![Spawn {
                team: None,
                position: [54.56507, 1.0, 42.14788],
                yaw: -170.3434,
            }],
            mode_name: PRACTICE_RANGE_NAME.into(),
            team_sizes: vec![6, 0],
            free_for_all: false,
            objectives: Vec::new(),
            rooms: Vec::new(),
        }
    }

    /// Cycle through a team's points (or all points if that team has none).
    /// Players sharing a point alternate along its local right-hand axis.
    pub fn spawn_point(&self, team: u8, number: usize) -> Option<([f64; 3], f64)> {
        let mine: Vec<&Spawn> = self
            .spawns
            .iter()
            .filter(|spawn| spawn.team.is_none() || spawn.team == Some(team))
            .collect();
        if mine.is_empty() {
            place(&self.spawns.iter().collect::<Vec<_>>(), number)
        } else {
            place(&mine, number)
        }
    }

    /// The `number`-th point of the rooms `choice` names for `team`, as
    /// [`Self::spawn_point`] places them; the first round's points when the
    /// map has no such room. An objective's rooms are the attackers' or the
    /// defenders' whatever their tag's team: the teams swap sides.
    pub fn spawn_point_in(
        &self,
        team: u8,
        number: usize,
        choice: SpawnChoice,
    ) -> Option<([f64; 3], f64)> {
        let chosen: Vec<&Spawn> = self
            .rooms
            .iter()
            .filter(|room| match choice {
                SpawnChoice::First => false,
                SpawnChoice::Objective { objective, attack } => {
                    room.objectives.contains(&(objective, attack))
                }
                SpawnChoice::Stage(stage) => {
                    room.stage == Some(stage) && room.spawn.team == Some(team)
                }
            })
            .map(|room| &room.spawn)
            .collect();
        if chosen.is_empty() {
            self.spawn_point(team, number)
        } else {
            place(&chosen, number)
        }
    }
}

/// The `number`-th of `spawns`, rows past the first offset sideways.
fn place(spawns: &[&Spawn], number: usize) -> Option<([f64; 3], f64)> {
    let index = number.checked_rem(spawns.len())?;
    let spawn = spawns[index];
    let row = number / spawns.len();
    let side = row.div_ceil(2) as f64 * SPAWN_SPACING;
    let side = if row != 0 && row % 2 == 0 {
        -side
    } else {
        side
    };
    let (sin, cos) = spawn.yaw.to_radians().sin_cos();
    let [x, y, z] = spawn.position;
    Some(([x - cos * side, y, z + sin * side], spawn.yaw))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapListing {
    pub guid: u64,
    pub name: String,
}

#[derive(Debug, Default)]
pub struct MapCatalog {
    entries: IndexMap<u64, MapEntry>,
    modes: GameModes,
}

impl MapCatalog {
    /// Read the map catalog and the game modes from the runtime `data`
    /// folder; the Practice Range comes from them too.
    pub fn load(data: impl AsRef<Path>) -> io::Result<Self> {
        let data = data.as_ref();
        let path = data.join(MAPS_FILE);
        let text = fs::read_to_string(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Ok(Self {
            modes: GameModes::load(data.join(MODES_FILE))?,
            ..Self::from_json(&text)?
        })
    }

    /// The catalog alone, without the modes' rules.
    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: IndexMap<String, RawMap> = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut entries = IndexMap::with_capacity(raw.len());
        for (guid, raw) in raw {
            let modes = raw
                .modes
                .iter()
                .map(|mode| hex_guid(mode))
                .collect::<io::Result<_>>()?;
            let placeables_by_mode = raw
                .placeables_by_mode
                .unwrap_or_default()
                .into_iter()
                .map(|(mode, indexes)| Ok((hex_guid(&mode)?, indexes)))
                .collect::<io::Result<_>>()?;
            let spawns = raw
                .spawns
                .unwrap_or_default()
                .into_iter()
                .map(|entry| SpawnEntry {
                    spawn: entry.spawn,
                    free_for_all: entry.ffa.unwrap_or_default(),
                    objectives: entry
                        .objectives
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(objective, role)| (objective, role == "attack"))
                        .collect(),
                    stage: entry.stage,
                    rotation: entry.rotation,
                })
                .collect();
            entries.insert(
                hex_guid(&guid)?,
                MapEntry {
                    name: raw.name,
                    modes,
                    placeables: raw.placeables,
                    placeables_by_mode,
                    spawns,
                    objectives: raw.objectives.unwrap_or_default(),
                },
            );
        }
        Ok(Self {
            entries,
            modes: GameModes::default(),
        })
    }

    /// The catalog with these modes.
    pub fn with_modes(mut self, modes: GameModes) -> Self {
        self.modes = modes;
        self
    }

    /// Catalog order is stable among entries with the same display name.
    pub fn map_catalog(&self) -> Vec<MapListing> {
        let mut maps: Vec<MapListing> = self
            .entries
            .iter()
            .map(|(&guid, entry)| {
                let name = match entry.name.as_deref() {
                    Some(name) if !name.is_empty() => name.to_owned(),
                    _ => format!("0x{guid:X}"),
                };
                MapListing { guid, name }
            })
            .collect();
        maps.sort_by(|left, right| left.name.cmp(&right.name));
        maps
    }

    pub fn map_name(&self, guid: u64) -> Option<&str> {
        self.entries.get(&guid)?.name.as_deref()
    }

    /// The Practice Range as the install has it (the record of
    /// `ow174/game/content.py` `PRACTICE_RANGE`): map 0688's name and its
    /// placeables in mode 005A, its team 0 spawn point (for every team), and
    /// the mode's team sizes. Python's record rounded the spawn point: its
    /// position to 5 decimals, its yaw (from the rotation) to 4; so does
    /// this one. An error when the data no longer has them.
    pub fn practice_range(&self) -> io::Result<GameMap> {
        let invalid = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the Practice Range (map 0688, mode 005A) {what}"),
            )
        };
        let entry = self
            .entries
            .get(&PRACTICE_RANGE_MAP)
            .ok_or_else(|| invalid("is not in the map catalog"))?;
        let name = entry
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| invalid("has no name"))?;
        if !entry.modes.contains(&PRACTICE_RANGE_MODE) {
            return Err(invalid("is not played in its mode"));
        }
        let placeables = entry
            .placeables_by_mode
            .get(&PRACTICE_RANGE_MODE)
            .ok_or_else(|| invalid("has no placeables for its mode"))?
            .clone();
        let spawn = entry
            .spawns
            .iter()
            .find(|entry| !entry.free_for_all && entry.spawn.team == Some(0))
            .ok_or_else(|| invalid("has no team 0 spawn point"))?;
        let [_, y, _, w] = spawn
            .rotation
            .ok_or_else(|| invalid("spawn point has no rotation"))?;
        let round = |value: f64, places: i32| {
            let scale = 10f64.powi(places);
            (value * scale).round() / scale
        };
        let mode = self
            .modes
            .get(PRACTICE_RANGE_MODE)
            .ok_or_else(|| invalid("mode is not in the game modes"))?;
        if mode.teams.iter().any(|team| team.team == FREE_FOR_ALL_TEAM) {
            return Err(invalid("mode is free for all"));
        }
        Ok(GameMap {
            name: name.clone(),
            map_guid: PRACTICE_RANGE_MAP,
            mode_guid: PRACTICE_RANGE_MODE,
            placeables,
            spawns: vec![Spawn {
                team: None,
                position: spawn.spawn.position.map(|value| round(value, 5)),
                yaw: round(2.0 * y.atan2(w).to_degrees(), 4),
            }],
            mode_name: name,
            team_sizes: mode.teams.iter().map(|team| team.players).collect(),
            free_for_all: false,
            objectives: Vec::new(),
            rooms: Vec::new(),
        })
    }

    pub fn game_map(
        &self,
        map_guid: u64,
        mode_guid: u64,
        mode_name: &str,
        free_for_all: bool,
        team_sizes: &[u8],
    ) -> Option<GameMap> {
        // A forced Practice Range plays its own mode (the server's
        // matchmaker checked at load that the data has it).
        if map_guid == PRACTICE_RANGE_MAP {
            return self.practice_range().ok();
        }
        let entry = self.entries.get(&map_guid)?;
        if self.modes.scripted_loading(mode_guid) || !entry.modes.contains(&mode_guid) {
            return None;
        }
        let spawns = entry.first_round_spawns(free_for_all);
        if spawns.is_empty() {
            return None;
        }
        Some(GameMap {
            name: entry.name.clone()?,
            map_guid,
            mode_guid,
            placeables: entry
                .placeables_by_mode
                .get(&mode_guid)
                .unwrap_or(&entry.placeables)
                .clone(),
            spawns,
            mode_name: mode_name.into(),
            team_sizes: team_sizes.into(),
            free_for_all,
            objectives: entry
                .objectives
                .iter()
                .map(|objective| MapObjective {
                    kind: objective.kind,
                    order: objective.objective.or(objective.stage).unwrap_or(0),
                    name: objective.name.clone(),
                    placeable: objective.placeable,
                    position: objective.position,
                })
                .collect(),
            rooms: entry
                .spawns
                .iter()
                .filter(|entry| !entry.free_for_all && entry.spawn.team.is_some())
                .map(|entry| SpawnRoom {
                    spawn: entry.spawn,
                    objectives: entry.objectives.clone(),
                    stage: entry.stage,
                })
                .collect(),
        })
    }
}

#[derive(Debug)]
struct MapEntry {
    name: Option<String>,
    modes: Vec<u64>,
    placeables: Vec<u32>,
    placeables_by_mode: HashMap<u64, Vec<u32>>,
    spawns: Vec<SpawnEntry>,
    objectives: Vec<RawObjective>,
}

impl MapEntry {
    fn first_round_spawns(&self, free_for_all: bool) -> Vec<Spawn> {
        if free_for_all {
            return self
                .spawns
                .iter()
                .filter(|entry| entry.free_for_all || entry.spawn.team.is_none())
                .map(|entry| entry.spawn)
                .collect();
        }
        let mut chosen = Vec::new();
        for team in [0, 1] {
            let mine = self
                .spawns
                .iter()
                .filter(|entry| !entry.free_for_all && entry.spawn.team == Some(team));
            let has_first = mine.clone().any(|entry| entry.first_objective());
            chosen.extend(
                mine.filter(|entry| !has_first || entry.first_objective())
                    .map(|entry| entry.spawn),
            );
        }
        chosen
    }
}

#[derive(Debug)]
struct SpawnEntry {
    spawn: Spawn,
    free_for_all: bool,
    objectives: Vec<(u32, bool)>,
    stage: Option<u32>,
    /// The point's rotation quaternion (x, y, z, w).
    rotation: Option<[f64; 4]>,
}

impl SpawnEntry {
    fn first_objective(&self) -> bool {
        self.objectives.iter().any(|(objective, _)| *objective == 0)
    }
}

/// The game modes (`0C5`) as the derivation `game_modes` describes them.
#[derive(Debug, Default)]
pub struct GameModes {
    modes: HashMap<u64, GameMode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameMode {
    /// `m_teams`, in the asset's order.
    pub teams: Vec<ModeTeam>,
    /// The loading screen waits for a state of the mode's own script
    /// (`m_0FC17230` = 1; the loader's gate at 0x7FF789A271E0), so a server
    /// has to run that script first (`SCRIPTED_LOADING_MODES` in
    /// `ow174/game/content.py`).
    pub scripted_loading: bool,
}

/// [`ModeTeam::team`] of a free-for-all mode's one team.
const FREE_FOR_ALL_TEAM: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeTeam {
    /// `TeamIndex`: 0 blue, 1 red, 4 free for all.
    pub team: i64,
    /// The team's controller graph (`01B`), when it has one.
    pub controller: Option<u64>,
    /// `m_170AA4B8`: the team's players.
    pub players: u8,
}

impl GameModes {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawModes = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let modes = raw
            .modes
            .into_iter()
            .map(|(guid, mode)| {
                let teams = mode
                    .teams
                    .into_iter()
                    .map(|team| {
                        Ok(ModeTeam {
                            team: team.team,
                            controller: team.controller.as_deref().map(hex_guid).transpose()?,
                            players: team.players,
                        })
                    })
                    .collect::<io::Result<_>>()?;
                let mode = GameMode {
                    teams,
                    scripted_loading: mode.scripted_loading,
                };
                Ok((hex_guid(&guid)?, mode))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { modes })
    }

    pub fn get(&self, mode: u64) -> Option<&GameMode> {
        self.modes.get(&mode)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, &GameMode)> {
        self.modes.iter().map(|(&guid, mode)| (guid, mode))
    }

    /// Whether the mode's loading screen waits for its own script; the
    /// server can't host those modes (`content.py` `game_map`).
    pub fn scripted_loading(&self, mode: u64) -> bool {
        self.get(mode).is_some_and(|mode| mode.scripted_loading)
    }
}

#[derive(Deserialize)]
struct RawModes {
    modes: HashMap<String, RawMode>,
}

#[derive(Deserialize)]
struct RawMode {
    teams: Vec<RawTeam>,
    scripted_loading: bool,
}

#[derive(Deserialize)]
struct RawTeam {
    team: i64,
    controller: Option<String>,
    players: u8,
}

#[derive(Deserialize)]
struct RawMap {
    name: Option<String>,
    #[serde(default)]
    modes: Vec<String>,
    #[serde(default)]
    placeables: Vec<u32>,
    placeables_by_mode: Option<IndexMap<String, Vec<u32>>>,
    spawns: Option<Vec<RawSpawn>>,
    objectives: Option<Vec<RawObjective>>,
}

#[derive(Debug, Deserialize)]
struct RawObjective {
    #[serde(deserialize_with = "objective_kind")]
    kind: ObjectiveKind,
    objective: Option<u32>,
    stage: Option<u32>,
    name: Option<String>,
    placeable: u32,
    position: [f64; 3],
}

fn objective_kind<'de, D: serde::Deserializer<'de>>(input: D) -> Result<ObjectiveKind, D::Error> {
    let text = String::deserialize(input)?;
    match text.as_str() {
        "point" => Ok(ObjectiveKind::Point),
        "checkpoint" => Ok(ObjectiveKind::Checkpoint),
        "payload" => Ok(ObjectiveKind::Payload),
        "control point" => Ok(ObjectiveKind::ControlPoint),
        _ => Err(serde::de::Error::custom(format!(
            "unknown objective kind {text:?}"
        ))),
    }
}

#[derive(Deserialize)]
struct RawSpawn {
    #[serde(flatten)]
    spawn: Spawn,
    ffa: Option<bool>,
    objectives: Option<Vec<(u32, String)>>,
    stage: Option<u32>,
    rotation: Option<[f64; 4]>,
}

fn hex_guid(text: &str) -> io::Result<u64> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let parsed = match digits.as_bytes() {
        [b'0', b'x' | b'X', ..] => parse_guid(digits),
        [b'_', ..] => None,
        _ => parse_guid(&format!("0x{digits}")),
    };
    parsed.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid map or mode GUID {text:?}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARENA: &str = r#"{
        "0x80": {
            "name": "Fixture Arena",
            "modes": ["20", "0x21", "0X22", "0x0230000000000025"],
            "placeables": [8, 4, 8],
            "placeables_by_mode": {"0x20": [], "21": [3, 1, 3]},
            "spawns": [
                {"team": 1, "position": [8, 1, 0], "objectives": [[1, "attack"]]},
                {"team": 0, "position": [1, 1, 0], "objectives": [[2, "defense"]]},
                {"team": 0, "position": [9, 1, 0], "objectives": [[0, "defense"]], "ffa": true},
                {"team": 0, "position": [2, 1, 0], "objectives": [[0, "defense"]], "yaw": 90},
                {"team": 1, "position": [3, 1, 0], "objectives": null},
                {"team": 0, "position": [4, 1, 0], "objectives": [[1, "defense"], [0, "defense"]]},
                {"team": null, "position": [5, 1, 0]},
                {"position": [6, 1, 0], "ffa": false},
                {"team": 1, "position": [7, 1, 0], "ffa": true}
            ]
        },
        "0x81": {"name": "No points", "modes": ["20"], "spawns": null}
    }"#;

    const MODES: &str = r#"{"modes": {
        "0x0230000000000025": {"teams": [
            {"team": 0, "controller": "0x0580000000000F44", "players": 4},
            {"team": 1, "controller": null, "players": 0}
        ], "scripted_loading": true},
        "0x20": {"teams": [], "scripted_loading": false}
    }}"#;

    #[test]
    fn objectives_load_in_their_order_per_kind() {
        let catalog = MapCatalog::from_json(
            r#"{"0x80": {"name": "Hybrid Fixture", "modes": ["20"],
                "spawns": [{"team": 0, "position": [0, 0, 0]}],
                "objectives": [
                    {"objective": 2, "name": null, "kind": "checkpoint", "placeable": 9, "position": [3, 0, 0]},
                    {"objective": 0, "name": "Point A", "kind": "point", "placeable": 7, "position": [1, 0, 0]},
                    {"objective": 1, "name": null, "kind": "checkpoint", "placeable": 8, "position": [2, 0, 0]},
                    {"kind": "payload", "placeable": 6, "position": [0, 0, 0], "rotation": [0, 0, 0, 1]},
                    {"stage": 1, "name": "B", "kind": "control point", "placeable": 5, "position": [4, 0, 0]}
                ]}}"#,
        )
        .unwrap();
        let map = catalog.game_map(0x80, 0x20, "", false, &[]).unwrap();
        let order = |kind| {
            map.objectives_of(kind)
                .map(|objective| (objective.order, objective.placeable))
                .collect::<Vec<_>>()
        };
        assert_eq!(order(ObjectiveKind::Checkpoint), [(1, 8), (2, 9)]);
        assert_eq!(order(ObjectiveKind::Payload), [(0, 6)]);
        assert_eq!(order(ObjectiveKind::ControlPoint), [(1, 5)]);
        let point = map.objectives_of(ObjectiveKind::Point).next().unwrap();
        assert_eq!(point.name.as_deref(), Some("Point A"));
        assert_eq!(point.position, [1.0, 0.0, 0.0]);
        assert!(
            MapCatalog::from_json(
                r#"{"0x80": {"objectives": [{"kind": "flag", "placeable": 1, "position": [0, 0, 0]}]}}"#
            )
            .is_err()
        );
    }

    /// A Practice Range entry and its mode as the derivations write them.
    const PRACTICE: &str = r#"{
        "0x0800000000000688": {"name": "Practice Range", "modes": ["0x0230000000000018", "0x023000000000005A"],
            "placeables": [1, 2, 3, 4, 5],
            "placeables_by_mode": {"0x023000000000005A": [136, 558, 564, 649]},
            "spawns": [
                {"team": 1, "position": [1, 2, 3], "yaw": 10, "rotation": [0, 1, 0, 0]},
                {"team": 0, "position": [54.56507, 0.99999833, 42.147877], "yaw": -170.343,
                    "rotation": [0.0, -0.99645156, 7.1054274e-15, 0.08417019]}
            ]}
    }"#;
    const PRACTICE_MODES: &str = r#"{"modes": {"0x023000000000005A": {"teams": [
        {"team": 0, "controller": null, "players": 6},
        {"team": 1, "controller": null, "players": 0}
    ], "scripted_loading": false}}}"#;

    #[test]
    fn the_practice_range_comes_from_the_catalog_and_its_mode() {
        let catalog = MapCatalog::from_json(PRACTICE)
            .unwrap()
            .with_modes(GameModes::from_json(PRACTICE_MODES).unwrap());
        // The install's values give Python's record, its spawn rounded.
        let practice = catalog.practice_range().unwrap();
        assert_eq!(practice, GameMap::practice_range());
        // A forced Practice Range plays its own mode, whatever the card's.
        assert_eq!(
            catalog.game_map(
                PRACTICE_RANGE_MAP,
                0x0230_0000_0000_0008,
                "Ignored",
                true,
                &[9]
            ),
            Some(practice)
        );
        assert_eq!(catalog.map_name(PRACTICE_RANGE_MAP), Some("Practice Range"));

        // Without the map, its mode's placeables or the mode, there is none.
        let error = |catalog: MapCatalog| catalog.practice_range().unwrap_err().kind();
        assert_eq!(
            error(MapCatalog::from_json("{}").unwrap()),
            io::ErrorKind::InvalidData
        );
        let other_mode = PRACTICE.replace(
            r#""placeables_by_mode": {"0x023000000000005A""#,
            r#""placeables_by_mode": {"0x0230000000000018""#,
        );
        assert_eq!(
            error(
                MapCatalog::from_json(&other_mode)
                    .unwrap()
                    .with_modes(GameModes::from_json(PRACTICE_MODES).unwrap())
            ),
            io::ErrorKind::InvalidData
        );
        let no_mode = MapCatalog::from_json(PRACTICE).unwrap();
        assert_eq!(error(no_mode), io::ErrorKind::InvalidData);
        let unturned = PRACTICE.replace(
            r#""rotation": [0.0, -0.99645156, 7.1054274e-15, 0.08417019]"#,
            r#""rotation": null"#,
        );
        assert_eq!(
            error(
                MapCatalog::from_json(&unturned)
                    .unwrap()
                    .with_modes(GameModes::from_json(PRACTICE_MODES).unwrap())
            ),
            io::ErrorKind::InvalidData
        );
        assert!(
            MapCatalog::from_json("{}")
                .unwrap()
                .game_map(PRACTICE_RANGE_MAP, PRACTICE_RANGE_MODE, "", false, &[])
                .is_none()
        );
    }

    #[test]
    fn loads_external_catalog_with_its_modes() {
        let temp = tempfile::tempdir().unwrap();
        // The catalog is needed.
        assert_eq!(
            MapCatalog::load(temp.path()).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        let path = temp.path().join(MAPS_FILE);
        fs::write(&path, ARENA).unwrap();
        // A catalog needs its modes.
        assert_eq!(
            MapCatalog::load(temp.path()).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::write(temp.path().join(MODES_FILE), MODES).unwrap();
        let catalog = MapCatalog::load(temp.path()).unwrap();
        assert_eq!(catalog.map_name(0x80), Some("Fixture Arena"));
        assert!(
            catalog
                .game_map(0x80, 0x0230_0000_0000_0025, "", false, &[])
                .is_none()
        );
        fs::write(&path, "{").unwrap();
        assert_eq!(
            MapCatalog::load(temp.path()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn reads_the_modes_teams_and_loading_rule() {
        let modes = GameModes::from_json(MODES).unwrap();
        let scripted = modes.get(0x0230_0000_0000_0025).unwrap();
        assert!(scripted.scripted_loading);
        assert_eq!(
            scripted.teams,
            [
                ModeTeam {
                    team: 0,
                    controller: Some(0x0580_0000_0000_0f44),
                    players: 4
                },
                ModeTeam {
                    team: 1,
                    controller: None,
                    players: 0
                }
            ]
        );
        assert!(modes.scripted_loading(0x0230_0000_0000_0025));
        assert!(!modes.scripted_loading(0x20));
        assert!(!modes.scripted_loading(0x21));
        assert_eq!(
            GameModes::from_json(r#"{"modes": {"x": {"teams": [], "scripted_loading": true}}}"#)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn dashboard_listing_sorts_stably_by_the_catalog_s_names() {
        let catalog = MapCatalog::from_json(
            r#"{
            "0x9": {"name": "Same"},
            "0008": {"name": "Same"},
            "0x7": {},
            "0x0800000000000688": {"name": "Override"},
            "6": {"name": ""}
        }"#,
        )
        .unwrap();
        let listing = catalog.map_catalog();
        assert_eq!(
            listing
                .iter()
                .map(|entry| (entry.guid, entry.name.as_str()))
                .collect::<Vec<_>>(),
            [
                (6, "0x6"),
                (7, "0x7"),
                (PRACTICE_RANGE_MAP, "Override"),
                (9, "Same"),
                (8, "Same")
            ]
        );
        assert_eq!(catalog.map_name(6), Some(""));
        assert_eq!(catalog.map_name(7), None);
        assert_eq!(catalog.map_name(50), None);
    }

    #[test]
    fn uses_mode_placeables_and_first_objective_in_team_order() {
        let catalog = MapCatalog::from_json(ARENA)
            .unwrap()
            .with_modes(GameModes::from_json(MODES).unwrap());
        let arena = catalog
            .game_map(0x80, 0x20, "Team Fixture", false, &[6, 5])
            .unwrap();
        assert_eq!(arena.mode_name, "Team Fixture");
        assert_eq!(arena.team_sizes, [6, 5]);
        assert!(arena.placeables.is_empty());
        assert_eq!(
            arena
                .spawns
                .iter()
                .map(|spawn| (spawn.team, spawn.position[0]))
                .collect::<Vec<_>>(),
            [
                (Some(0), 2.0),
                (Some(0), 4.0),
                (Some(1), 8.0),
                (Some(1), 3.0)
            ]
        );
        let mode_placeables = catalog.game_map(0x80, 0x21, "", false, &[]).unwrap();
        assert_eq!(mode_placeables.placeables, [3, 1, 3]);
        let fallback = catalog.game_map(0x80, 0x22, "", false, &[]).unwrap();
        assert_eq!(fallback.placeables, [8, 4, 8]);
        assert!(catalog.game_map(0x80, 0x23, "", false, &[]).is_none());
        assert!(
            catalog
                .game_map(0x80, 0x0230_0000_0000_0025, "", false, &[])
                .is_none()
        );
        assert!(catalog.game_map(0x81, 0x20, "", false, &[]).is_none());
    }

    #[test]
    fn free_for_all_keeps_tagged_or_teamless_points_in_source_order() {
        let catalog = MapCatalog::from_json(ARENA).unwrap();
        let arena = catalog
            .game_map(0x80, 0x20, "FFA Fixture", true, &[8])
            .unwrap();
        assert!(arena.free_for_all);
        assert_eq!(arena.team_sizes, [8]);
        assert_eq!(
            arena
                .spawns
                .iter()
                .map(|spawn| (spawn.team, spawn.position[0]))
                .collect::<Vec<_>>(),
            [(Some(0), 9.0), (None, 5.0), (None, 6.0), (Some(1), 7.0)]
        );
        assert_eq!(arena.spawn_point(3, 0), Some(([5.0, 1.0, 0.0], 0.0)));
        assert_eq!(arena.spawn_point(0, 0), Some(([9.0, 1.0, 0.0], 0.0)));
    }

    #[test]
    fn repeated_spawns_offset_along_local_right_and_left() {
        let catalog = MapCatalog::from_json(ARENA).unwrap();
        let arena = catalog.game_map(0x80, 0x20, "", false, &[]).unwrap();
        for (number, expected_z) in [(0, 0.0), (2, 1.5), (4, -1.5), (6, 3.0), (8, -3.0)] {
            let ([x, y, z], yaw) = arena.spawn_point(0, number).unwrap();
            assert!((x - 2.0).abs() < 1.0e-12);
            assert_eq!(y, 1.0);
            assert_eq!(z, expected_z);
            assert_eq!(yaw, 90.0);
        }
        assert_eq!(arena.spawn_point(0, 3), Some(([2.5, 1.0, 0.0], 0.0)));
        assert_eq!(arena.spawn_point(2, 3), Some(([3.0, 1.0, 0.0], 0.0)));
        assert_eq!(arena.spawn_point(2, 7), Some(([1.5, 1.0, 0.0], 0.0)));
        let empty = GameMap {
            spawns: Vec::new(),
            ..arena
        };
        assert_eq!(empty.spawn_point(0, 0), None);
    }

    #[test]
    fn spawn_rooms_follow_the_objective_side_or_the_control_stage() {
        let catalog = MapCatalog::from_json(ARENA).unwrap();
        let arena = catalog.game_map(0x80, 0x20, "", false, &[]).unwrap();
        let x = |team, number, choice| arena.spawn_point_in(team, number, choice).unwrap().0[0];
        let attack = |objective| SpawnChoice::Objective {
            objective,
            attack: true,
        };
        let defense = |objective| SpawnChoice::Objective {
            objective,
            attack: false,
        };
        // A side's rooms whatever the team's tag: team 0 attacks from 8.
        assert_eq!(x(0, 0, attack(1)), 8.0);
        assert_eq!(x(1, 0, defense(0)), 2.0);
        assert_eq!(x(1, 1, defense(0)), 4.0);
        assert_eq!(x(1, 0, defense(1)), 4.0);
        // No room for it: the first round's points.
        assert_eq!(x(1, 1, attack(5)), 3.0);
        assert_eq!(x(0, 0, SpawnChoice::First), 2.0);

        let control = MapCatalog::from_json(
            r#"{"0x80": {"name": "Stages", "modes": ["20"], "spawns": [
                {"team": 0, "position": [1, 0, 0], "stage": 0},
                {"team": 1, "position": [2, 0, 0], "stage": 0},
                {"team": 0, "position": [3, 0, 0], "stage": 1},
                {"team": 1, "position": [4, 0, 0], "stage": 1}
            ]}}"#,
        )
        .unwrap()
        .game_map(0x80, 0x20, "", false, &[])
        .unwrap();
        let stage = |team, stage| {
            control
                .spawn_point_in(team, 0, SpawnChoice::Stage(stage))
                .unwrap()
                .0[0]
        };
        assert_eq!([stage(0, 0), stage(1, 0)], [1.0, 2.0]);
        assert_eq!([stage(0, 1), stage(1, 1)], [3.0, 4.0]);
    }

    #[test]
    fn malformed_guids_are_invalid_data() {
        for input in [
            r#"{"not-hex":{}}"#,
            r#"{"0x1":{"modes":["not-hex"]}}"#,
            r#"{"0x1":{"placeables_by_mode":{"not-hex":[]}}}"#,
        ] {
            assert_eq!(
                MapCatalog::from_json(input).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    #[ignore = "requires the derived 104319 map catalog"]
    fn map_catalog_and_mode_selections_match_the_python_corpus() {
        use sha2::{Digest, Sha256};

        fn count(digest: &mut Sha256, value: usize) {
            digest.update(u32::try_from(value).unwrap().to_le_bytes());
        }

        fn text(digest: &mut Sha256, value: &str) {
            count(digest, value.len());
            digest.update(value.as_bytes());
        }

        let data = orpheus_testkit::runtime_dir().join("data");
        let catalog = MapCatalog::load(&data).unwrap();
        let listing = catalog.map_catalog();
        let mut digest = Sha256::new();
        count(&mut digest, listing.len());
        let mut cases = 0;
        let mut accepted = 0;
        for entry in &listing {
            digest.update(entry.guid.to_le_bytes());
            text(&mut digest, &entry.name);
            let name = catalog.map_name(entry.guid);
            digest.update([u8::from(name.is_some())]);
            if let Some(name) = name {
                text(&mut digest, name);
            }
            // Python's Range is mode 0018 with one player; ours is 005A,
            // with the same map record otherwise: hash Python's mode and
            // team sizes in their place.
            let modes = if entry.guid == PRACTICE_RANGE_MAP {
                &[PYTHON_PRACTICE_RANGE_MODE]
            } else {
                catalog.entries[&entry.guid].modes.as_slice()
            };
            count(&mut digest, modes.len());
            for &mode in modes {
                digest.update(mode.to_le_bytes());
                for ffa in [false, true] {
                    cases += 1;
                    let sizes: &[u8] = if ffa { &[8] } else { &[6, 6] };
                    let game = catalog.game_map(entry.guid, mode, "Fixture", ffa, sizes);
                    digest.update([u8::from(game.is_some())]);
                    let Some(game) = game else { continue };
                    accepted += 1;
                    digest.update(game.map_guid.to_le_bytes());
                    let (mode_guid, team_sizes) = if game.mode_guid == PRACTICE_RANGE_MODE {
                        (PYTHON_PRACTICE_RANGE_MODE, vec![1, 0])
                    } else {
                        (game.mode_guid, game.team_sizes.clone())
                    };
                    digest.update(mode_guid.to_le_bytes());
                    text(&mut digest, &game.name);
                    text(&mut digest, &game.mode_name);
                    digest.update([u8::from(game.free_for_all)]);
                    count(&mut digest, team_sizes.len());
                    digest.update(&team_sizes);
                    count(&mut digest, game.placeables.len());
                    for placeable in game.placeables {
                        digest.update(placeable.to_le_bytes());
                    }
                    count(&mut digest, game.spawns.len());
                    for spawn in game.spawns {
                        digest.update([spawn.team.unwrap_or(u8::MAX)]);
                        for value in spawn.position.into_iter().chain([spawn.yaw]) {
                            digest.update((value as f32).to_le_bytes());
                        }
                    }
                }
            }
        }
        assert_eq!((listing.len(), cases, accepted), (95, 698, 551));
        // Python game.content catalog and selection outputs on the derived
        // data, with geometry as LE f32. (On the prepared pack, whose five
        // lobby maps carried internal names, Python gives 414daebd....)
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "d6d5792b3a374a271c64b15fb6204e9a7c1f5e74428453e9617cd04f5000ccca"
        );
    }

    /// The install's scripted-loading modes are the ones Python listed
    /// (`content.py` `SCRIPTED_LOADING_MODES`: Mei's Snowball Offensive, two
    /// PvE modes, Retribution and Storm Rising).
    #[test]
    #[ignore = "requires the derived 104319 game modes"]
    fn the_scripted_loading_modes_are_the_install_s() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let modes = GameModes::load(data.join(MODES_FILE)).unwrap();
        let mut scripted: Vec<u64> = modes
            .iter()
            .filter(|(_, mode)| mode.scripted_loading)
            .map(|(guid, _)| guid)
            .collect();
        scripted.sort_unstable();
        assert_eq!(
            scripted,
            [
                0x0230_0000_0000_0008,
                0x0230_0000_0000_000f,
                0x0230_0000_0000_001a,
                0x0230_0000_0000_0025,
                0x0230_0000_0000_0043,
            ]
        );
    }

    /// The test fixture `GameMap::practice_range` is the install's record
    /// (`MapCatalog::practice_range`: map 0688's name and placeables in mode
    /// 005A, the mode's team sizes, the team 0 spawn point as Python rounded
    /// it), and that spawn is the install's within the rounding.
    #[test]
    #[ignore = "requires the derived 104319 map catalog"]
    fn the_practice_range_is_the_derived_record() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let derived = MapCatalog::load(&data).unwrap().practice_range().unwrap();
        assert_eq!(derived, GameMap::practice_range());
        assert_eq!(derived.team_sizes, [6, 0]);

        let maps: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(data.join(MAPS_FILE)).unwrap()).unwrap();
        let spawn = maps["0x0800000000000688"]["spawns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|spawn| spawn["team"] == 0)
            .unwrap();
        let number = |value: &serde_json::Value| value.as_f64().unwrap();
        let [chosen] = derived.spawns[..] else {
            panic!("one spawn point")
        };
        assert_eq!(chosen.team, None);
        for (axis, value) in spawn["position"].as_array().unwrap().iter().enumerate() {
            assert!((chosen.position[axis] - number(value)).abs() <= 0.000_005);
        }
        assert!((chosen.yaw - number(&spawn["yaw"])).abs() <= 0.000_5);
    }
}
