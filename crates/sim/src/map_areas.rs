//! Where a map's spawn rooms are and where its bodies die of the
//! environment.
//!
//! Spawn rooms (`game_map_areas_174.json`, the `lucy-assets` derivation
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
//! second).
//!
//! The server runs the rooms' `0D62` with the other server placeables
//! (`crate::placed_script`) and hands their `0723` and `0B7B` to the
//! bodies, which run `0C8E` (`crate::body_script::ModeBody`): its status
//! makes the body invulnerable (`vm/statuses.rs`), its `ModifyHealth` heals
//! with the room as instigator (`crate::combat::Combat::modify_health`).
//! The mode graphs that send the rooms `0724` don't run on the server;
//! [`room_holds`] sends it in their place (docs/dev/loose-ends.md). The
//! hero select flag (component 74 field 6) stays the server's own rule:
//! the body's feet in one of its team's rooms ([`in_spawn_room`]).
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
//!   collision that only query category 0x02 meets (`lucy_assets`'
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
//! Server placeables (`game_map_areas` `server_placeables`): the map's
//! server-owned placeables that are not on the loading list (spawn door
//! barriers, doorway frames, team walls, spawn doors, objective models and
//! other scenery). The client makes a server-owned placeable only when the
//! server creates it, so the scene creates them from the map after the
//! player and the mode. Their scripts (each one's definition, position and
//! statescript instance data, [`PlacedScript`]) run on the server in
//! `crate::placed_script`.
//!
//! Placeable bodies ([`PlaceableBody`]): the client builds the static
//! physics shapes of every server placeable's model when the server
//! creates it, as it builds a client-made prop's, so they stop his mover
//! (and his shots) though the movement cache, which holds the client-made
//! props only, leaves them out. The server puts them in the movers' world
//! as switched groups (`lucy_physics` `ContactWorld::with_switched`),
//! on while the placeable stands (until a script or the mode removes it).
//! Where the client puts each shape (game checks 2026-11-22, the client's
//! physics world on the Practice Range and Hanamura, refs `MapEntities.md`
//! "MapCollision") depends on what poses the model ([`ModelPose`]):
//! - nothing: every shape at the placeable's place times its scale (the
//!   Range's Training Partner console `3963`, one 0x80 mesh, its box within
//!   0.01 m of the client's; Hanamura's `0428`);
//! - an animation component (component 39, `STU_496257B5`; the spawn doors
//!   of graph `0D32`, the Range's `1793` and `230A`, Hanamura's `292C`,
//!   `292B`, `25D7`): component 39 brings component 28, whose movement
//!   state gives the entity's transform; the server's create carries none,
//!   so the whole model stays at the world origin, unscaled, its static
//!   frame mesh too (a create with the map's place as that state stands
//!   the doors in their doorways, but closed: they shut the Practice
//!   Range's spawn in; refs `MapEntities.md` "MapAnim", loose end);
//! - a movement state (component 28 from `STU_E272A026`, or from
//!   `STU_6235CCE1`, component 34, without component 39: the lifts of
//!   graph `2ABA`, Volskaya's `3CD8`): without one the client keeps the
//!   whole model at the world origin, so the server's create carries the
//!   map's place ([`StatePlace`]: the position on the 1/1024 m grid, the
//!   turn as a 16-bit yaw), and the shapes stand there, unscaled;
//! - an idle animation (`STUMirroredIdleAnimComponent`): the static parts
//!   (category 1 or 0x80) at the placeable's place (Hanamura's `13C7`), the
//!   animated ones (category 0x1000) where the animation puts their bones:
//!   Hanamura's cars `03B6` and `03B7` stand 0.35 m off their bind pose
//!   and bob by centimetres, its truck `03B4` holds another pose. The
//!   server poses them as the client does each tick
//!   ([`crate::idle_animation`]; moving shapes, `lucy_physics`
//!   `ContactWorld::place`);
//! - a path (component 98, `STU_66A032BC`: the Range's platforms `0516`,
//!   Volskaya's and Route 66's flying platforms, Rialto's boats): the whole
//!   model, every shape, where the path puts the placeable at the path
//!   clock ([`crate::path_follower`]), its animated parts then on their
//!   bones as above.
//!
//! The movers' world poses the moving shapes at each command's view time
//! (`PreparedScene::pose_moving_for`; [`MovingBodies`]). A placeable whose
//! definition has component 77 and any of components 28, 39, 27 or 98 is a
//! platform: a mover standing on one of its shapes rides it, in the frame
//! of the shape's node (`lucy_physics` `Platform`, mover `platform`);
//! [`MovingBodies`] gives the movers the platform's frame a tick earlier
//! for its velocity.
//!
//! The shots meet a standing body's shapes of category 1 or 0x80, the
//! categories the client's world ray asks for, where they stand now
//! (`crate::hitscan::PlaceableRays`); a team wall's, which the client takes
//! for one team's viewers only, they don't.
//!
//! Team walls (placeable bodies running `0FA8`): the server placeables
//! running graph `0FA8` (definitions `1C90`, `1C91`) stand in pairs in the
//! PvP spawn doorways, and their models' meshes stop one team's mover each
//! (masks 0x88, 0x90; `lucy_assets` `map_areas`). `0FA8` turns a wall on
//! for the team in its v1777 (the same team as the mask, in every wall's
//! data) with state 3 (remote 1), which begins on game message `0845`
//! when its entry condition `#10768 == (attacking team != #1777)` holds;
//! before the message, and when the condition fails, state 2 (remote 0)
//! runs, which sets `STU_33A46033`'s `m_7E42F868` for everyone, as st3
//! does for the other team. So `m_7E42F868` set means off: two walls of
//! one doorway, one per team, can't both stop their team for the whole
//! match, or no one would leave that spawn room. The server's mover takes
//! a wall while its placed script runs state 3 and is complete, so that
//! the clients get the same state in its frames ([`team_wall_on`];
//! `lucy_physics` `ContactWorld::switch`); the wall's mask leaves the
//! other team out. Assumption: `m_7E42F868`'s flags (component 1, scene
//! node `0x8000` and `0x200000000000`, refs `OwnerPrediction.md` §12.4)
//! take the model's physics out with its drawing; not read in the client.
//! The mode graphs send `0845` (`0F4A` st60, `0D76` st19); the Rust match
//! flow that stands in for them (Escort, Hybrid and the other modes)
//! doesn't, so there every wall stays off, on the server as on the
//! clients. Under `0F4A` (Assault) the walls take it, and st5 reads the
//! attacking team from the mode graph's #10795e
//! (`MapEntityWorld::set_mode_variables`; before that the placed scripts
//! read the mode entity's ruleset values only, the attacking team null,
//! and Anubis's defenders' doorway walls stood all match). Measured on
//! Anubis (Practice vs AI): no wall on in assemble, setup or the round.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs, io,
    path::Path,
    sync::Arc,
};

use lucy_content::maps::{GameMap, SpawnChoice};
use lucy_physics::{
    collision::{
        cache::{HalfEdge, Hull, Mesh, Placement as CachePlacement, Shape, Triangle},
        index::SceneTriangle,
        scene::{CollisionScene, SceneError, probe_triangles},
    },
    contact::{
        math::ContactFrame,
        mesh::ContactMesh,
        world::{ContactWorldError, Platform, StaticGeometry, StaticShape, compose, scaled_solid},
    },
    mover::platform::PlatformClock,
};
use lucy_script::{
    graph::{GraphBinding, GraphStore},
    map_tags::{MapTags, TaggedEntity},
    placed::{EntityVolumes, PlacedEntity, Placement},
    track::{Track, TrackPoint},
    wire::{Value as ScriptValue, Variables},
};
use serde::Deserialize;

use crate::{
    bot_animation::{multiply, rotate},
    idle_animation::{IdleMotion, RawIdleRig},
    objective_data::MapObjectives,
    objectives::{Area, CaptureBox},
    path_follower::{PathFollower, RawPath},
};

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
/// A ground triangle whose surface has this bit is not ground
/// (`lucy_physics::mover::ground::NOT_GROUND`).
const NOT_GROUND: u16 = 1;
/// The team walls' graph and its state that turns a wall on (see the module
/// notes).
const TEAM_WALL_GRAPH: u32 = 0x0FA8;
const TEAM_WALL_ON: u16 = 3;

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

    /// The room's entity, as the scene creates it
    /// ([`PlacedScript::entity`]).
    pub fn entity(&self) -> u32 {
        0x8000_0000 | self.placeable
    }
}

/// The areas of the map's entities that have one (`STU_73E1C5F0`,
/// `STU_96F571ED`): the spawn rooms', as their `0D62` asks for its own, and
/// the capture points', as the capture area aura's `0F00` asks for its
/// point (`lucy_script` `vm/residency.rs`): each point's boxes where
/// its objective stands ([`Area::of_point`]).
pub struct MapVolumes {
    rooms: Arc<[SpawnRoom]>,
    points: Vec<(u32, Area)>,
}

impl MapVolumes {
    pub fn new(rooms: Arc<[SpawnRoom]>, map: &GameMap, objectives: &MapObjectives) -> Self {
        let points = objectives
            .points
            .iter()
            .map(|point| {
                let at = map
                    .objectives
                    .iter()
                    .find(|objective| objective.placeable == point.placeable)
                    .map_or(point.position, |objective| objective.position);
                (0x8000_0000 | point.placeable, Area::of_point(point, at))
            })
            .collect();
        Self { rooms, points }
    }
}

impl EntityVolumes for MapVolumes {
    fn contains(&self, entity: u32, point: [f64; 3]) -> Option<bool> {
        if let Some(room) = self.rooms.iter().find(|room| room.entity() == entity) {
            return Some(room.contains(point));
        }
        self.points
            .iter()
            .find(|(placed, _)| *placed == entity)
            .map(|(_, area)| area.contains(point))
    }
}

/// The mode graph's game message to a spawn room (`0724.025`) and its
/// parameters: `03D4` invulnerable, `03D5` heal, `03D3` hero select, `0473`
/// the room's team.
pub const ROOM_MESSAGE: u64 = 0x0240_0000_0000_0724;
const ROOM_INVULNERABLE: u32 = 0x03d4;
const ROOM_HEAL: u32 = 0x03d5;
const ROOM_HERO_SELECT: u32 = 0x03d3;
const ROOM_TEAM: u32 = 0x0473;

/// The mode graph's `0724` for a room it holds (see [`room_holds`]): all
/// three flags on, as every mode graph sends them (`13C0` st9, `0F4A` st23
/// and st90, `0EC2` st97, `0D76`, `0D0B`, `0F60`, `0F73`), and the team it
/// gives the room, if any.
pub fn room_message(team: Option<u8>) -> Variables {
    let mut parameters = Variables::from([
        (ROOM_INVULNERABLE, ScriptValue::Bool(true)),
        (ROOM_HEAL, ScriptValue::Bool(true)),
        (ROOM_HERO_SELECT, ScriptValue::Bool(true)),
    ]);
    if let Some(team) = team {
        parameters.insert(ROOM_TEAM, ScriptValue::Integer(i64::from(team)));
    }
    parameters
}

/// The rooms the mode graph holds its `0724` for now, with the team it
/// gives each (`0473`). Stand-in for the mode graphs, which the server
/// doesn't run (docs/dev/loose-ends.md): a room the map gives a team gets
/// the message with no team (its own `0D62` override names it, as the
/// Practice Range's `13C0` sends it to every entity); a room without one
/// gets the team whose current spawn point (the match flow's `choice`)
/// stands in it, or no team when both teams' do or in a free-for-all; a
/// room that holds no current spawn point gets none (the same rule as
/// [`in_spawn_room`]).
pub fn room_holds(
    rooms: &[SpawnRoom],
    map: &GameMap,
    choice: impl Fn(u8) -> SpawnChoice,
) -> Vec<(u32, Option<u8>)> {
    rooms
        .iter()
        .filter_map(|room| {
            if room.team.is_some() {
                return Some((room.entity(), None));
            }
            let teams: Vec<u8> = (0..2)
                .filter(|&team| {
                    map.spawn_point_in(team, 0, choice(team))
                        .is_some_and(|(at, _)| room.contains(at))
                })
                .collect();
            match teams[..] {
                [] => None,
                [team] if !map.free_for_all => Some((room.entity(), Some(team))),
                _ => Some((room.entity(), None)),
            }
        })
        .collect()
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

/// A server placeable's script data: its definition, where the map puts it
/// and its statescript instance data (`lucy_script::placed`).
#[derive(Debug)]
pub struct PlacedScript {
    pub placeable: u32,
    /// The entity definition (`003`).
    pub definition: u64,
    pub position: [f64; 3],
    pub overrides: Vec<GraphBinding>,
    pub extra: Vec<GraphBinding>,
    /// Its definition moves it by an idle animation the client plays (a
    /// `STUMirroredIdleAnimComponent`): the Practice Range's platforms.
    pub moving: bool,
    /// Its second identifier, by which a mode graph removes it
    /// (`STU_A3DC6A25`: Hanamura's health packs carry `2AB0.01C`).
    pub second_identifier: Option<u64>,
    /// Its definition gives it component 28 without component 39 (the
    /// lifts of graph `2ABA`): its create carries this place
    /// ([`StatePlace`]).
    pub state_place: Option<StatePlace>,
    /// The parametric track read by the placeable's own graph.
    pub track: Option<Arc<Track>>,
}

impl PlacedScript {
    /// The entity the client makes of it: `0x80000000 | flat index`.
    pub fn entity(&self) -> u32 {
        0x8000_0000 | self.placeable
    }

    pub fn placement(&self) -> Placement<'_> {
        Placement {
            entity: self.entity(),
            definition: self.definition,
            position: self.position,
            overrides: &self.overrides,
            extra: &self.extra,
        }
    }
}

/// Where a placeable whose transform comes from a movement state stands
/// (component 28 from `STU_E272A026` or `STU_6235CCE1`, without component
/// 39; see the module notes): the map's place as its create's movement
/// state carries it, the position on the 1/1024 m grid and its turn about
/// y as a 16-bit yaw word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatePlace {
    /// Metres, on the grid.
    pub position: [f64; 3],
    /// 0 along +z, 16384 along +x (refs `Payloads.md` "ModeGraph5 payload
    /// position" §2).
    pub yaw: i32,
}

impl StatePlace {
    /// The place of a placeable the map puts at `position`, turned by
    /// `rotation` (x, y, z, w); none for a turn that isn't about y alone
    /// (the ten such placeables of 1.74 are all turned about y).
    pub fn new(position: [f64; 3], rotation: [f32; 4]) -> Option<Self> {
        let [x, y, z, w] = rotation.map(f64::from);
        if x.abs() > 1e-6 || z.abs() > 1e-6 {
            return None;
        }
        // Forward (the turn's z column): (2wy, 0, 1 - 2y²).
        let yaw = (2.0 * w * y).atan2(1.0 - 2.0 * y * y);
        let steps = (yaw * 32768.0 / std::f64::consts::PI).round() as i64;
        Some(Self {
            position: position.map(|axis| (axis * 1024.0).round_ties_even() / 1024.0),
            yaw: i32::from(steps as u16 as i16),
        })
    }

    /// Its frame as the client builds it from the state: the yaw word
    /// times 2π/65536 in float (`0x7FF7895AE980`), a turn about y.
    pub fn frame(&self) -> ContactFrame {
        let half = f64::from(f32::from(self.yaw as u16) * 9.587_38e-5_f32) * 0.5;
        ContactFrame {
            translation: self.position.map(|axis| axis as f32),
            rotation: [0.0, half.sin() as f32, 0.0, half.cos() as f32],
        }
    }
}

/// A server placeable whose definition's model has physics shapes that stop
/// the mover (see the module notes): its placeable, where the map puts it,
/// its model's shapes and what keeps it in the movers' world.
#[derive(Debug)]
pub struct PlaceableBody {
    pub placeable: MapPlaceable,
    frame: ContactFrame,
    scale: [f32; 3],
    model: Arc<[ModelShape]>,
    /// A team wall (graph `0FA8`): its script turns it on
    /// ([`team_wall_on`]); any other body stands while its placeable does.
    pub team_wall: bool,
    /// What poses its model on the clients.
    pose: ModelPose,
    /// The path it follows (component 98, [`PathFollower`]): the whole
    /// model goes where the path puts it, its animated parts then on their
    /// bones.
    path: Option<Arc<PathFollower>>,
    track: bool,
    /// The share of its velocity a mover standing on it takes, when it is
    /// a platform the mover rides (component 77 and any of components 28,
    /// 39, 27 or 98; `lucy_physics` `Platform`).
    platform: Option<f32>,
}

/// What poses a placeable's model on the clients, and so its physics
/// (see the module notes).
#[derive(Clone, Debug)]
enum ModelPose {
    /// Nothing: the model stands where the map puts it.
    Placed,
    /// Its animation component (component 39, `STU_496257B5`), whose
    /// movement state the server's create doesn't carry: the model stays
    /// at the world origin, unscaled.
    Origin,
    /// Its idle animation (`STUMirroredIdleAnimComponent`,
    /// [`PlacedScript::moving`]): its animated parts follow the animation
    /// ([`MovingBody`]; none without the definition's idle animation), the
    /// rest stands where the map puts it.
    Idle(Option<IdleMotion>),
}

/// One physics shape of a placeable's model, in model space, with the
/// category the client files it under, the bone of the idle rig it hangs
/// on (when the model has one), and its model node with that node's place
/// in the model (0 and the identity without one).
#[derive(Debug)]
struct ModelShape {
    category: u16,
    bone: Option<usize>,
    node: u32,
    node_place: ContactFrame,
    geometry: ModelGeometry,
}

/// A model shape's geometry; a mesh shared by every placement of the model.
#[derive(Debug)]
enum ModelGeometry {
    Mesh(u16, Arc<Mesh>),
    Solid(Shape),
}

/// The category of a model's animated parts: the client places them where
/// the model's animation puts their bones (see the module notes).
const ANIMATED: u16 = 0x1000;
/// The categories the client's world ray meets: a shot's body mask & 0x81
/// (refs `WeaponRays.md`).
const RAY_CATEGORIES: u16 = 0x81;

/// Where the client puts a shape of a placeable's model.
enum ShapePlace {
    /// At this frame times this scale.
    Fixed(ContactFrame, [f32; 3]),
    /// Where the placeable stands now (the map's place, or its path's)
    /// times its scale, moved by its bone's move when it hangs on a bone of
    /// the idle rig ([`MovingBody`]).
    Moving(Option<usize>),
}

impl ModelPose {
    /// Where the client puts `shape` of a model that the map puts at
    /// `frame` times `scale`, on a path or not; `None` for an animated
    /// part of an idle animation the server can't play.
    fn place(
        &self,
        shape: &ModelShape,
        frame: ContactFrame,
        scale: [f32; 3],
        path: bool,
    ) -> Option<ShapePlace> {
        match self {
            Self::Origin => Some(ShapePlace::Fixed(ContactFrame::IDENTITY, [1.0; 3])),
            Self::Idle(motion) if shape.category == ANIMATED => motion
                .as_ref()
                .and(shape.bone)
                .map(|bone| ShapePlace::Moving(Some(bone))),
            _ if path => Some(ShapePlace::Moving(None)),
            Self::Placed | Self::Idle(_) => Some(ShapePlace::Fixed(frame, scale)),
        }
    }
}

/// The moving shapes of a placeable that follows a path or plays an idle
/// animation: its entity, where the map puts it, its path and animation,
/// and each moving shape's index among its shapes
/// ([`PlaceableBody::shapes`]) with its bone, if any, and its model node.
#[derive(Clone, Debug)]
pub struct MovingBody {
    entity: u32,
    frame: ContactFrame,
    scale: [f32; 3],
    motion: Option<IdleMotion>,
    path: Option<Arc<PathFollower>>,
    parts: Vec<(u32, Option<usize>, u32)>,
    /// Server-driven movement records sampled at world time. Kept for
    /// posing at a rider's delayed view time and one tick before it.
    track: Option<Arc<std::sync::Mutex<TrackMotion>>>,
}

/// A bounded history of the exact grid positions sent to clients.
#[derive(Debug)]
struct TrackMotion {
    positions: VecDeque<(f64, [f32; 3])>,
}

impl TrackMotion {
    const CAPACITY: usize = 256;

    fn new(position: [f32; 3]) -> Self {
        let mut positions = VecDeque::with_capacity(Self::CAPACITY);
        positions.push_back((0.0, position));
        Self { positions }
    }

    fn record(&mut self, time: f64, position: [f32; 3]) {
        if self.positions.back().is_some_and(|&(last, _)| last >= time) {
            return;
        }
        if self.positions.len() == Self::CAPACITY {
            self.positions.pop_front();
        }
        self.positions.push_back((time, position));
    }

    fn at(&self, time: f64) -> [f32; 3] {
        let &(mut before, mut position) = self.positions.front().expect("initial position");
        for &(after, next) in self.positions.iter().skip(1) {
            if time <= after {
                let share = ((time - before) / (after - before)).clamp(0.0, 1.0) as f32;
                return std::array::from_fn(|axis| {
                    position[axis] + (next[axis] - position[axis]) * share
                });
            }
            before = after;
            position = next;
        }
        position
    }
}

impl MovingBody {
    /// Each moving shape's index and frame: the placeable's frame (where
    /// its path puts it at the path clock's `path_time`, or where the map
    /// does), after its bone's move at the idle clock's `idle_time`, the
    /// move's translation scaled by the placeable's scale (see the module
    /// notes of [`crate::idle_animation`] and [`crate::path_follower`]).
    pub fn frames(&self, idle_time: f32, path_time: f64) -> Vec<(u32, ContactFrame)> {
        let moves = self.motion.as_ref().map(|motion| motion.moves(idle_time));
        let (mut translation, rotation) = self
            .path
            .as_ref()
            .and_then(|path| path.place(path_time, self.frame.rotation))
            .unwrap_or((self.frame.translation, self.frame.rotation));
        if let Some(track) = &self.track {
            translation = track
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .at(path_time);
        }
        let base = ContactFrame {
            translation,
            rotation,
        };
        let rotation = rotation.map(f64::from);
        let translation = translation.map(f64::from);
        self.parts
            .iter()
            .map(|&(index, bone, _)| {
                let Some(moved) = bone.zip(moves.as_ref()).map(|(bone, moves)| &moves[bone]) else {
                    return (index, base);
                };
                let scaled =
                    std::array::from_fn(|k| moved.translation[k] * f64::from(self.scale[k]));
                let turned = rotate(rotation, scaled);
                let frame = ContactFrame {
                    translation: std::array::from_fn(|k| (translation[k] + turned[k]) as f32),
                    rotation: multiply(rotation, moved.rotation).map(|v| v as f32),
                };
                (index, frame)
            })
            .collect()
    }
}

/// The placeables' moving shapes and the clocks the match last posed them
/// at, which the movers ask for their platforms' frames a tick before
/// (`lucy_physics` `PlatformClock`).
#[derive(Debug, Default)]
pub struct MovingBodies {
    bodies: Vec<(usize, MovingBody)>,
    /// The idle and the path clock of the last pose.
    times: std::sync::Mutex<(f32, f64)>,
}

impl MovingBodies {
    /// `bodies`: each moving body by its switched group.
    pub fn new(bodies: Vec<(usize, MovingBody)>) -> Self {
        Self {
            bodies,
            times: std::sync::Mutex::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Keep the positions that the next movement packets carry. Paths
    /// and idle animations keep their existing independent clocks.
    pub fn record_tracks(&self, world: Option<&lucy_script::placed::MapEntityWorld>, time: f64) {
        let Some(world) = world else {
            return;
        };
        for (_, body) in &self.bodies {
            if let Some(track) = &body.track
                && let Some(placed) = world.entity(body.entity)
            {
                let position = placed
                    .position
                    .map(|axis| ((axis * 1024.0).round_ties_even() / 1024.0) as f32);
                track
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record(time, position);
            }
        }
    }

    /// Each body's group and its moving shapes' indexes and frames at the
    /// idle clock's `idle` and the path clock's `path`, which become the
    /// clocks of the poses now.
    pub fn pose(&self, idle: f32, path: f64) -> Vec<(usize, Vec<(u32, ContactFrame)>)> {
        *self
            .times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (idle, path);
        self.bodies
            .iter()
            .map(|(group, body)| (*group, body.frames(idle, path)))
            .collect()
    }
}

impl PlatformClock for MovingBodies {
    /// The platform posed `back` before now on both clocks: its path and
    /// its bones (the client's velocity of a bobbing Range platform has the
    /// bob in it: session `D:/Temp/mm/r2`; with the path alone moving back
    /// the vertical part of the local velocity was 0.1 m/s off).
    fn frame_before(&self, platform: &Platform, back: f32) -> Option<ContactFrame> {
        let (idle, path) = *self
            .times
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let body = self
            .bodies
            .iter()
            .map(|(_, body)| body)
            .find(|body| body.entity == platform.entity)?;
        let index = body
            .parts
            .iter()
            .find(|&&(_, _, node)| node == platform.node)?
            .0;
        let (_, frame) = body
            .frames(idle - back, path - f64::from(back))
            .into_iter()
            .find(|&(part, _)| part == index)?;
        Some(compose(frame, platform.node_place))
    }
}

impl PlaceableBody {
    /// The entity the client makes of it ([`PlacedScript::entity`]).
    pub fn entity(&self) -> u32 {
        0x8000_0000 | self.placeable.placeable
    }

    /// Each of its shapes the client places, with where.
    fn placed(&self) -> impl Iterator<Item = (&ModelShape, ShapePlace)> + '_ {
        self.model.iter().filter_map(|shape| {
            let place = self.pose.place(
                shape,
                self.frame,
                self.scale,
                self.path.is_some() || self.track,
            )?;
            Some((shape, place))
        })
    }

    /// Its shapes in the world as the client places them
    /// ([`ModelPose::place`]), each in its frame times its scale as the
    /// movement cache places a prop's (`ContactWorld::from_movement_cache`),
    /// with its triangles for the mover's ground rays as the movement
    /// cache's triangle index keeps a prop's (`StaticShape::with_ground`):
    /// the client's ray meets them, so a mover stands on the face under
    /// its centre, not on what the disc's sweep catches beside it (the
    /// Training Partner console's top, refs `MapEntities.md`
    /// "RangeConsole"). A shape on a path or an animated part of an idle
    /// animation is a moving shape, at the placeable's frame until the
    /// match poses it ([`Self::moving`]). A platform's shapes name it
    /// ([`Platform`]: the entity, the shape's node, that node's place
    /// scaled as the shape).
    pub fn shapes(&self) -> Result<Vec<StaticShape>, ContactWorldError> {
        self.placed()
            .map(|(shape, place)| {
                let (frame, scale) = match place {
                    ShapePlace::Fixed(frame, scale) => (frame, scale),
                    ShapePlace::Moving(_) => (self.frame, self.scale),
                };
                let geometry = match &shape.geometry {
                    ModelGeometry::Mesh(mask, mesh) => StaticGeometry::Mesh(Arc::new(
                        ContactMesh::new(Arc::clone(mesh), scale, *mask)?,
                    )),
                    ModelGeometry::Solid(solid) => {
                        scaled_solid(solid, scale).ok_or(ContactWorldError::InvalidGeometry)?
                    }
                };
                let ground = probe_triangles(&shape.cache_shape(), scale)
                    .map_err(|_| ContactWorldError::InvalidGeometry)?;
                let placed = StaticShape::new(geometry, Some(frame))?.with_ground(ground)?;
                let placed = match self.platform {
                    Some(factor) => placed.on_platform(Platform {
                        entity: self.entity(),
                        node: shape.node,
                        node_place: ContactFrame {
                            translation: std::array::from_fn(|k| {
                                shape.node_place.translation[k] * scale[k]
                            }),
                            rotation: shape.node_place.rotation,
                        },
                        factor,
                    }),
                    None => placed,
                };
                Ok(match place {
                    ShapePlace::Fixed(..) => placed,
                    ShapePlace::Moving(_) => placed.moving(),
                })
            })
            .collect()
    }

    /// Its moving shapes, when it has any.
    pub fn moving(&self) -> Option<MovingBody> {
        let parts: Vec<(u32, Option<usize>, u32)> = self
            .placed()
            .zip(0..)
            .filter_map(|((shape, place), index)| match place {
                ShapePlace::Moving(bone) => Some((index, bone, shape.node)),
                ShapePlace::Fixed(..) => None,
            })
            .collect();
        let motion = match &self.pose {
            ModelPose::Idle(motion) => motion.clone(),
            ModelPose::Placed | ModelPose::Origin => None,
        };
        (!parts.is_empty()).then(|| MovingBody {
            entity: self.entity(),
            frame: self.frame,
            scale: self.scale,
            motion,
            path: self.path.clone(),
            track: self.track.then(|| {
                Arc::new(std::sync::Mutex::new(TrackMotion::new(
                    self.frame.translation,
                )))
            }),
            parts,
        })
    }

    /// Its fixed shapes that the client's shots meet (categories 1 and
    /// 0x80, [`RAY_CATEGORIES`]), placed as [`Self::shapes`] and as the ray
    /// cache places a prop's; `None` when it has none, and for a team wall,
    /// which the client takes for one team's viewers only (see the module
    /// notes).
    pub fn ray_scene(&self) -> Result<Option<CollisionScene>, SceneError> {
        if self.team_wall {
            return Ok(None);
        }
        let mut place = None;
        let model: Vec<Shape> = self
            .placed()
            .filter(|(shape, _)| shape.category & RAY_CATEGORIES != 0)
            .filter_map(|(shape, at)| match at {
                ShapePlace::Fixed(frame, scale) => {
                    place = Some((frame, scale));
                    Some(shape.cache_shape())
                }
                ShapePlace::Moving(_) => None,
            })
            .collect();
        let Some((frame, scale)) = place else {
            return Ok(None);
        };
        let placement = CachePlacement {
            model: 0,
            group: 0,
            translation: frame.translation,
            scale,
            rotation: frame.rotation,
        };
        CollisionScene::placed_model(&model, placement).map(Some)
    }

    /// Its moving shapes as the rays meet them
    /// (`crate::hitscan::PlaceableRays`): each one's index among its shapes
    /// ([`Self::shapes`]), category and geometry in that moving shape's
    /// frame (scaled by the placeable's scale; the movers' world gives the
    /// frame). None for a team wall.
    pub fn moving_ray_parts(&self) -> Result<Vec<(u32, u16, CollisionScene)>, SceneError> {
        if self.team_wall {
            return Ok(Vec::new());
        }
        let placement = CachePlacement {
            model: 0,
            group: 0,
            translation: [0.0; 3],
            scale: self.scale,
            rotation: ContactFrame::IDENTITY.rotation,
        };
        self.placed()
            .zip(0..)
            .filter(|((_, place), _)| matches!(place, ShapePlace::Moving(_)))
            .map(|((shape, _), index)| {
                let scene = CollisionScene::placed_model(&[shape.cache_shape()], placement)?;
                Ok((index, shape.category, scene))
            })
            .collect()
    }
}

impl ModelShape {
    /// Its geometry as the collision cache keeps a prop's.
    fn cache_shape(&self) -> Shape {
        match &self.geometry {
            ModelGeometry::Mesh(mask, mesh) => Shape::Mesh {
                mask: *mask,
                mesh: Mesh::clone(mesh),
            },
            ModelGeometry::Solid(solid) => solid.clone(),
        }
    }
}

/// Whether a team wall stands in the world now: `0FA8`'s state 3 is active
/// in its placed script (`entity`, none once it is gone), and the script is
/// complete, so its frames tell the clients the same (a script with gaps
/// sends none, `crate::placed_script`, and their walls stay as they were).
pub fn team_wall_on(entity: Option<&PlacedEntity>) -> bool {
    entity
        .filter(|entity| entity.complete())
        .and_then(PlacedEntity::snapshot)
        .is_some_and(|snapshot| {
            snapshot.instances.values().any(|instance| {
                instance.graph == TEAM_WALL_GRAPH
                    && instance
                        .states
                        .get(&TEAM_WALL_ON)
                        .is_some_and(|state| state.active)
            })
        })
}

/// One map's areas.
#[derive(Debug)]
struct MapAreas {
    rooms: Result<Arc<[SpawnRoom]>, String>,
    damage_areas: Vec<DamageArea>,
    server_placeables: Vec<MapPlaceable>,
    /// The server placeables' script data, in the same order.
    scripts: Vec<Arc<PlacedScript>>,
    /// The server placeables whose models stop the mover, in placeable
    /// order.
    bodies: Vec<PlaceableBody>,
    kill_volumes: Arc<[CaptureBox]>,
    /// The entities with target tags, with the placeable each is.
    tagged: Vec<(MapPlaceable, TaggedEntity)>,
}

/// Every map's spawn rooms, damage areas, server placeables and kill volumes, by
/// map GUID.
#[derive(Debug, Default)]
pub struct MapAreaData {
    maps: HashMap<u64, MapAreas>,
}

#[derive(Deserialize)]
struct RawFile {
    maps: HashMap<String, RawMap>,
    /// Each server placeable definition's physics shapes that stop the
    /// mover, in model space, by definition.
    #[serde(default)]
    models: HashMap<String, Vec<RawModelShape>>,
    /// Each idle-animated definition's rig, by definition
    /// ([`crate::idle_animation`]).
    #[serde(default)]
    idle_animations: HashMap<String, RawIdleRig>,
}

#[derive(Deserialize)]
struct RawMap {
    #[serde(default)]
    spawn_rooms: Vec<RawRoom>,
    error: Option<String>,
    #[serde(default)]
    damage_areas: Vec<RawDamageArea>,
    #[serde(default)]
    server_placeables: Vec<RawServerPlaceable>,
    #[serde(default)]
    kill_volumes: Vec<CaptureBox>,
    #[serde(default)]
    tagged: Vec<RawTagged>,
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

#[derive(Deserialize)]
struct RawServerPlaceable {
    #[serde(flatten)]
    placeable: RawPlaceable,
    #[serde(default)]
    definition: Option<String>,
    #[serde(default)]
    position: [f64; 3],
    #[serde(default)]
    script: RawScript,
    #[serde(default)]
    moving: bool,
    /// Its model has the animation component the server mirrors
    /// (component 39; [`ModelPose::Origin`]).
    #[serde(default)]
    posed: bool,
    /// Its definition gives it component 28 without component 39: its
    /// create carries its place ([`StatePlace`]).
    #[serde(default)]
    state_placed: bool,
    #[serde(default)]
    second_identifier: Option<String>,
    /// Its rotation and scale, when its definition has `models` shapes.
    #[serde(default)]
    rotation: Option<[f32; 4]>,
    #[serde(default)]
    scale: Option<[f32; 3]>,
    /// Where its idle animation starts and how fast it plays, when its
    /// instance data says (`STU_5AD3B86B`).
    #[serde(default)]
    idle: Option<RawIdleStart>,
    /// The path it follows (component 98), when its instance data names
    /// one ([`PathFollower`]).
    #[serde(default)]
    path: Option<RawPath>,
    /// Native parametric movement, not a client path follower.
    #[serde(default)]
    track: Option<Vec<crate::objective_data::WaypointData>>,
    /// The share of its velocity a mover standing on it takes, when it is
    /// a platform ([`PlaceableBody`]).
    #[serde(default)]
    platform: Option<f32>,
}

/// A placeable's idle animation start, in 30 fps frames, and rate.
#[derive(Deserialize)]
struct RawIdleStart {
    start: i32,
    rate: f32,
}

/// A model's physics shape with the category the client files it under and,
/// for an animated part of an idle-animated model, its bone in the rig.
#[derive(Deserialize)]
struct RawModelShape {
    category: u16,
    #[serde(default)]
    bone: Option<usize>,
    /// The model node it hangs on, and that node's place in the model.
    #[serde(default)]
    node: Option<u32>,
    #[serde(default)]
    node_place: Option<RawNodePlace>,
    #[serde(flatten)]
    shape: RawShape,
}

#[derive(Deserialize)]
struct RawNodePlace {
    translation: [f32; 3],
    rotation: [f32; 4],
}

/// A model's physics shape: a mesh's holder mask, vertices and triangles
/// (corners, wings, mask, surface); a polytope's mask, centroid, vertices,
/// planes, faces and half edges (twin, origin, face, next); a capsule's or
/// a sphere's mask and geometry.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RawShape {
    Mesh {
        mask: u16,
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[i32; 8]>,
    },
    Hull {
        mask: u16,
        centroid: [f32; 3],
        vertices: Vec<[f32; 3]>,
        planes: Vec<[f32; 4]>,
        faces: Vec<u8>,
        half_edges: Vec<[i16; 4]>,
    },
    Capsule {
        mask: u16,
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    Sphere {
        mask: u16,
        center: [f32; 3],
        radius: f32,
    },
}

impl RawShape {
    fn read(self) -> Result<ModelGeometry, &'static str> {
        Ok(match self {
            Self::Mesh {
                mask,
                vertices,
                triangles,
            } => {
                let count = vertices.len();
                let vertex = |index: i32| usize::try_from(index).is_ok_and(|index| index < count);
                let triangles = triangles
                    .into_iter()
                    .map(|[a, b, c, wa, wb, wc, mask, surface]| {
                        let (corners, wings) = ([a, b, c], [wa, wb, wc]);
                        if !corners.into_iter().all(vertex)
                            || !wings.into_iter().all(|wing| wing == -1 || vertex(wing))
                        {
                            return Err("a triangle's vertex is out of range");
                        }
                        Ok(Triangle {
                            corners,
                            wings,
                            mask: u16::try_from(mask).map_err(|_| "a triangle's mask")?,
                            surface: u16::try_from(surface).map_err(|_| "a triangle's surface")?,
                        })
                    })
                    .collect::<Result<_, _>>()?;
                ModelGeometry::Mesh(
                    mask,
                    Arc::new(Mesh {
                        vertices,
                        triangles,
                    }),
                )
            }
            Self::Hull {
                mask,
                centroid,
                vertices,
                planes,
                faces,
                half_edges,
            } => {
                let half_edges = half_edges
                    .into_iter()
                    .map(|[twin, origin, face, next]| {
                        let index = |value: i16, count: usize| {
                            u8::try_from(value)
                                .ok()
                                .filter(|value| usize::from(*value) < count)
                        };
                        match (
                            i8::try_from(twin),
                            index(origin, vertices.len()),
                            index(face, planes.len()),
                            index(next, 256),
                        ) {
                            (Ok(twin), Some(origin), Some(face), Some(next)) => Ok(HalfEdge {
                                twin,
                                origin,
                                face,
                                next,
                            }),
                            _ => Err("a polytope's edge is out of range"),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if faces.len() != planes.len()
                    || faces
                        .iter()
                        .chain(half_edges.iter().map(|edge| &edge.next))
                        .any(|&edge| usize::from(edge) >= half_edges.len())
                {
                    return Err("a polytope's face or edge is out of range");
                }
                ModelGeometry::Solid(Shape::Hull(Hull {
                    mask,
                    centroid,
                    vertices,
                    planes,
                    faces,
                    half_edges,
                }))
            }
            Self::Capsule { mask, a, b, radius } => {
                ModelGeometry::Solid(Shape::Capsule { mask, a, b, radius })
            }
            Self::Sphere {
                mask,
                center,
                radius,
            } => ModelGeometry::Solid(Shape::Sphere {
                mask,
                center,
                radius,
            }),
        })
    }
}

#[derive(Default, Deserialize)]
struct RawScript {
    #[serde(default)]
    overrides: Vec<GraphBinding>,
    #[serde(default)]
    extra: Vec<GraphBinding>,
}

#[derive(Deserialize)]
struct RawTagged {
    #[serde(flatten)]
    placeable: RawPlaceable,
    position: [f64; 3],
    rotation: [f64; 4],
    tags: Vec<String>,
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
        let definition_of = |key: &str, what: &str| {
            u64::from_str_radix(key.trim_start_matches("0x"), 16)
                .map_err(|_| invalid(format!("{FILE}: {what} key {key}")))
        };
        let mut rigs = HashMap::new();
        for (key, raw) in file.idle_animations {
            let rig = raw
                .rig()
                .map_err(|error| invalid(format!("{FILE}: idle animation {key}: {error}")))?;
            rigs.insert(definition_of(&key, "idle animation")?, Arc::new(rig));
        }
        let mut models = HashMap::new();
        for (key, shapes) in file.models {
            let definition = definition_of(&key, "model")?;
            let bones = rigs.get(&definition).map_or(0, |rig| rig.bones());
            let shapes = shapes
                .into_iter()
                .map(|raw| {
                    if raw.bone.is_some_and(|bone| bone >= bones) {
                        return Err("a shape on a bone the idle rig hasn't");
                    }
                    Ok(ModelShape {
                        category: raw.category,
                        bone: raw.bone,
                        node: raw.node.unwrap_or(0),
                        node_place: raw.node_place.map_or(ContactFrame::IDENTITY, |place| {
                            ContactFrame {
                                translation: place.translation,
                                rotation: place.rotation,
                            }
                        }),
                        geometry: raw.shape.read()?,
                    })
                })
                .collect::<Result<Arc<[ModelShape]>, &str>>()
                .map_err(|error| invalid(format!("{FILE}: model {key}: {error}")))?;
            models.insert(definition, shapes);
        }
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
            let mut server_placeables = Vec::new();
            let mut scripts = Vec::new();
            let mut bodies = Vec::new();
            for mut raw in raw.server_placeables {
                let placeable = raw.placeable.read(&key)?;
                let guid = |what: &str, text: Option<String>| {
                    text.map(|text| {
                        u64::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| {
                            invalid(format!(
                                "{FILE}: map {key}: placeable {}: {what} {text}",
                                placeable.placeable
                            ))
                        })
                    })
                    .transpose()
                };
                let definition = guid("definition", raw.definition)?.unwrap_or_default();
                let second_identifier = guid("second identifier", raw.second_identifier)?;
                let state_place = match (raw.state_placed, raw.rotation) {
                    (false, _) => None,
                    (true, Some(rotation)) => {
                        Some(StatePlace::new(raw.position, rotation).ok_or_else(|| {
                            invalid(format!(
                                "{FILE}: map {key}: placeable {}: state placed with a turn not \
                                 about y",
                                placeable.placeable
                            ))
                        })?)
                    }
                    (true, None) => {
                        return Err(invalid(format!(
                            "{FILE}: map {key}: placeable {}: state placed without rotation",
                            placeable.placeable
                        )));
                    }
                };
                let track = raw
                    .track
                    .take()
                    .map(|points| {
                        let points: Vec<_> = points
                            .into_iter()
                            .map(|point| TrackPoint {
                                entity: 0x8000_0000 | point.placeable,
                                position: point.position,
                                curve: point.curve,
                                length: point.length,
                                barrier: point.barrier,
                                objective: None,
                            })
                            .collect();
                        Track::new(&points).map(Arc::new).map_err(|error| {
                            invalid(format!(
                                "{FILE}: map {key}: placeable {}: {error}",
                                placeable.placeable
                            ))
                        })
                    })
                    .transpose()?;
                if let Some(model) = models.get(&definition) {
                    let (Some(rotation), Some(scale)) = (raw.rotation, raw.scale) else {
                        return Err(invalid(format!(
                            "{FILE}: map {key}: placeable {}: shapes without rotation or scale",
                            placeable.placeable
                        )));
                    };
                    let team_wall = raw
                        .script
                        .overrides
                        .iter()
                        .chain(&raw.script.extra)
                        .any(|binding| binding.graph == Some(TEAM_WALL_GRAPH));
                    let path = raw
                        .path
                        .take()
                        .map(|path| path.path().map(Arc::new))
                        .transpose()
                        .map_err(|error| {
                            invalid(format!(
                                "{FILE}: map {key}: placeable {}: {error}",
                                placeable.placeable
                            ))
                        })?;
                    bodies.push(PlaceableBody {
                        placeable,
                        frame: state_place.map_or(
                            ContactFrame {
                                translation: raw.position.map(|axis| axis as f32),
                                rotation,
                            },
                            |place| place.frame(),
                        ),
                        // The movement state carries no scale: the client
                        // keeps such a model unscaled (Anubis's door 971,
                        // map scale 0.5, when its create carried its place;
                        // refs `MapEntities.md` "MapAnim").
                        scale: if state_place.is_some() {
                            [1.0; 3]
                        } else {
                            scale
                        },
                        model: Arc::clone(model),
                        team_wall,
                        pose: match rigs.get(&definition).filter(|_| raw.moving) {
                            _ if raw.posed => ModelPose::Origin,
                            Some(rig) => {
                                let start = raw.idle.as_ref();
                                ModelPose::Idle(Some(IdleMotion {
                                    rig: Arc::clone(rig),
                                    start: start.map_or(0, |idle| idle.start),
                                    rate: start.map_or(1.0, |idle| idle.rate),
                                }))
                            }
                            None if raw.moving => ModelPose::Idle(None),
                            None => ModelPose::Placed,
                        },
                        path,
                        track: track.is_some(),
                        platform: raw.platform,
                    });
                }
                scripts.push(Arc::new(PlacedScript {
                    placeable: placeable.placeable,
                    definition,
                    position: raw.position,
                    overrides: raw.script.overrides,
                    extra: raw.script.extra,
                    moving: raw.moving,
                    second_identifier,
                    state_place,
                    track,
                }));
                server_placeables.push(placeable);
            }
            for (number, volume) in (0u32..).zip(&raw.kill_volumes) {
                check_boxes(std::slice::from_ref(volume), &key, "kill volume", number)?;
            }
            let mut tagged = Vec::new();
            for raw in raw.tagged {
                let placeable = raw.placeable.read(&key)?;
                let tags = raw
                    .tags
                    .iter()
                    .map(|text| {
                        u64::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| {
                            invalid(format!(
                                "{FILE}: map {key}: placeable {}: tag {text}",
                                placeable.placeable
                            ))
                        })
                    })
                    .collect::<io::Result<_>>()?;
                let entity = TaggedEntity {
                    entity: 0x8000_0000 | placeable.placeable,
                    position: raw.position,
                    rotation: raw.rotation,
                    tags,
                };
                tagged.push((placeable, entity));
            }
            maps.insert(
                guid,
                MapAreas {
                    rooms,
                    damage_areas,
                    server_placeables,
                    scripts,
                    bodies,
                    kill_volumes: raw.kill_volumes.into(),
                    tagged,
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

    /// The script data of the map's server placeables that exist in a mode
    /// that lists `identifiers`, in placeable order.
    pub fn placed_scripts(
        &self,
        map: &GameMap,
        identifiers: &BTreeSet<u64>,
    ) -> Vec<Arc<PlacedScript>> {
        self.maps
            .get(&map.map_guid)
            .map(|areas| {
                areas
                    .server_placeables
                    .iter()
                    .zip(&areas.scripts)
                    .filter(|(placeable, _)| placeable.in_mode(identifiers))
                    .map(|(_, script)| Arc::clone(script))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The map's server placeables whose models stop the mover that exist
    /// in a mode that lists `identifiers`, in placeable order (see the
    /// module notes).
    pub fn placeable_bodies(
        &self,
        map: &GameMap,
        identifiers: &BTreeSet<u64>,
    ) -> Vec<&PlaceableBody> {
        self.maps
            .get(&map.map_guid)
            .map(|areas| {
                areas
                    .bodies
                    .iter()
                    .filter(|body| body.placeable.in_mode(identifiers))
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

    /// The map's tagged entities that exist in a mode that lists
    /// `identifiers`, for the scene tag queries (`lucy_script::map_tags`;
    /// `graphs` tells which tags bodies may carry).
    pub fn map_tags(
        &self,
        map: &GameMap,
        identifiers: &BTreeSet<u64>,
        graphs: &GraphStore,
    ) -> Arc<MapTags> {
        let entities = self
            .maps
            .get(&map.map_guid)
            .into_iter()
            .flat_map(|areas| &areas.tagged)
            .filter(|(placeable, _)| placeable.in_mode(identifiers))
            .map(|(_, entity)| entity.clone())
            .collect();
        Arc::new(MapTags::new(entities, graphs))
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

/// The zones the client's `0x7FF789B2F280` tests a falling ray against
/// (Meteor Strike's floor, `lucy_physics::mover::rise`): the world's
/// kill height (the client's world `+276`; here the catch floor,
/// [`kill_height`], an assumption) and the damage areas (the client keeps
/// those with `m_1BB427AE` set; here the lethal ones, an assumption).
#[derive(Debug)]
pub struct DeathZones {
    pub kill_height: Option<f64>,
    pub areas: std::sync::Arc<[DamageArea]>,
}

impl lucy_physics::mover::DeathZones for DeathZones {
    /// Below the kill height: the start when it is below, else where the
    /// segment crosses it (the start when it is level within 0.004); then
    /// the highest entry into an area's box over that.
    fn entry(&self, start: [f64; 3], end: [f64; 3]) -> Option<[f64; 3]> {
        let mut found = None;
        if let Some(height) = self.kill_height {
            if start[1] < height {
                return Some(start);
            }
            if end[1] < height {
                found = Some(if (start[1] - end[1]).abs() > 0.0040000002 {
                    let share = (height - start[1]) / (end[1] - start[1]);
                    std::array::from_fn(|axis| (end[axis] - start[axis]) * share + start[axis])
                } else {
                    start
                });
            }
        }
        for share in self
            .areas
            .iter()
            .flat_map(|area| &area.boxes)
            .filter_map(|area| area.entry(start, end))
        {
            let point: [f64; 3] =
                std::array::from_fn(|axis| (end[axis] - start[axis]) * share + start[axis]);
            if found.is_none_or(|best: [f64; 3]| point[1] > best[1]) {
                found = Some(point);
            }
        }
        found
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
