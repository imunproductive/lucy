//! The frame sampler: a hook in the client's input sampler, once per
//! rendered frame on the client's main thread. It writes the script's input
//! into the sample, runs queued calls, and records the local body.

use std::{
    collections::VecDeque,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::Instant,
};

use super::{Probe, call, hook, mem};
use crate::{
    client::*,
    expr::Expr,
    input::{Sample, Script, view_quaternion},
    ring::{Event, FrameSample},
};

#[derive(Default)]
pub struct Game {
    pub state: Mutex<State>,
    /// Input samples seen.
    pub frames: AtomicU64,
    /// The local player's component 59, its entity (the player, whose
    /// component 59 it is) and its body's component 4 (for the read
    /// expressions' `c59`, `ent`, `c4`).
    pub comp59: AtomicU64,
    pub entity: AtomicU64,
    pub comp4: AtomicU64,
    /// The body's entity id: set by `sampler {"body"}`, else 0 for the
    /// player's id + 1 (player A0000100, body A0000101 in the Practice Range).
    pub body: AtomicU32,
    /// The body id whose component 4 the scanner looks for (0: none).
    scan: Mutex<u32>,
    scan_wake: Condvar,
    pub frame_hook: Mutex<Option<u32>>,
}

#[derive(Default)]
pub struct State {
    /// Record a frame sample every `every` input samples (0: none).
    pub every: u64,
    pub reads: Vec<Expr>,
    pub script: Script,
    pub calls: VecDeque<call::Request>,
    pub last: Option<FrameSample>,
    /// Where the position last changed, for the velocity.
    moved: Option<(Instant, [f32; 3])>,
    velocity: Option<[f32; 3]>,
}

/// `c59`, `ent`, `c4` in read expressions.
pub fn base_name(probe: &Probe, name: &str) -> Option<u64> {
    let game = &probe.game;
    let value = match name {
        "c59" => game.comp59.load(Ordering::Relaxed),
        "ent" => game.entity.load(Ordering::Relaxed),
        "c4" => game.comp4.load(Ordering::Relaxed),
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

fn comp4_of(probe: &Probe, comp4: u64, body: u32) -> bool {
    comp4 != 0
        && mem::read_u64(comp4) == Some(probe.runtime(COMP4_VTABLE))
        && owner(probe, comp4).and_then(entity_id) == Some(body)
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

/// The thread that finds the local body's component 4 (a heap scan of a
/// few seconds) whenever the frame sampler asks.
pub fn start_scanner(probe: &'static Probe) {
    let _ = thread::Builder::new()
        .name("owprobe-scan".into())
        .spawn(move || {
            let game = &probe.game;
            loop {
                let body = {
                    let Ok(mut wanted) = game.scan.lock() else {
                        return;
                    };
                    while *wanted == 0 {
                        wanted = match game.scan_wake.wait(wanted) {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };
                    }
                    *wanted
                };
                let started = Instant::now();
                let vtable = probe.runtime(COMP4_VTABLE);
                let found = components(probe, vtable, 1 << 16)
                    .into_iter()
                    .filter(|&(_, _, id)| id == body)
                    .map(|(component, _, _)| component)
                    .next_back();
                if let Some(comp4) = found {
                    game.comp4.store(comp4, Ordering::Relaxed);
                }
                probe.log(&format!(
                    "component 4 of body {body:08X}: {} in {:.1} s",
                    found.map_or("not found".into(), |c| format!("{c:#X}")),
                    started.elapsed().as_secs_f32()
                ));
                if let Ok(mut wanted) = game.scan.lock() {
                    if *wanted == body {
                        *wanted = 0;
                    }
                }
                // Not again at once when the body has none yet.
                thread::sleep(std::time::Duration::from_secs(2));
            }
        });
}

/// One input sample (the frame hook's handler).
pub fn on_frame(probe: &Probe, regs: &mut hook::Regs) {
    let game = &probe.game;
    let comp59 = regs.rdi;
    let index = game.frames.fetch_add(1, Ordering::Relaxed) + 1;
    game.comp59.store(comp59, Ordering::Relaxed);
    let entity = owner(probe, comp59).unwrap_or(0);
    game.entity.store(entity, Ordering::Relaxed);
    let body = match game.body.load(Ordering::Relaxed) {
        0 => entity_id(entity).map_or(0, |id| id.wrapping_add(1)),
        body => body,
    };
    let mut comp4 = game.comp4.load(Ordering::Relaxed);
    if entity != 0 && !comp4_of(probe, comp4, body) {
        comp4 = 0;
        game.comp4.store(0, Ordering::Relaxed);
        if let Ok(mut wanted) = game.scan.lock() {
            if *wanted != body {
                *wanted = body;
                game.scan_wake.notify_one();
            }
        }
    }
    let Ok(mut state) = game.state.lock() else {
        return;
    };
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
    let mut sample = FrameSample {
        index,
        frame: ring_frame(comp59),
        comp59,
        entity,
        entity_id: body,
        comp4,
        position,
        velocity: state.velocity,
        view: mem::read_i16_pair(comp59 + VIEW_YAW).unwrap_or([0, 0]),
        buttons: mem::read_u32(comp59 + SAMPLE_BUTTONS).unwrap_or(0),
        forward: read_i8(comp59 + SAMPLE_FORWARD),
        right: read_i8(comp59 + SAMPLE_RIGHT),
        action: read_i8(comp59 + SAMPLE_ACTION) as u8,
        reads: Vec::new(),
    };
    if record && !state.reads.is_empty() {
        let names = |name: &str| match name {
            "c59" => Some(comp59),
            "ent" => (entity != 0).then_some(entity),
            "c4" => (comp4 != 0).then_some(comp4),
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
