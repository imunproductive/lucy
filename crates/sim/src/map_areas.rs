//! Where a map's spawn rooms are and where its bodies die of the
//! environment.
//!
//! Spawn rooms (`game_map_areas_174.json`, the `orpheus-assets` derivation
//! `game_map_areas`): each room is a map entity running graph `0D62` with
//! the boxes of the area it names. The mode graph sends each room game
//! message `0724` with three flags; the Practice Range's `13C0` sets all
//! three. The room passes them in `0723` to its team's heroes inside, and
//! the heroes' mode body graph `0C8E` (server-only states) acts on them:
//! - `03D4` (v9014): status `STU_9028E035` type 0 and stack #8271e. Strong
//!   guess: type 0 is invulnerable (game-rules.md 21: "players there are
//!   invulnerable").
//! - `03D5` (v420): `ModifyHealth` of 400 a second with the room as
//!   instigator (game-rules.md 21: 400 HP/s).
//! - `03D3` (v8419, `m_allowHeroSelect`): game message `068A` to the
//!   owner. Hero select itself is gated in the client: H opens it only
//!   while the player entity's component 74 field 6 is set
//!   (`STU_E4C6AAB4`, "You must be in a spawn room to change heroes";
//!   ow1-preservation, `docs/notes/related-projects.md`), and that field is
//!   the server's.
//!
//! The rooms of the other team hurt intruders (`0B7B`, 0C8E st45: 400 a
//! second); not done here, as the Practice Range has one team.
//!
//! A room the placeable gives a team belongs to it; a room without one gets
//! its team from the mode (`0724` parameter `0473`). Assumption for those:
//! a room belongs to the teams whose current spawn point (the match flow's
//! choice) stands in it.
//!
//! Environmental death: the Practice Range has no damage area entity (the
//! `STUECDamageAreaInstanceData` kill volumes of maps like Ilios and Nepal),
//! and its collision has no other volume of its own. What it has under the
//! playable area are invisible floors of the world's player clip (triangles
//! with mask [`WORLD_CLIP`], which only the team proxies 0x08 and 0x10 and
//! category 0x02 meet): a pit floor at -11 m and the world's bottom at
//! -19.8 m, while the lowest floor a player walks on is about -5.25 m. A
//! body that falls out of the playable area lands on one of them. Strong
//! guess: such a catch floor is where retail's environment kills, so a
//! living body whose feet reach the highest catch floor dies
//! ([`kill_height`]). A catch floor is a level, walkable clip triangle of at
//! least [`CATCH_AREA`] below every spawn point; Watchpoint: Gibraltar's
//! clip has none (its level clip faces are small ledges), so the rule finds
//! nothing there.

use std::{collections::HashMap, fs, io, path::Path, sync::Arc};

use orpheus_content::maps::{GameMap, SpawnChoice};
use orpheus_physics::collision::index::SceneTriangle;
use serde::Deserialize;

use crate::objectives::CaptureBox;

/// The file in the runtime `data` folder.
pub const FILE: &str = "game_map_areas_174.json";
/// The mask of the world's player clip: it stops category 0x02 and the team
/// proxies 0x08 and 0x10, and nothing else (no shots, no camera).
pub const WORLD_CLIP: u16 = 0x1a;
/// The least area of a catch floor triangle, m²: the Practice Range's are
/// 161 m² and more, the clip ledges of other maps a few dozen at most.
pub const CATCH_AREA: f64 = 100.0;
/// How close above a catch floor the feet count as on it, m.
const CATCH_MARGIN: f64 = 0.1;
/// What the room's `ModifyHealth` heals a second (0C8E st13, `m_amount`).
pub const SPAWN_HEALING: f64 = 400.0;
/// A ground triangle whose surface has this bit is not ground
/// (`orpheus_physics::mover::ground::NOT_GROUND`).
const NOT_GROUND: u16 = 1;

/// One spawn room: the team the map gives it, if any, and its area.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnRoom {
    pub placeable: u32,
    pub team: Option<u8>,
    pub boxes: Vec<CaptureBox>,
}

impl SpawnRoom {
    pub fn contains(&self, position: [f64; 3]) -> bool {
        self.boxes.iter().any(|area| area.contains(position))
    }
}

/// Every map's spawn rooms, by map GUID.
#[derive(Debug, Default)]
pub struct MapAreaData {
    maps: HashMap<u64, Result<Arc<[SpawnRoom]>, String>>,
}

#[derive(Deserialize)]
struct RawFile {
    maps: HashMap<String, RawMap>,
}

#[derive(Deserialize)]
struct RawMap {
    #[serde(default)]
    spawn_rooms: Vec<RawRoom>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RawRoom {
    placeable: u32,
    team: Option<i8>,
    boxes: Vec<CaptureBox>,
}

impl MapAreaData {
    /// Read the file from the runtime `data` folder.
    pub fn load(data: &Path) -> io::Result<Self> {
        let path = data.join(FILE);
        let bytes = fs::read(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::read(&bytes)
    }

    pub fn read(bytes: &[u8]) -> io::Result<Self> {
        let file: RawFile = serde_json::from_slice(bytes).map_err(invalid)?;
        let mut maps = HashMap::new();
        for (key, raw) in file.maps {
            let guid = u64::from_str_radix(key.trim_start_matches("0x"), 16)
                .map_err(|_| invalid(format!("{FILE}: map key {key}")))?;
            let entry = match raw.error {
                Some(error) => Err(error),
                None => Ok(raw
                    .spawn_rooms
                    .into_iter()
                    .map(|room| {
                        for area in &room.boxes {
                            area.check().map_err(|error| {
                                invalid(format!(
                                    "{FILE}: map {key}: room {}: {error}",
                                    room.placeable
                                ))
                            })?;
                        }
                        Ok(SpawnRoom {
                            placeable: room.placeable,
                            team: room.team.and_then(|team| u8::try_from(team).ok()),
                            boxes: room.boxes,
                        })
                    })
                    .collect::<io::Result<Arc<[SpawnRoom]>>>()?),
            };
            maps.insert(guid, entry);
        }
        Ok(Self { maps })
    }

    /// The map's spawn rooms; none when the data has none or its
    /// derivation failed (the reason goes to the log).
    pub fn spawn_rooms(&self, map: &GameMap) -> Arc<[SpawnRoom]> {
        match self.maps.get(&map.map_guid) {
            Some(Ok(rooms)) => Arc::clone(rooms),
            Some(Err(error)) => {
                eprintln!("{}: no spawn rooms ({error})", map.name);
                Arc::default()
            }
            None => Arc::default(),
        }
    }
}

/// Whether `position` is in one of `team`'s spawn rooms now: a room the map
/// gives the team, or one without a team that holds the team's current
/// spawn point (see the module notes). `None` when the team has no room at
/// all, so nothing is gated by one.
pub fn in_spawn_room(
    rooms: &[SpawnRoom],
    map: &GameMap,
    team: u8,
    choice: SpawnChoice,
    position: [f64; 3],
) -> Option<bool> {
    let spawn = map.spawn_point_in(team, 0, choice).map(|(at, _)| at);
    let mut theirs = rooms.iter().filter(|room| match room.team {
        Some(owner) => owner == team,
        None => spawn.is_some_and(|spawn| room.contains(spawn)),
    });
    let first = theirs.next()?;
    Some(first.contains(position) || theirs.any(|room| room.contains(position)))
}

/// The height of the map's highest catch floor below `below` (its lowest
/// spawn point), if it has one (see the module notes).
pub fn kill_height<'a>(
    triangles: impl IntoIterator<Item = &'a SceneTriangle>,
    below: f64,
) -> Option<f64> {
    triangles
        .into_iter()
        .filter(|triangle| triangle.mask == WORLD_CLIP && triangle.surface & NOT_GROUND == 0)
        .filter_map(|triangle| {
            let [a, b, c] = triangle.vertices;
            let u: [f64; 3] = std::array::from_fn(|axis| b[axis] - a[axis]);
            let v: [f64; 3] = std::array::from_fn(|axis| c[axis] - a[axis]);
            let normal = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let twice = normal.iter().map(|part| part * part).sum::<f64>().sqrt();
            let level = a[1] == b[1] && b[1] == c[1];
            (level && twice / 2.0 >= CATCH_AREA && a[1] < below).then_some(a[1])
        })
        .max_by(f64::total_cmp)
}

/// Whether feet at `y` are on or below the catch floor at `height`.
pub fn caught(height: f64, y: f64) -> bool {
    y < height + CATCH_MARGIN
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
