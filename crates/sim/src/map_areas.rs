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
//! The bodies run `0C8E` (`crate::body_script::ModeBody`), but these
//! states of it stay idle: they wait for `0723`, which only the rooms'
//! entities (`0D62`) send, and the server runs none. The server keeps the
//! rules above in their place: the graph's own forms don't work on the
//! server (st11's status `STU_9028E035` is native and unknown, st13's
//! `ModifyHealth` is an inert state in the VM, and `068A`'s handler is in
//! the client), and sending `0723` as well would heal twice once the VM
//! runs `ModifyHealth`. The one networked part, st12's stack #8271e, is
//! not sent.
//!
//! A room the placeable gives a team belongs to it; a room without one gets
//! its team from the mode (`0724` parameter `0473`). Assumption for those:
//! a room belongs to the teams whose current spawn point (the match flow's
//! choice) stands in it.
//!
//! Environmental death, in this order of evidence. Every rule tests the
//! centre of the body's collision capsule (its middle between the lowest and
//! highest points, from the body's mover data and stance: 1.25 m above the
//! feet of a standing 2.0 m hero with a 0.5 m pogo), not its feet: the user
//! saw retail test the body's centre.
//! - Damage areas (`game_map_areas` `damage_areas`: entities with
//!   `STUECDamageAreaInstanceData`, definition `0109`) on the maps that have
//!   them (Ilios, Nepal, Lijiang Tower, Oasis, Eichenwalde, Volskaya, Busan,
//!   Horizon, Junkertown, Blizzard World, Rialto, Castillo, Black Forest,
//!   Necropolis, Petra, ...: see `docs/notes/behavior.md`). Their damage
//!   (`m_B81F7395`) is 100 on most, 9000 on Busan, 9999 on Petra, 0 on a few.
//!   Retail kills any hero at once in the Ilios well or a Lijiang pit, and
//!   those areas have 100, so 100 can't be 100 health a second (a 600 HP
//!   hero would live 6 s): strong guess, the damage is a share of the body's
//!   health in percent, so 100 and more kill at once ([`LETHAL_DAMAGE`]), as
//!   the 9000 and 9999 do whatever it means. A living body whose centre is in
//!   a lethal area of the mode dies. The areas of damage 0 are left out
//!   (assumption: off unless a graph turns them on; no graph does here).
//! - Kill volumes (`game_map_areas` `kill_volumes`): the boxes of the map's
//!   collision that only query category 0x02 meets (`orpheus_assets`'
//!   `map_areas` notes), nearly all 4 m thick slabs under a map's drops; 73
//!   of the 95 map records with a header have them, Watchpoint: Gibraltar,
//!   Dorado and Route 66 among the maps without damage areas. Strong guess:
//!   a body that falls off the map dies where it enters one. The slabs are
//!   the only shapes of their own under those drops, a few metres below the
//!   lowest floor (`docs/notes/behavior.md` has the table), and retail
//!   kills there well before the map's lowest triangle (seen in the game).
//!   Not read in the client: which query category 0x02 is. A living body
//!   whose centre is in one dies.
//! - The catch floor, on a map with neither damage areas nor kill volumes.
//!   The Practice Range has none, and its collision has no other volume of
//!   its own. What it has under the playable area are invisible floors of
//!   the world's player clip (triangles with mask [`WORLD_CLIP`], which only
//!   the team proxies 0x08 and 0x10 and category 0x02 meet): a pit floor at
//!   -11 m and the world's bottom at -19.8 m, while the lowest floor a
//!   player walks on is about -5.25 m. Strong guess: such a catch floor is
//!   where retail's environment kills, so a living body that lands on the
//!   highest catch floor dies ([`kill_height`]). The movers stand on the
//!   clip, so the centre never goes below the floor: the body dies when its
//!   centre is no higher above the floor than its centre is above its feet
//!   ([`caught`]). A catch floor is a level, walkable clip triangle of at
//!   least [`CATCH_AREA`] below every spawn point.
//!
//! Not a rule of the game: a safety net. A living body whose centre is
//! [`FALL_LIMIT`] below the map's lowest collision triangle dies, and the
//! server logs a warning with the map and the position, as the map's data
//! then lacks the volume that kills there. The mover stops a falling body
//! 100 m below that triangle (`ABYSS`, `Mover._place` in upstream
//! leafall903/ORPHEUS `mover.py`), so without the net it would stand alive
//! on nothing. The maps the rules above miss (Hollywood, Temple of Anubis,
//! Ecopoint: Antarctica, the menu maps) are closed or never played.
//!
//! Spawn door barriers (`game_map_areas` `door_barriers`): the server-owned
//! placeables running graph `00F6` in the spawn room doorways. The client
//! makes a server-owned placeable only when the server creates it, so the
//! scene creates them from the map after the player and the mode (see
//! `orpheus_assets`' `map_areas` notes).

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::Path,
    sync::Arc,
};

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
/// How far below the map's lowest collision triangle the safety net kills
/// a body's centre, m (module notes). Not from the game.
pub const FALL_LIMIT: f64 = 20.0;
/// The least damage of a damage area that kills: 100 % of the body's
/// health (see the module notes).
pub const LETHAL_DAMAGE: f64 = 100.0;
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

/// A damage area of a map (`STUECDamageAreaInstanceData`): its damage and
/// boxes.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageArea {
    pub placeable: u32,
    /// Its first identifier: it exists only in the modes that list it.
    pub identifier: Option<u64>,
    /// `m_B81F7395`: a share of the body's maximum health in percent (see
    /// the module notes).
    pub damage: f64,
    pub boxes: Vec<CaptureBox>,
}

impl DamageArea {
    /// Whether it kills what enters it (see the module notes).
    pub fn lethal(&self) -> bool {
        self.damage >= LETHAL_DAMAGE
    }

    pub fn contains(&self, position: [f64; 3]) -> bool {
        self.boxes.iter().any(|area| area.contains(position))
    }
}

/// A server-owned placeable of a map: its flat index and first identifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapPlaceable {
    pub placeable: u32,
    pub identifier: Option<u64>,
}

impl MapPlaceable {
    /// Whether the placeable exists in a mode that lists `identifiers`: one
    /// without an identifier exists in every mode.
    pub fn in_mode(&self, identifiers: &BTreeSet<u64>) -> bool {
        self.identifier
            .is_none_or(|identifier| identifiers.contains(&identifier))
    }
}

/// One map's areas.
#[derive(Debug)]
struct MapAreas {
    rooms: Result<Arc<[SpawnRoom]>, String>,
    damage_areas: Vec<DamageArea>,
    door_barriers: Vec<MapPlaceable>,
    kill_volumes: Arc<[CaptureBox]>,
}

/// Every map's spawn rooms, damage areas, door barriers and kill volumes, by
/// map GUID.
#[derive(Debug, Default)]
pub struct MapAreaData {
    maps: HashMap<u64, MapAreas>,
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
    #[serde(default)]
    damage_areas: Vec<RawDamageArea>,
    #[serde(default)]
    door_barriers: Vec<RawPlaceable>,
    #[serde(default)]
    kill_volumes: Vec<CaptureBox>,
}

#[derive(Deserialize)]
struct RawRoom {
    placeable: u32,
    team: Option<i8>,
    boxes: Vec<CaptureBox>,
}

#[derive(Deserialize)]
struct RawDamageArea {
    #[serde(flatten)]
    placeable: RawPlaceable,
    damage: f64,
    boxes: Vec<CaptureBox>,
}

#[derive(Deserialize)]
struct RawPlaceable {
    placeable: u32,
    identifier: Option<String>,
}

impl RawPlaceable {
    fn read(self, key: &str) -> io::Result<MapPlaceable> {
        let identifier = self
            .identifier
            .map(|text| {
                u64::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| {
                    invalid(format!(
                        "{FILE}: map {key}: placeable {}: identifier {text}",
                        self.placeable
                    ))
                })
            })
            .transpose()?;
        Ok(MapPlaceable {
            placeable: self.placeable,
            identifier,
        })
    }
}

fn check_boxes(boxes: &[CaptureBox], key: &str, what: &str, placeable: u32) -> io::Result<()> {
    for area in boxes {
        area.check()
            .map_err(|error| invalid(format!("{FILE}: map {key}: {what} {placeable}: {error}")))?;
    }
    Ok(())
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
            let rooms = match raw.error {
                Some(error) => Err(error),
                None => Ok(raw
                    .spawn_rooms
                    .into_iter()
                    .map(|room| {
                        check_boxes(&room.boxes, &key, "room", room.placeable)?;
                        Ok(SpawnRoom {
                            placeable: room.placeable,
                            team: room.team.and_then(|team| u8::try_from(team).ok()),
                            boxes: room.boxes,
                        })
                    })
                    .collect::<io::Result<Arc<[SpawnRoom]>>>()?),
            };
            let damage_areas = raw
                .damage_areas
                .into_iter()
                .map(|area| {
                    let placeable = area.placeable.read(&key)?;
                    check_boxes(&area.boxes, &key, "damage area", placeable.placeable)?;
                    Ok(DamageArea {
                        placeable: placeable.placeable,
                        identifier: placeable.identifier,
                        damage: area.damage,
                        boxes: area.boxes,
                    })
                })
                .collect::<io::Result<_>>()?;
            let door_barriers = raw
                .door_barriers
                .into_iter()
                .map(|barrier| barrier.read(&key))
                .collect::<io::Result<_>>()?;
            for (number, volume) in (0u32..).zip(&raw.kill_volumes) {
                check_boxes(std::slice::from_ref(volume), &key, "kill volume", number)?;
            }
            maps.insert(
                guid,
                MapAreas {
                    rooms,
                    damage_areas,
                    door_barriers,
                    kill_volumes: raw.kill_volumes.into(),
                },
            );
        }
        Ok(Self { maps })
    }

    /// The map's spawn rooms; none when the data has none or its
    /// derivation failed (the reason goes to the log).
    pub fn spawn_rooms(&self, map: &GameMap) -> Arc<[SpawnRoom]> {
        match self.maps.get(&map.map_guid).map(|areas| &areas.rooms) {
            Some(Ok(rooms)) => Arc::clone(rooms),
            Some(Err(error)) => {
                eprintln!("{}: no spawn rooms ({error})", map.name);
                Arc::default()
            }
            None => Arc::default(),
        }
    }

    /// The map's lethal damage areas that exist in a mode that lists
    /// `identifiers`.
    pub fn lethal_areas(&self, map: &GameMap, identifiers: &BTreeSet<u64>) -> Arc<[DamageArea]> {
        self.maps
            .get(&map.map_guid)
            .map(|areas| {
                areas
                    .damage_areas
                    .iter()
                    .filter(|area| {
                        area.lethal()
                            && MapPlaceable {
                                placeable: area.placeable,
                                identifier: area.identifier,
                            }
                            .in_mode(identifiers)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The flat indexes of the map's spawn door barriers that exist in a
    /// mode that lists `identifiers`.
    pub fn door_barriers(&self, map: &GameMap, identifiers: &BTreeSet<u64>) -> Vec<u32> {
        self.maps
            .get(&map.map_guid)
            .map(|areas| {
                areas
                    .door_barriers
                    .iter()
                    .filter(|barrier| barrier.in_mode(identifiers))
                    .map(|barrier| barrier.placeable)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The map's kill volumes (see the module notes).
    pub fn kill_volumes(&self, map: &GameMap) -> Arc<[CaptureBox]> {
        self.maps
            .get(&map.map_guid)
            .map(|areas| Arc::clone(&areas.kill_volumes))
            .unwrap_or_default()
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

/// Whether a body whose centre is at `y`, `above_feet` over its feet, is
/// on or below the catch floor at `height`.
pub fn caught(height: f64, y: f64, above_feet: f64) -> bool {
    y < height + above_feet + CATCH_MARGIN
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
