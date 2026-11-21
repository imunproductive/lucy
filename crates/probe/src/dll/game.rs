//! The frame sampler: a hook in the client's input sampler, once per
//! rendered frame on the client's main thread. It writes the script's input
//! into the sample, runs queued calls, and records the local player's body:
//! its position, its mover's movement state, the camera and the watched
//! statescript instances.

use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Instant,
};

use serde_json::{Map, Value, json};

use super::{Probe, call, hook, mem};
use crate::{
    client::*,
    expr::Expr,
    input::{Sample, Script, view_quaternion},
    layout::{
        CAMERA_MATRIX_TABLE, Camera, MOVEMENT_BYTES, MOVEMENT_TICKED, Movement, script_value,
    },
    ring::{Event, FrameSample},
};

#[derive(Default)]
pub struct Game {
    pub state: Mutex<State>,
    /// Input samples seen.
    pub frames: AtomicU64,
    /// The local player's component 59, its entity (the player, whose
    /// component 59 it is), its body's component 4 and 31 (for the read
    /// expressions' `c59`, `ent`, `c4`, `c31`).
    pub comp59: AtomicU64,
    pub entity: AtomicU64,
    pub comp4: AtomicU64,
    pub comp31: AtomicU64,
    /// The body's entity id: set by `sampler {"body"}`, else 0 for the one
    /// the player possesses (its component 44).
    pub body: AtomicU32,
    pub frame_hook: Mutex<Option<u32>>,
}

/// A statescript graph whose instances the sampler records each frame.
#[derive(Clone, Debug)]
pub struct Watch {
    /// On the player entity (else on the body).
    pub player: bool,
    /// The graph's id (the low 16 bits of its GUID, `0D06`).
    pub graph: u16,
    /// Variables to read (none: all the instance has).
    pub variables: Option<Vec<u16>>,
    /// Entity variables (`#Ne`) of the instance's entity to read.
    pub entity_variables: Vec<u16>,
}

impl Watch {
    /// `0D06`, `player:0C90`, `0D06:2083,11432` (graph, then variables),
    /// `player:288B:1962,31435e` (`e`: an entity variable).
    pub fn parse(text: &str) -> Result<Self, String> {
        let (player, rest) = match text.strip_prefix("player:") {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix("body:").unwrap_or(text)),
        };
        let (graph, list) = match rest.split_once(':') {
            Some((graph, list)) => (graph, Some(list)),
            None => (rest, None),
        };
        let graph = u16::from_str_radix(graph.trim(), 16)
            .map_err(|_| format!("{text:?}: the graph is 4 hex digits (0D06)"))?;
        let mut variables = list.map(|_| Vec::new());
        let mut entity_variables = Vec::new();
        for id in list.into_iter().flat_map(|list| list.split(',')) {
            let id = id.trim().trim_start_matches('#');
            let (id, entity) = match id.strip_suffix('e') {
                Some(id) => (id, true),
                None => (id, false),
            };
            let id = id
                .parse::<u16>()
                .map_err(|_| format!("{text:?}: variables are decimal ids (2083, 31435e)"))?;
            if entity {
                entity_variables.push(id);
            } else if let Some(variables) = &mut variables {
                variables.push(id);
            }
        }
        Ok(Self {
            player,
            graph,
            variables,
            entity_variables,
        })
    }

    pub fn text(&self) -> String {
        let mut text = format!(
            "{}{:04X}",
            if self.player { "player:" } else { "" },
            self.graph
        );
        if let Some(variables) = &self.variables {
            let list: Vec<String> = variables
                .iter()
                .map(u16::to_string)
                .chain(self.entity_variables.iter().map(|id| format!("{id}e")))
                .collect();
            text += &format!(":{}", list.join(","));
        }
        text
    }
}

pub struct State {
    /// Record a frame sample every `every` input samples (0: none).
    pub every: u64,
    pub reads: Vec<Expr>,
    /// Read the camera each recorded frame.
    pub camera: bool,
    pub watches: Vec<Watch>,
    pub script: Script,
    pub calls: VecDeque<call::Request>,
    pub last: Option<FrameSample>,
    /// Where the position last changed, for the velocity.
    moved: Option<(Instant, [f32; 3])>,
    velocity: Option<[f32; 3]>,
    /// The last camera error, logged once.
    camera_error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            every: 0,
            reads: Vec::new(),
            camera: true,
            watches: Vec::new(),
            script: Script::default(),
            calls: VecDeque::new(),
            last: None,
            moved: None,
            velocity: None,
            camera_error: None,
        }
    }
}

/// `c59`, `ent`, `c4`, `c31` in read expressions.
pub fn base_name(probe: &Probe, name: &str) -> Option<u64> {
    let game = &probe.game;
    let value = match name {
        "c59" => game.comp59.load(Ordering::Relaxed),
        "ent" => game.entity.load(Ordering::Relaxed),
        "c4" => game.comp4.load(Ordering::Relaxed),
        "c31" => game.comp31.load(Ordering::Relaxed),
        _ => return None,
    };
    (value != 0).then_some(value)
}

/// Install the frame hook unless it is in.
pub fn ensure_frame_hook(probe: &Probe) -> Result<u32, String> {
    let mut frame_hook = probe
        .game
        .frame_hook
        .lock()
        .map_err(|_| "frame hook lock")?;
    if let Some(id) = *frame_hook {
        return Ok(id);
    }
    let hook = hook::install(
        probe,
        probe.runtime(SAMPLE_SITE),
        "frame sampler".into(),
        false,
        hook::Action::Frame,
        0,
    )?;
    probe.log(&format!("frame hook {} at {:#X}", hook.id, hook.address));
    *frame_hook = Some(hook.id);
    Ok(hook.id)
}

/// The owner entity of a component: `[comp+8]` decoded.
pub fn owner(probe: &Probe, component: u64) -> Option<u64> {
    let key = mem::read_u64(probe.runtime(POINTER_KEY))?;
    let mask = key.rotate_left(2) ^ POINTER_SALT;
    let entity = mem::read_u64(component + 8)? ^ mask;
    (0x10000..1 << 47).contains(&entity).then_some(entity)
}

pub fn entity_id(entity: u64) -> Option<u32> {
    mem::read_u32(entity + ENTITY_ID)
}

/// Components with vtable `vtable` and their owners: (component, entity,
/// entity id). A destroyed component keeps its vtable but loses its owner.
pub fn components(probe: &Probe, vtable: u64, max: usize) -> Vec<(u64, u64, u32)> {
    mem::scan_u64(&mem::heap_regions(), vtable, max)
        .into_iter()
        .filter_map(|component| {
            let entity = owner(probe, component)?;
            Some((component, entity, entity_id(entity)?))
        })
        .collect()
}

/// The process's `PEB.Ldr`, part of the client's pointer mask.
fn peb_loader() -> u64 {
    let peb: u64;
    // SAFETY: reads the PEB address from the TEB (`gs:[0x60]` on x64).
    unsafe { std::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly)) };
    mem::read_u64(peb + 0x18).unwrap_or(0)
}

/// A component pointer as the client stores it in an entity's array,
/// unmasked (the inlined code after `Prot_MaskingStub` in each lookup).
fn unmask(probe: &Probe, slot: u64) -> Option<u64> {
    let index = MASK_INDEX
        .wrapping_mul(2)
        .wrapping_sub(probe.runtime(MASK_STUB));
    let seed = MASK_SEED ^ peb_loader() ^ MASK_LDR_KEY;
    let table = probe.runtime(MASK_TABLE);
    let near = mem::read_u64(table + (index & 0xFFF))?;
    let far = mem::read_u64(table + (index >> 52))? as u32;
    let (low, high) = (slot as u32, (slot >> 32) as u32);
    let high = near as u32 ^ low ^ high;
    let low9 = seed as u32 ^ low;
    let high9 = (seed >> 32) as u32 ^ low.wrapping_sub(MASK_ADD) ^ high;
    let high10 = far.rotate_right(3).wrapping_mul(2).wrapping_sub(low9) ^ high9;
    let high = ((near >> 32) as u32).wrapping_sub(low9) ^ high10;
    Some(u64::from(low9) | u64::from(high) << 32)
}

/// Component `kind` of `entity`, as the client's own lookup finds it.
pub fn entity_component(probe: &Probe, entity: u64, kind: u8) -> Option<u64> {
    let word = u64::from(kind / 64);
    let bit = kind % 64;
    let bits = mem::read_u64(entity + COMPONENT_BITS + 8 * word)?;
    if bits >> bit & 1 == 0 {
        return None;
    }
    let mut first = [0u8];
    if !mem::read(entity + COMPONENT_FIRST + word, &mut first) {
        return None;
    }
    let index = u64::from(first[0]) + u64::from((bits & ((1u64 << bit) - 1)).count_ones());
    let array = mem::read_u64(entity + COMPONENT_ARRAY)?;
    let component = unmask(probe, mem::read_u64(array + 8 * index)?)?;
    (component != 0).then_some(component)
}

/// The entity with id `id` in the world of `entity` (any entity of it).
pub fn entity_by_id(probe: &Probe, entity: u64, id: u32) -> Option<u64> {
    let world = mem::read_u64(entity + ENTITY_WORLD)? ^ WORLD_KEY;
    let key = mem::read_u64(probe.runtime(ENTITIES_KEY_GLOBAL))?;
    let buckets = mem::read_u64(world + WORLD_ENTITIES)?
        ^ key
            .rotate_right(4)
            .wrapping_mul(2)
            .wrapping_add(ENTITIES_KEY);
    let bucket = buckets + 16 * u64::from(id & 0xFFF);
    let count = mem::read_u32(bucket + 8)? as usize;
    let entries = read_u64s(mem::read_u64(bucket)?, 2 * count.min(4096))?;
    entries
        .chunks_exact(2)
        .find(|entry| entry[0] as u32 == id)
        .map(|entry| entry[1])
        .filter(|&found| found != 0)
}

/// Component `kind` of the entity with id `id`, found through the world of
/// `entity` (0: none).
pub fn component(probe: &Probe, entity: u64, kind: u8, id: u32) -> u64 {
    if entity == 0 || id == 0 || id == u32::MAX {
        return 0;
    }
    entity_by_id(probe, entity, id)
        .and_then(|owner| entity_component(probe, owner, kind))
        .unwrap_or(0)
}

/// One input sample (the frame hook's handler).
pub fn on_frame(probe: &Probe, regs: &mut hook::Regs) {
    let game = &probe.game;
    let comp59 = regs.rdi;
    let index = game.frames.fetch_add(1, Ordering::Relaxed) + 1;
    game.comp59.store(comp59, Ordering::Relaxed);
    let entity = owner(probe, comp59).unwrap_or(0);
    game.entity.store(entity, Ordering::Relaxed);
    let player_id = if entity != 0 {
        entity_id(entity).unwrap_or(0)
    } else {
        0
    };
    let Ok(mut state) = game.state.lock() else {
        return;
    };
    let body_id = match game.body.load(Ordering::Relaxed) {
        0 => match component(probe, entity, 44, player_id) {
            0 => 0,
            comp44 => mem::read_u32(comp44 + POSSESSED).unwrap_or(0),
        },
        body => body,
    };
    let comp4 = component(probe, entity, 4, body_id);
    game.comp4.store(comp4, Ordering::Relaxed);
    let comp31 = component(probe, entity, 31, body_id);
    game.comp31.store(comp31, Ordering::Relaxed);
    let now_ms = probe.started.elapsed().as_millis() as u64;
    if state.script.active() {
        let view = mem::read_i16_pair(comp59 + VIEW_YAW).unwrap_or([0, 0]);
        let sample = state.script.next(now_ms, (view[0], view[1]));
        apply(probe, comp59, &sample);
    }
    while let Some(request) = state.calls.pop_front() {
        call::run(probe, request);
    }
    let position = (comp4 != 0)
        .then(|| mem::read_f32x3(comp4 + COMP4_POSITION))
        .flatten();
    if let Some(position) = position {
        let now = Instant::now();
        match state.moved {
            Some((at, last)) if last != position => {
                let seconds = now.duration_since(at).as_secs_f32();
                if seconds > 0.0 {
                    state.velocity = Some([0, 1, 2].map(|i| (position[i] - last[i]) / seconds));
                }
                state.moved = Some((now, position));
            }
            Some((at, _)) if now.duration_since(at).as_secs_f32() > 0.1 => {
                state.velocity = Some([0.0; 3]);
            }
            None => state.moved = Some((now, position)),
            _ => {}
        }
    }
    let record = state.every > 0 && index % state.every == 0;
    let mover = (comp31 != 0)
        .then(|| {
            let mut bytes = [0u8; MOVEMENT_BYTES];
            mem::read(comp31 + MOVEMENT_TICKED, &mut bytes).then(|| Movement::parse(&bytes))?
        })
        .flatten();
    let mut sample = FrameSample {
        index,
        frame: ring_frame(comp59),
        comp59,
        player: entity,
        player_id,
        body_id,
        comp4,
        comp31,
        position,
        velocity: state.velocity,
        mover,
        camera: None,
        view: mem::read_i16_pair(comp59 + VIEW_YAW).unwrap_or([0, 0]),
        buttons: mem::read_u32(comp59 + SAMPLE_BUTTONS).unwrap_or(0),
        forward: read_i8(comp59 + SAMPLE_FORWARD),
        right: read_i8(comp59 + SAMPLE_RIGHT),
        action: read_i8(comp59 + SAMPLE_ACTION) as u8,
        scripts: Vec::new(),
        reads: Vec::new(),
    };
    if record && state.camera && entity != 0 {
        match camera(probe, entity) {
            Ok(camera) => {
                sample.camera = Some(camera);
                state.camera_error = None;
            }
            Err(error) => {
                if state.camera_error.as_ref() != Some(&error) {
                    probe.log(&format!("camera: {error}"));
                    state.camera_error = Some(error);
                }
            }
        }
    }
    if record && !state.watches.is_empty() {
        for watch in &state.watches {
            let id = if watch.player { player_id } else { body_id };
            let comp47 = component(probe, entity, 47, id);
            if comp47 != 0 {
                sample
                    .scripts
                    .extend(script_samples(comp47 + HOST_IN_COMP47, id, watch));
            }
        }
    }
    if record && !state.reads.is_empty() {
        let names = |name: &str| match name {
            "c59" => Some(comp59),
            "ent" => (entity != 0).then_some(entity),
            "c4" => (comp4 != 0).then_some(comp4),
            "c31" => (comp31 != 0).then_some(comp31),
            "base" => Some(probe.base),
            _ => regs.get(name),
        };
        sample.reads = state
            .reads
            .iter()
            .map(|r| r.eval(&names, &mem::read))
            .collect();
    }
    if record {
        probe.record(Event::Frame(Box::new(sample.clone())));
    }
    state.last = Some(sample);
}

/// The camera's world matrix: the camera object from the world's camera
/// system, its matrix decrypted with the key the client's own decryptor
/// gives (called through the in-image gadget, which its caller check needs).
fn camera(probe: &Probe, entity: u64) -> Result<Camera, String> {
    let world = mem::read_u64(entity + ENTITY_WORLD).ok_or("no world")? ^ WORLD_KEY;
    let key = mem::read_u64(probe.runtime(SYSTEMS_KEY_GLOBAL)).ok_or("no systems key")?;
    let systems =
        mem::read_u64(world + WORLD_SYSTEMS).ok_or("no systems")? ^ SYSTEMS_KEY.wrapping_sub(key);
    let system = mem::read_u64(systems + 8 * CAMERA_SYSTEM).ok_or("no camera system")?;
    let gadget = call::gadget(probe).ok_or("no call gadget (the image is not writable)")?;
    let callers = probe.runtime(DECRYPT_CALLERS);
    let low = mem::read_u64(callers).ok_or("no decryptor caller range")?;
    let size = mem::read_u32(callers + 8).ok_or("no decryptor caller range")?;
    if !(low..low + u64::from(size)).contains(&gadget) {
        return Err(format!(
            "the call gadget {gadget:#X} is outside the decryptors' caller range {low:#X}+{size:#X}"
        ));
    }
    let mut arguments = [0u64; call::MAX_ARGUMENTS];
    arguments[0] = system;
    let (camera, _) = call::call_here(probe, probe.runtime(CAMERA_GET), arguments)?;
    if camera == 0 {
        return Err("no camera".into());
    }
    arguments[0] = mem::read_u64(camera + CAMERA_SEED).ok_or("no camera key seed")?;
    let (key, _) = call::call_here(probe, probe.runtime(CAMERA_KEY), arguments)?;
    let mut bytes = [0u8; 64];
    if !mem::read(camera + CAMERA_MATRIX, &mut bytes) {
        return Err("the camera matrix is not readable".into());
    }
    let words: [u64; 8] = std::array::from_fn(|index| {
        u64::from_le_bytes(bytes[8 * index..8 * index + 8].try_into().expect("8 bytes"))
    });
    Ok(Camera::from_matrix(crate::layout::decrypt_words(
        words,
        key,
        &CAMERA_MATRIX_TABLE,
    )))
}

fn read_u64s(address: u64, count: usize) -> Option<Vec<u64>> {
    let mut bytes = vec![0u8; count * 8];
    mem::read(address, &mut bytes).then(|| {
        bytes
            .chunks_exact(8)
            .map(|word| u64::from_le_bytes(word.try_into().expect("8 bytes")))
            .collect()
    })
}

/// The instances of the watched graph on a statescript host: their running
/// states (the state array's indices whose state has flag bit 0), their
/// variables and the watch's entity variables of the host's entity.
fn script_samples(host: u64, entity: u32, watch: &Watch) -> Vec<Value> {
    let mut samples = Vec::new();
    let count = mem::read_u32(host + HOST_COUNT).unwrap_or(0) as usize;
    let Some(instances) = mem::read_u64(host + HOST_INSTANCES)
        .filter(|_| count <= 1024)
        .and_then(|array| read_u64s(array, count))
    else {
        return samples;
    };
    let entity_variables = (!watch.entity_variables.is_empty()).then(|| {
        variables(
            mem::read_u64(host + HOST_ENTITY_VARIABLES).unwrap_or(0),
            Some(&watch.entity_variables),
        )
    });
    for instance in instances {
        let mut head = [0u8; INSTANCE_GRAPH as usize + 8];
        if !mem::read(instance, &mut head) {
            continue;
        }
        let word = |at: u64| {
            u64::from_le_bytes(
                head[at as usize..at as usize + 8]
                    .try_into()
                    .expect("8 bytes"),
            )
        };
        if word(INSTANCE_GRAPH) & 0xFFFF != u64::from(watch.graph) {
            continue;
        }
        let id = word(INSTANCE_ID) as u16;
        let states = (word(INSTANCE_STATE_COUNT) as u32) as usize;
        let running: Vec<usize> = read_u64s(word(INSTANCE_STATES), states.min(4096))
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .filter(|&(_, state)| {
                state != 0 && mem::read_u32(state + STATE_FLAGS).is_some_and(|flags| flags & 1 != 0)
            })
            .map(|(index, _)| index)
            .collect();
        let mut sample = json!({
            "entity": entity,
            "graph": format!("{:04X}", watch.graph),
            "instance": id,
            "states": running,
            "vars": variables(word(INSTANCE_VARIABLES), watch.variables.as_deref()),
        });
        if let Some(entity_variables) = &entity_variables {
            sample["entity_vars"] = entity_variables.clone();
        }
        samples.push(sample);
    }
    samples
}

/// A variable store's variables (`{"2083": value}`), all or the given ones: an
/// instance's (`INSTANCE_VARIABLES`) or an entity's (`HOST_ENTITY_VARIABLES`).
/// A bound variable's value is the one its bindings gave when it was last
/// read.
fn variables(store: u64, wanted: Option<&[u16]>) -> Value {
    let mut out = Map::new();
    if store == 0 {
        return Value::Object(out);
    }
    let buckets: Vec<u64> = match wanted {
        Some(ids) => {
            let mut buckets: Vec<u64> = ids.iter().map(|&id| u64::from(id & 0xF)).collect();
            buckets.sort_unstable();
            buckets.dedup();
            buckets
        }
        None => (0..16).collect(),
    };
    for bucket in buckets {
        let at = store + VARIABLE_BUCKETS + 32 * bucket;
        let (Some(entries), Some(count)) = (mem::read_u64(at), mem::read_u32(at + 8)) else {
            continue;
        };
        let count = count as usize;
        if entries == 0 || count > 4096 {
            continue;
        }
        let mut table = vec![0u8; count * 16];
        if !mem::read(entries, &mut table) {
            continue;
        }
        for entry in table.chunks_exact(16) {
            let id = u16::from_le_bytes([entry[0], entry[1]]);
            if wanted.is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            let variable = u64::from_le_bytes(entry[8..16].try_into().expect("8 bytes"));
            if variable == 0 {
                continue;
            }
            let bound = mem::read_u32(variable + VARIABLE_BINDINGS).is_some_and(|n| n != 0);
            let at = if bound {
                VARIABLE_BOUND_VALUE
            } else {
                VARIABLE_VALUE
            };
            let mut value = [0u8; 16];
            if mem::read(variable + at, &mut value) {
                out.insert(id.to_string(), script_value(&value));
            }
        }
    }
    Value::Object(out)
}

fn read_i8(address: u64) -> i8 {
    let mut byte = [0u8; 1];
    if mem::read(address, &mut byte) {
        byte[0] as i8
    } else {
        0
    }
}

/// The newest frame in component 59's command ring.
fn ring_frame(comp59: u64) -> u32 {
    let mut ring = vec![0u8; (COMMAND_RECORD * COMMAND_SLOTS) as usize];
    if !mem::read(comp59 + COMMAND_RING, &mut ring) {
        return 0;
    }
    ring.chunks_exact(COMMAND_RECORD as usize)
        .map(|record| u32::from_le_bytes(record[..4].try_into().expect("4 bytes")))
        .max()
        .unwrap_or(0)
}

/// Write the script's input into the sample being built (ClientInput.md).
fn apply(probe: &Probe, comp59: u64, sample: &Sample) {
    if sample.buttons != 0 {
        let old = mem::read_u32(comp59 + SAMPLE_BUTTONS).unwrap_or(0);
        mem::write(
            comp59 + SAMPLE_BUTTONS,
            &(old | sample.buttons).to_le_bytes(),
        );
    }
    if sample.forward != 0 || sample.right != 0 {
        mem::write(
            comp59 + SAMPLE_RIGHT,
            &[sample.right as u8, sample.forward as u8],
        );
    }
    if sample.action != 0 {
        mem::write(comp59 + SAMPLE_ACTION, &[sample.action]);
    }
    if let Some((yaw, pitch)) = sample.turn_to {
        set_view(probe, comp59, yaw, pitch);
    }
    if let Some((yaw, pitch)) = sample.hold_view {
        let mut bytes = [0u8; 4];
        bytes[..2].copy_from_slice(&yaw.to_le_bytes());
        bytes[2..].copy_from_slice(&pitch.to_le_bytes());
        mem::write(comp59 + SAMPLE_YAW, &bytes);
    }
}

/// The client's set-view on this (the sampler's) thread: camera, prediction
/// and command turn together.
fn set_view(probe: &Probe, comp59: u64, yaw: i16, pitch: i16) {
    let quaternion = view_quaternion(yaw, pitch);
    let target = probe.runtime(SET_VIEW);
    // SAFETY: set_view(component 59, float[4]) is the client's own function
    // (ClientInput.md; owdbg calls it from this site), called on the
    // sampler's thread with the component the sampler is filling.
    unsafe {
        let set_view: extern "C" fn(u64, *const [f32; 4]) = std::mem::transmute(target);
        set_view(comp59, &quaternion);
    }
}
