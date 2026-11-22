//! The local socket: one thread per connection, one JSON request at a time,
//! each answered with `{"id", "ok": true, "result"}` or `{"id", "ok": false,
//! "error"}`.

use std::{
    io::BufReader,
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use iced_x86::{Decoder, DecoderOptions, Formatter, IntelFormatter};
use serde_json::{Map, Value, json};
use windows_sys::Win32::System::{
    Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc},
    Threading::GetCurrentProcessId,
};

use super::{Probe, call, game, hook, mem};
use crate::{
    expr::Expr,
    input::Step,
    read_frame,
    ring::{Event, Filter, FrameSample},
    write_frame,
};

pub fn serve(probe: &'static Probe) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, probe.port)) {
        Ok(listener) => listener,
        Err(error) => {
            probe.log(&format!(
                "cannot listen on 127.0.0.1:{}: {error}",
                probe.port
            ));
            return;
        }
    };
    probe.log(&format!("listening on 127.0.0.1:{}", probe.port));
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        let _ = thread::Builder::new()
            .name("owprobe-client".into())
            .spawn(move || connection(probe, stream));
    }
}

fn connection(probe: &'static Probe, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    while let Ok(Some(body)) = read_frame(&mut reader) {
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let answer = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle(probe, &request)
        })) {
            Ok(Ok(result)) => json!({"id": id, "ok": true, "result": result}),
            Ok(Err(error)) => json!({"id": id, "ok": false, "error": error}),
            Err(_) => json!({"id": id, "ok": false, "error": "the probe panicked"}),
        };
        let text = serde_json::to_vec(&answer).unwrap_or_default();
        if write_frame(&mut writer, &text).is_err() {
            break;
        }
    }
}

/// A number from JSON: an integer, or a string in hex (`0x…`) or decimal.
fn number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().map(|n| n as u64)),
        Value::String(text) => {
            let text = text.trim();
            match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => text.parse::<i64>().ok().map(|n| n as u64),
            }
        }
        _ => None,
    }
}

fn arg(request: &Value, key: &str) -> Result<u64, String> {
    request
        .get(key)
        .and_then(number)
        .ok_or_else(|| format!("{key}: a number is needed"))
}

fn arg_or(request: &Value, key: &str, default: u64) -> Result<u64, String> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => number(value).ok_or_else(|| format!("{key}: not a number")),
    }
}

fn strings(request: &Value, key: &str) -> Vec<String> {
    request
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn exprs(request: &Value, key: &str) -> Result<Vec<Expr>, String> {
    strings(request, key)
        .iter()
        .map(|text| Expr::parse(text))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if text.len() % 2 != 0 {
        return Err("odd number of hex digits".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// `48 8B ?? 05` → bytes with wildcards.
fn pattern(text: &str) -> Result<Vec<Option<u8>>, String> {
    text.split_whitespace()
        .map(|word| match word {
            "?" | "??" => Ok(None),
            byte => u8::from_str_radix(byte, 16)
                .map(Some)
                .map_err(|e| format!("{byte}: {e}")),
        })
        .collect()
}

fn handle(probe: &'static Probe, request: &Value) -> Result<Value, String> {
    let command = request["cmd"].as_str().ok_or("no cmd")?;
    match command {
        "info" => Ok(info(probe)),
        "modules" => Ok(mem::modules()
            .into_iter()
            .map(|(path, base, size)| json!({"path": path, "base": base, "size": size}))
            .collect()),
        "regions" => {
            let start = arg_or(request, "start", 0x10000)?;
            let end = arg_or(request, "end", 0x7FFF_FFFF_0000)?;
            let committed = request["committed"].as_bool().unwrap_or(true);
            Ok(mem::regions(start, end)
                .into_iter()
                .filter(|region| !committed || region.committed())
                .map(|region| {
                    json!({
                        "base": region.base, "size": region.size, "allocation": region.allocation,
                        "state": region.state_name(), "type": region.kind_name(),
                        "protect": region.protect,
                    })
                })
                .collect())
        }
        "read" => {
            let address = arg(request, "addr")?;
            let length = arg(request, "len")?.min(16 << 20) as usize;
            let bytes = if request["partial"].as_bool().unwrap_or(false) {
                mem::read_partial(address, length)
            } else {
                let mut bytes = vec![0u8; length];
                if !mem::read(address, &mut bytes) {
                    return Err(format!("{address:#X}+{length:#X} is not readable"));
                }
                bytes
            };
            Ok(json!({"hex": hex(&bytes)}))
        }
        "write" => {
            let address = arg(request, "addr")?;
            let bytes = unhex(request["hex"].as_str().ok_or("hex: needed")?)?;
            let code = (probe.text.0..probe.text.1).contains(&address);
            if code {
                mem::patch_code(address, &bytes)
                    .map_err(|e| format!("cannot write the code at {address:#X}: {e}"))?;
            } else if !mem::write(address, &bytes) {
                return Err(format!("{address:#X} is not writable"));
            }
            Ok(json!({"written": bytes.len()}))
        }
        "eval" => {
            let names = |name: &str| match name {
                "base" => Some(probe.base),
                _ => game::base_name(probe, name),
            };
            Ok(exprs(request, "exprs")?
                .iter()
                .map(|expr| expr.eval(&names, &mem::read))
                .collect())
        }
        "alloc" => {
            let size = arg(request, "size")?.clamp(1, 64 << 20) as usize;
            // SAFETY: reserves and commits fresh read-write memory.
            let at = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    size,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            if at.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(json!(at as u64))
        }
        "scan" => {
            let max = arg_or(request, "max", 1000)? as usize;
            if let Some(value) = request.get("value") {
                let value = number(value).ok_or("value: a number is needed")?;
                return Ok(json!(mem::scan_u64(&mem::heap_regions(), value, max)));
            }
            let bytes = pattern(
                request["pattern"]
                    .as_str()
                    .ok_or("pattern or value needed")?,
            )?;
            if bytes.is_empty() {
                return Err("empty pattern".into());
            }
            let start = arg_or(request, "start", probe.text.0)?;
            let end = arg_or(request, "end", probe.text.1)?;
            Ok(json!(mem::scan_pattern(start, end, &bytes, max)))
        }
        "components" => {
            let vtable = arg(request, "vtable")?;
            let max = arg_or(request, "max", 1 << 16)? as usize;
            Ok(game::components(probe, vtable, max)
                .into_iter()
                .map(|(component, entity, id)| json!({"component": component, "entity": entity, "id": id}))
                .collect())
        }
        "component" => {
            let id = arg(request, "id")? as u32;
            let kind = u8::try_from(arg(request, "kind")?).map_err(|_| "kind: 0-255")?;
            let player = probe.game.entity.load(Ordering::Relaxed);
            if player == 0 {
                return Err("no player entity yet (the frame sampler has not run)".into());
            }
            let entity = game::entity_by_id(probe, player, id);
            let component = entity.and_then(|entity| game::entity_component(probe, entity, kind));
            Ok(json!({"entity": entity, "component": component}))
        }
        "owner" => {
            let component = arg(request, "component")?;
            let entity = game::owner(probe, component);
            Ok(json!({"entity": entity, "id": entity.and_then(game::entity_id)}))
        }
        "disasm" => {
            let address = arg(request, "addr")?;
            let count = arg_or(request, "count", 16)?.min(500) as usize;
            let bytes = mem::read_partial(address, count * 15);
            let mut decoder = Decoder::with_ip(64, &bytes, address, DecoderOptions::NONE);
            let mut formatter = IntelFormatter::new();
            let mut lines = Vec::new();
            while decoder.can_decode() && lines.len() < count {
                let instruction = decoder.decode();
                let mut text = String::new();
                formatter.format(&instruction, &mut text);
                let start = (instruction.ip() - address) as usize;
                lines.push(json!({
                    "addr": instruction.ip(),
                    "bytes": hex(&bytes[start..start + instruction.len()]),
                    "text": text,
                }));
            }
            Ok(Value::Array(lines))
        }
        "hook" => add_hook(probe, request),
        "unhook" => {
            hook::remove(probe, arg(request, "id")? as u32)?;
            Ok(Value::Null)
        }
        "hooks" => {
            let hooks = probe.hooks.lock().map_err(|_| "hooks lock")?;
            Ok(hooks
                .iter()
                .filter(|hook| hook.installed.load(Ordering::Relaxed))
                .map(|hook| hook.json())
                .collect())
        }
        "fetch" => fetch(probe, request),
        "note" => {
            let text = request["text"].as_str().unwrap_or("").to_owned();
            probe.record(Event::Text(text));
            Ok(json!(probe.ring.lock().map_err(|_| "ring lock")?.last()))
        }
        "sampler" => {
            let hook = game::ensure_frame_hook(probe)?;
            let mut state = probe.game.state.lock().map_err(|_| "state lock")?;
            if let Some(body) = request.get("body") {
                let body = number(body).ok_or("body: an entity id (0: the possessed one)")?;
                probe.game.body.store(body as u32, Ordering::Relaxed);
            }
            if let Some(every) = request.get("every") {
                state.every = number(every).ok_or("every: a number")?;
            }
            if request.get("reads").is_some() {
                state.reads = exprs(request, "reads")?;
            }
            if let Some(camera) = request.get("camera") {
                state.camera = camera.as_bool().ok_or("camera: true or false")?;
            }
            if request.get("scripts").is_some() {
                state.watches = strings(request, "scripts")
                    .iter()
                    .map(|text| game::Watch::parse(text))
                    .collect::<Result<_, _>>()?;
            }
            Ok(json!({
                "hook": hook, "every": state.every, "camera": state.camera,
                "reads": state.reads.iter().map(|r| r.text.clone()).collect::<Vec<_>>(),
                "scripts": state.watches.iter().map(game::Watch::text).collect::<Vec<_>>(),
                "body": probe.game.body.load(Ordering::Relaxed),
            }))
        }
        "frame" => {
            let state = probe.game.state.lock().map_err(|_| "state lock")?;
            Ok(json!({
                "frames": probe.game.frames.load(Ordering::Relaxed),
                "last": state.last.as_ref().map(FrameSample::json),
            }))
        }
        "input" => {
            game::ensure_frame_hook(probe)?;
            let steps: Vec<Step> = request
                .get("steps")
                .and_then(Value::as_array)
                .map(|steps| steps.iter().map(Step::from_json).collect())
                .transpose()?
                .unwrap_or_default();
            let mut state = probe.game.state.lock().map_err(|_| "state lock")?;
            if request["clear"].as_bool().unwrap_or(false) {
                state.script.clear();
            }
            state.script.push(steps);
            Ok(json!({"pending": state.script.pending(), "samples": state.script.samples}))
        }
        "input_state" => {
            let state = probe.game.state.lock().map_err(|_| "state lock")?;
            Ok(json!({
                "pending": state.script.pending(),
                "active": state.script.active(),
                "samples": state.script.samples,
            }))
        }
        "call" => {
            let target = arg(request, "addr")?;
            call::check(probe, target, request["force"].as_bool().unwrap_or(false))?;
            let mut arguments = [0u64; call::MAX_ARGUMENTS];
            let given = request
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if given.len() > call::MAX_ARGUMENTS {
                return Err(format!("at most {} arguments", call::MAX_ARGUMENTS));
            }
            for (slot, value) in arguments.iter_mut().zip(&given) {
                *slot = number(value).ok_or("args: numbers (floats as their bits)")?;
            }
            let (rax, xmm0) = match request["thread"].as_str().unwrap_or("frame") {
                "frame" => {
                    game::ensure_frame_hook(probe)?;
                    let timeout = Duration::from_millis(arg_or(request, "timeout_ms", 5000)?);
                    call::on_frame(probe, target, arguments, timeout)?
                }
                "probe" => call::call_here(probe, target, arguments)?,
                other => return Err(format!("thread: frame or probe, not {other}")),
            };
            Ok(
                json!({"rax": rax, "xmm0": xmm0, "xmm0_f32": f64::from(f32::from_bits(xmm0 as u32))}),
            )
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

fn info(probe: &Probe) -> Value {
    let game = &probe.game;
    // SAFETY: a plain process id query.
    let pid = unsafe { GetCurrentProcessId() };
    let code_writable = mem::query(probe.text.0)
        .map(|region| region.Type == windows_sys::Win32::System::Memory::MEM_IMAGE);
    json!({
        "pid": pid,
        "exe": probe.exe,
        "base": probe.base,
        "image_size": probe.image_size,
        "text": [probe.text.0, probe.text.1],
        "code_is_image": code_writable,
        "port": probe.port,
        "uptime": probe.started.elapsed().as_secs_f64(),
        "frames": game.frames.load(Ordering::Relaxed),
        "comp59": game.comp59.load(Ordering::Relaxed),
        "entity": game.entity.load(Ordering::Relaxed),
        "entity_id": game::entity_id(game.entity.load(Ordering::Relaxed)),
        "comp4": game.comp4.load(Ordering::Relaxed),
        "comp31": game.comp31.load(Ordering::Relaxed),
        "body": game.state.lock().ok().and_then(|state| state.last.as_ref().map(|s| s.body_id)),
        "frame_hook": game.frame_hook.lock().ok().and_then(|hook| *hook),
        "records": probe.ring.lock().map(|ring| ring.last()).unwrap_or(0),
        "lucy_base": crate::LUCY_BASE,
    })
}

fn add_hook(probe: &'static Probe, request: &Value) -> Result<Value, String> {
    let address = arg(request, "addr")?;
    let entry = request["entry"].as_bool().unwrap_or(true);
    let ret = request["ret"].as_bool().unwrap_or(false);
    if ret {
        if !entry {
            return Err("ret needs an entry hook".into());
        }
        probe.shared()?;
    }
    let mut sets = Vec::new();
    if let Some(Value::Object(map)) = request.get("set") {
        for (register, value) in map {
            sets.push((
                register.to_lowercase(),
                number(value).ok_or("set: register values are numbers")?,
            ));
        }
    }
    let action = hook::Action::Record {
        reads: exprs(request, "reads")?,
        ret_reads: exprs(request, "ret_reads")?,
        stack: arg_or(request, "stack", 0)?.min(64) as usize,
        ret,
        sets,
    };
    let name = request["name"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{address:#X}"));
    let hook = hook::install(
        probe,
        address,
        name,
        entry,
        action,
        arg_or(request, "max_hits", 0)?,
    )?;
    probe.log(&format!(
        "hook {} at {:#X} ({})",
        hook.id, hook.address, hook.name
    ));
    Ok(hook.json())
}

fn fetch(probe: &Probe, request: &Value) -> Result<Value, String> {
    let since = arg_or(request, "since", 0)?;
    let max = arg_or(request, "max", 5000)?.clamp(1, 100_000) as usize;
    let wait = Duration::from_millis(arg_or(request, "wait_ms", 0)?.min(10_000));
    let filter = Filter {
        kinds: strings(request, "kinds"),
        hooks: request
            .get("hooks")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(number).map(|id| id as u32).collect())
            .unwrap_or_default(),
    };
    let deadline = Instant::now() + wait;
    loop {
        let fetched = probe
            .ring
            .lock()
            .map_err(|_| "ring lock")?
            .fetch(since, max, &filter);
        if !fetched.records.is_empty() || Instant::now() >= deadline {
            let mut answer = Map::new();
            answer.insert("records".into(), Value::Array(fetched.records));
            answer.insert("next".into(), fetched.next.into());
            answer.insert("lost".into(), fetched.lost.into());
            return Ok(Value::Object(answer));
        }
        thread::sleep(Duration::from_millis(5));
    }
}
