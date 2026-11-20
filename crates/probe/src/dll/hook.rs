//! Inline hooks: a jump at the hooked address to a stub that saves every
//! register, calls [`dispatch`] with them, restores them (with any change the
//! handler made) and runs the displaced instructions in a trampoline.
//!
//! An entry hook can also catch the return: [`dispatch`] swaps the return
//! address for a gadget that leads to the return stub, and keeps the real one
//! on a per-thread stack. The gadget sits in the image's padding when the
//! image is writable, so the client's caller checks (return address inside
//! the code range, a CALL before it) still pass.

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, FlowControl, IcedError,
    Instruction, InstructionBlock, code_asm::CodeAssembler,
};
use serde_json::{Value, json};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::{
                CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext, RtlLookupFunctionEntry,
                RtlVirtualUnwind, UNW_FLAG_NHANDLER,
            },
            ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
        },
        Threading::{
            GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
            THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

use super::{Probe, game, mem, probe};
use crate::{expr::Expr, ring::Event};

/// The registers a stub saves, in memory order.
#[repr(C)]
pub struct Regs {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rflags: u64,
}

const REGISTERS: [&str; 17] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15", "rflags",
];

impl Regs {
    pub fn words(&self) -> [u64; 17] {
        // SAFETY: `Regs` is 17 `u64`s with C layout.
        unsafe { std::mem::transmute_copy(self) }
    }

    pub fn get(&self, name: &str) -> Option<u64> {
        let index = REGISTERS.iter().position(|&r| r == name)?;
        Some(self.words()[index])
    }

    pub fn set(&mut self, name: &str, value: u64) -> bool {
        let Some(index) = REGISTERS.iter().position(|&r| r == name) else {
            return false;
        };
        // SAFETY: `Regs` is 17 `u64`s with C layout; `index` < 17.
        unsafe { (self as *mut Regs as *mut u64).add(index).write(value) };
        true
    }
}

/// What a hook does at each hit.
pub enum Action {
    /// The frame sampler (the input sample site).
    Frame,
    /// Log the registers and reads; `ret` also logs the return.
    Record {
        reads: Vec<Expr>,
        ret_reads: Vec<Expr>,
        stack: usize,
        ret: bool,
        /// Register writes at each hit (`rcx` = value), e.g. to change an argument.
        sets: Vec<(String, u64)>,
    },
}

/// A hook. The probe's registry owns every hook it ever installed for the
/// rest of the process: the stub holds a pointer to it, and a thread may still
/// run a removed hook's stub.
pub struct Hook {
    pub id: u32,
    pub address: u64,
    pub name: String,
    pub entry: bool,
    /// The jump is in place.
    pub installed: AtomicBool,
    /// Hits are handled (off once `max_hits` is reached).
    pub enabled: AtomicBool,
    pub hits: AtomicU64,
    pub returns: AtomicU64,
    pub max_hits: u64,
    pub action: Action,
    original: Vec<u8>,
    stub: u64,
    trampoline: u64,
}

impl Hook {
    pub fn json(&self) -> Value {
        let (reads, ret, stack) = match &self.action {
            Action::Frame => (Vec::new(), false, 0),
            Action::Record {
                reads, ret, stack, ..
            } => (reads.iter().map(|r| r.text.clone()).collect(), *ret, *stack),
        };
        json!({
            "id": self.id, "address": self.address, "name": self.name, "entry": self.entry,
            "enabled": self.enabled.load(Ordering::Relaxed),
            "hits": self.hits.load(Ordering::Relaxed),
            "returns": self.returns.load(Ordering::Relaxed),
            "max_hits": self.max_hits, "reads": reads, "ret": ret, "stack": stack,
            "frame": matches!(self.action, Action::Frame),
            "stub": self.stub, "trampoline": self.trampoline,
        })
    }
}

/// The in-image gadgets the hooks and calls share (the return stub itself is
/// only reached through `return_gadget`).
pub struct Shared {
    /// The fake return address for a caught return (after a `call rax`).
    pub return_gadget: u64,
    /// `call rax; jmp r12` for calling client functions from inside the image.
    pub call_gadget: Option<u64>,
}

thread_local! {
    static IN_HANDLER: Cell<bool> = const { Cell::new(false) };
    static RETURNS: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

/// A call whose return a hook waits for.
struct Pending {
    /// A hook of the registry, which never drops one.
    hook: *const Hook,
    return_address: u64,
    /// `rsp` at the function's entry (the slot of the return address).
    rsp: u64,
}

/// Called by every hook stub.
extern "C" fn dispatch(regs: *mut Regs, xsave: *const u8, hook: *const Hook) {
    // SAFETY: the stub passes its saved registers, its XSAVE area and the
    // hook it was built for, which the registry keeps for the process.
    let (regs, hook) = unsafe { (&mut *regs, &*hook) };
    if !hook.enabled.load(Ordering::Relaxed)
        || !hook.installed.load(Ordering::Relaxed)
        || IN_HANDLER.with(Cell::get)
    {
        return;
    }
    IN_HANDLER.with(|flag| flag.set(true));
    let _ = catch_unwind(AssertUnwindSafe(|| hit(regs, xsave, hook)));
    IN_HANDLER.with(|flag| flag.set(false));
}

/// Called by the return stub; returns where the call really returns to.
extern "C" fn dispatch_return(regs: *mut Regs, xsave: *const u8) -> u64 {
    // SAFETY: the return stub passes its saved registers and its XSAVE area,
    // whose legacy part holds xmm0 at +160.
    let (regs, xmm0) = unsafe { (&mut *regs, (xsave.add(160) as *const u64).read_unaligned()) };
    let pending = RETURNS.with(|returns| {
        let mut returns = returns.borrow_mut();
        // Calls whose return never came (an exception or a longjmp went past
        // them) sit above this one.
        while let Some(top) = returns.last() {
            if top.rsp < regs.rsp {
                returns.pop();
            } else {
                break;
            }
        }
        returns.pop()
    });
    let Some(pending) = pending else {
        // Cannot happen: a return through the gadget always has an entry.
        return 0;
    };
    if !IN_HANDLER.with(Cell::get) {
        IN_HANDLER.with(|flag| flag.set(true));
        let _ = catch_unwind(AssertUnwindSafe(|| returned(regs, xmm0, &pending)));
        IN_HANDLER.with(|flag| flag.set(false));
    }
    pending.return_address
}

fn names<'a>(regs: &'a Regs, probe: &'a Probe) -> impl Fn(&str) -> Option<u64> + 'a {
    move |name| match name {
        "base" => Some(probe.base),
        _ => regs.get(name).or_else(|| game::base_name(probe, name)),
    }
}

fn hit(regs: &mut Regs, xsave: *const u8, hook: &Hook) {
    let probe = probe();
    let count = hook.hits.fetch_add(1, Ordering::Relaxed) + 1;
    match &hook.action {
        Action::Frame => game::on_frame(probe, regs),
        Action::Record {
            reads,
            stack,
            ret,
            sets,
            ..
        } => {
            if hook.max_hits > 0 && count > hook.max_hits {
                hook.enabled.store(false, Ordering::Relaxed);
                return;
            }
            let caller = hook.entry.then(|| mem::read_u64(regs.rsp)).flatten();
            let values = {
                let name = names(regs, probe);
                reads.iter().map(|r| r.eval(&name, &mem::read)).collect()
            };
            let mut xmm = [0u64; 4];
            for (index, slot) in xmm.iter_mut().enumerate() {
                // SAFETY: the XSAVE legacy area holds xmm0 at +160, 16 bytes each.
                *slot = unsafe { (xsave.add(160 + 16 * index) as *const u64).read_unaligned() };
            }
            let stack = if *stack > 0 {
                unwind(regs, hook.address, *stack)
            } else {
                Vec::new()
            };
            probe.record(Event::Hit {
                hook: hook.id,
                address: hook.address,
                registers: Box::new(regs.words()),
                caller,
                xmm,
                reads: values,
                stack,
            });
            for (register, value) in sets {
                regs.set(register, *value);
            }
            if *ret && hook.entry {
                // Built when the hook was added (`server`), so no lock here.
                if let (Some(Ok(shared)), Some(return_address)) = (probe.shared.get(), caller) {
                    if mem::write(regs.rsp, &shared.return_gadget.to_le_bytes()) {
                        RETURNS.with(|returns| {
                            returns.borrow_mut().push(Pending {
                                hook: hook as *const Hook,
                                return_address,
                                rsp: regs.rsp,
                            })
                        });
                    }
                }
            }
        }
    }
}

fn returned(regs: &mut Regs, xmm0: u64, pending: &Pending) {
    let probe = probe();
    // SAFETY: the registry keeps every hook for the rest of the process.
    let hook = unsafe { &*pending.hook };
    hook.returns.fetch_add(1, Ordering::Relaxed);
    let Action::Record { ret_reads, .. } = &hook.action else {
        return;
    };
    let name = names(regs, probe);
    let reads = ret_reads
        .iter()
        .map(|r| r.eval(&name, &mem::read))
        .collect();
    probe.record(Event::Return {
        hook: hook.id,
        rax: regs.rax,
        xmm0,
        caller: pending.return_address,
        reads,
    });
}

/// The callers of a hit, unwound with the image's unwind data from the
/// hooked address and the saved registers.
fn unwind(regs: &Regs, rip: u64, frames: usize) -> Vec<u64> {
    // SAFETY: an all-zero CONTEXT is valid.
    let mut context: CONTEXT = unsafe { std::mem::zeroed() };
    context.Rip = rip;
    context.Rsp = regs.rsp;
    context.Rax = regs.rax;
    context.Rcx = regs.rcx;
    context.Rdx = regs.rdx;
    context.Rbx = regs.rbx;
    context.Rbp = regs.rbp;
    context.Rsi = regs.rsi;
    context.Rdi = regs.rdi;
    context.R8 = regs.r8;
    context.R9 = regs.r9;
    context.R10 = regs.r10;
    context.R11 = regs.r11;
    context.R12 = regs.r12;
    context.R13 = regs.r13;
    context.R14 = regs.r14;
    context.R15 = regs.r15;
    let mut out = Vec::new();
    for _ in 0..frames {
        let mut image_base = 0u64;
        // SAFETY: looks up the unwind data of a code address; null when none.
        let entry =
            unsafe { RtlLookupFunctionEntry(context.Rip, &mut image_base, std::ptr::null_mut()) };
        if entry.is_null() {
            // A leaf: the return address is at rsp.
            let Some(next) = mem::read_u64(context.Rsp) else {
                break;
            };
            context.Rip = next;
            context.Rsp += 8;
        } else {
            let mut handler_data = std::ptr::null_mut::<c_void>();
            let mut establisher = 0u64;
            // The unwinder reads the stack: check the frame is readable first.
            if mem::read_u64(context.Rsp).is_none() {
                break;
            }
            // SAFETY: unwinds one frame of a context built from the hit's
            // registers, with the function entry the lookup returned.
            unsafe {
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    image_base,
                    context.Rip,
                    entry,
                    &mut context,
                    &mut handler_data,
                    &mut establisher,
                    std::ptr::null_mut(),
                )
            };
        }
        if context.Rip == 0 {
            break;
        }
        out.push(context.Rip);
    }
    out
}

/// The instructions a 5-byte jump at `address` displaces.
fn displaced(address: u64) -> Result<(Vec<Instruction>, usize), String> {
    let bytes = mem::read_partial(address, 32);
    if bytes.len() < 16 {
        return Err(format!("{address:#X} is not readable"));
    }
    if bytes[0] == 0xCC {
        return Err(format!(
            "{address:#X} holds an int3 (a debugger breakpoint?)"
        ));
    }
    let mut decoder = Decoder::with_ip(64, &bytes, address, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    let mut length = 0;
    while length < 5 {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(format!("cannot decode {:#X}", instruction.ip()));
        }
        let ends = matches!(
            instruction.flow_control(),
            FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch
        );
        length += instruction.len();
        instructions.push(instruction);
        if ends && length < 5 {
            return Err(format!(
                "{address:#X}: the code ends after {length} bytes, too short for a jump"
            ));
        }
    }
    Ok((instructions, length))
}

/// The stub: save everything, call `handler(regs, xsave, hook)`, restore,
/// jump to `then`.
fn assemble_stub(at: u64, handler: u64, hook: u64, then: u64) -> Result<Vec<u8>, IcedError> {
    use iced_x86::code_asm::*;
    let mut a = CodeAssembler::new(64)?;
    push_registers(&mut a)?;
    call_with_registers(&mut a, handler, Some(hook))?;
    restore_registers(&mut a)?;
    a.jmp(then)?;
    a.assemble(at)
}

/// The return stub: a slot for the real return address, save everything,
/// ask [`dispatch_return`] for the address, restore, return to it.
fn assemble_return_stub(at: u64) -> Result<Vec<u8>, IcedError> {
    use iced_x86::code_asm::*;
    let mut a = CodeAssembler::new(64)?;
    a.sub(rsp, 8)?;
    push_registers(&mut a)?;
    call_with_registers(&mut a, dispatch_return as *const () as u64, None)?;
    restore_registers(&mut a)?;
    a.ret()?;
    a.assemble(at)
}

/// pushfq and the 16 registers so that `rsp` points at a [`Regs`] whose
/// `rsp` field holds the value before the pushes.
fn push_registers(a: &mut CodeAssembler) -> Result<(), IcedError> {
    use iced_x86::code_asm::*;
    a.pushfq()?;
    a.cld()?;
    for register in [r15, r14, r13, r12, r11, r10, r9, r8, rdi, rsi, rbp] {
        a.push(register)?;
    }
    a.push(rax)?; // the rsp slot
    for register in [rbx, rdx, rcx, rax] {
        a.push(register)?;
    }
    a.lea(rax, qword_ptr(rsp + 0x88))?;
    a.mov(qword_ptr(rsp + 0x20), rax)?;
    Ok(())
}

/// With `rsp` at the [`Regs`]: XSAVE the vector state, call
/// `handler(regs, xsave, extra)`, store a returned `rax` in the slot above
/// the registers when there is no `extra` (the return stub), XRSTOR.
fn call_with_registers(
    a: &mut CodeAssembler,
    handler: u64,
    extra: Option<u64>,
) -> Result<(), IcedError> {
    use iced_x86::code_asm::*;
    a.mov(rbp, rsp)?;
    a.and(rsp, -64)?;
    a.sub(rsp, 0x420)?;
    // XRSTOR needs the XSAVE header's reserved bytes zero; XSAVE writes only
    // its first word.
    a.xor(eax, eax)?;
    for index in 0..8 {
        a.mov(qword_ptr(rsp + (0x20 + 512 + index * 8)), rax)?;
    }
    a.mov(eax, 7)?;
    a.xor(edx, edx)?;
    a.xsave64(ptr(rsp + 0x20))?;
    a.mov(rcx, rbp)?;
    a.lea(rdx, ptr(rsp + 0x20))?;
    if let Some(extra) = extra {
        a.mov(r8, extra)?;
    }
    a.mov(rax, handler)?;
    a.call(rax)?;
    if extra.is_none() {
        a.mov(qword_ptr(rbp + 0x88), rax)?;
    }
    a.mov(eax, 7)?;
    a.xor(edx, edx)?;
    a.xrstor64(ptr(rsp + 0x20))?;
    a.mov(rsp, rbp)?;
    Ok(())
}

fn restore_registers(a: &mut CodeAssembler) -> Result<(), IcedError> {
    use iced_x86::code_asm::*;
    for register in [rax, rcx, rdx, rbx] {
        a.pop(register)?;
    }
    a.lea(rsp, qword_ptr(rsp + 8))?; // the rsp slot; lea keeps the flags
    for register in [rbp, rsi, rdi, r8, r9, r10, r11, r12, r13, r14, r15] {
        a.pop(register)?;
    }
    a.popfq()?;
    Ok(())
}

/// Build the shared code: the return stub, and the gadgets in the image's
/// padding when its code is writable.
pub fn shared(arena: &mut mem::Arena, text: (u64, u64)) -> Result<Shared, String> {
    let return_stub = arena.alloc(512)?;
    let code = assemble_return_stub(return_stub).map_err(|e| e.to_string())?;
    mem::put_code(return_stub, &code);
    // `call rax` (FF D0) then `jmp return_stub` (E9 rel32): the fake return
    // address is after the call. Then `call rax; jmp r12` for calls.
    let cave = find_cave(text, 16);
    let in_image = cave.and_then(|cave| {
        let mut code = vec![0xFF, 0xD0, 0xE9];
        let rel = return_stub as i64 - (cave + 7) as i64;
        let rel = i32::try_from(rel).ok()?;
        code.extend_from_slice(&rel.to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xD0, 0x41, 0xFF, 0xE4]);
        mem::patch_code(cave, &code).ok()?;
        Some(cave)
    });
    Ok(match in_image {
        Some(cave) => Shared {
            return_gadget: cave + 2,
            call_gadget: Some(cave + 7),
        },
        None => {
            // Outside the image: callers that check their return address
            // (the protector's decryptor checks) must not be caught.
            let gadget = arena.alloc(16)?;
            let mut code = vec![0xFF, 0xD0, 0xE9];
            let rel = i32::try_from(return_stub as i64 - (gadget + 7) as i64)
                .map_err(|_| "the return stub is too far from its gadget")?;
            code.extend_from_slice(&rel.to_le_bytes());
            mem::put_code(gadget, &code);
            Shared {
                return_gadget: gadget + 2,
                call_gadget: None,
            }
        }
    })
}

/// `size` bytes of int3 padding at a 16-byte boundary near the end of `.text`.
fn find_cave((start, end): (u64, u64), size: usize) -> Option<u64> {
    let window = 0x10_0000u64;
    let mut at = end.saturating_sub(window).max(start) & !0xF;
    let data = mem::read_partial(at, (end - at) as usize);
    let mut offset = 0usize;
    while offset + size + 16 <= data.len() {
        // Padding before the slot too, so it cannot be the tail of an instruction.
        if data[offset..offset + size + 16].iter().all(|&b| b == 0xCC) {
            at += offset as u64 + 16;
            return Some(at);
        }
        offset += 16;
    }
    None
}

/// Install a hook at `address`.
pub fn install(
    probe: &Probe,
    address: u64,
    name: String,
    entry: bool,
    action: Action,
    max_hits: u64,
) -> Result<Arc<Hook>, String> {
    let mut hooks = probe.hooks.lock().map_err(|_| "hooks lock")?;
    if hooks.iter().any(|hook| {
        hook.installed.load(Ordering::Relaxed)
            && hook.address < address + 5
            && address < hook.address + hook.original.len() as u64
    }) {
        return Err(format!("{address:#X} overlaps a hook"));
    }
    let (instructions, length) = displaced(address)?;
    let original = mem::read_partial(address, length);
    let mut arena = probe.arena.lock().map_err(|_| "arena lock")?;
    let trampoline = arena.alloc(128)?;
    let block = InstructionBlock::new(&instructions, trampoline);
    let mut code = BlockEncoder::encode(64, block, BlockEncoderOptions::NONE)
        .map_err(|e| format!("relocating {address:#X}: {e}"))?
        .code_buffer;
    let back = trampoline + code.len() as u64;
    let rel = i32::try_from((address + length as u64) as i64 - (back + 5) as i64)
        .map_err(|_| "the trampoline is too far from the image")?;
    code.push(0xE9);
    code.extend_from_slice(&rel.to_le_bytes());
    if code.len() > 128 {
        return Err("relocated code too long".into());
    }
    mem::put_code(trampoline, &code);
    let stub = arena.alloc(1024)?;
    drop(arena);
    let hook = Arc::new(Hook {
        id: probe.next_hook_id(),
        address,
        name,
        entry,
        installed: AtomicBool::new(false),
        enabled: AtomicBool::new(true),
        hits: AtomicU64::new(0),
        returns: AtomicU64::new(0),
        max_hits,
        action,
        original,
        stub,
        trampoline,
    });
    // The registry keeps the hook for the rest of the process (the stub
    // points at it), installed or not.
    hooks.push(Arc::clone(&hook));
    let code = assemble_stub(
        stub,
        dispatch as *const () as u64,
        Arc::as_ptr(&hook) as u64,
        trampoline,
    )
    .map_err(|e| e.to_string())?;
    if code.len() > 1024 {
        return Err("stub too long".into());
    }
    mem::put_code(stub, &code);
    let rel = i32::try_from(stub as i64 - (address + 5) as i64)
        .map_err(|_| "the stub is too far from the image")?;
    let mut patch = vec![0xE9];
    patch.extend_from_slice(&rel.to_le_bytes());
    patch.resize(length, 0x90);
    hook.installed.store(true, Ordering::Relaxed);
    if let Err(error) = with_threads_stopped(address, length, || mem::patch_code(address, &patch)) {
        hook.installed.store(false, Ordering::Relaxed);
        return Err(error);
    }
    Ok(hook)
}

/// Take a hook out: its original bytes go back. The stub stays (a thread
/// may be in it); a return it waits for still comes back through it.
pub fn remove(probe: &Probe, id: u32) -> Result<(), String> {
    let hooks = probe.hooks.lock().map_err(|_| "hooks lock")?;
    let hook = hooks
        .iter()
        .find(|hook| hook.id == id && hook.installed.load(Ordering::Relaxed))
        .ok_or_else(|| format!("no hook {id}"))?;
    with_threads_stopped(hook.address, hook.original.len(), || {
        mem::patch_code(hook.address, &hook.original)
    })?;
    hook.installed.store(false, Ordering::Relaxed);
    Ok(())
}

/// Run `patch` with every other thread suspended and none of them inside
/// `[address, address + length)`. Nothing here allocates while threads are
/// suspended (one of them may hold the heap lock).
fn with_threads_stopped(
    address: u64,
    length: usize,
    patch: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), String> {
    let handles = other_threads();
    let mut patch = Some(patch);
    let mut result = None;
    for _ in 0..200 {
        for &handle in &handles {
            // SAFETY: a thread handle opened with THREAD_SUSPEND_RESUME.
            unsafe { SuspendThread(handle) };
        }
        let inside = handles.iter().any(|&handle| {
            // SAFETY: an all-zero CONTEXT is valid; flags are set before use.
            let mut context: CONTEXT = unsafe { std::mem::zeroed() };
            context.ContextFlags = CONTEXT_CONTROL_AMD64;
            // SAFETY: a suspended thread's handle with THREAD_GET_CONTEXT;
            // CONTEXT is 16-byte aligned by its type.
            let ok = unsafe { GetThreadContext(handle, &mut context) } != 0;
            ok && (address..address + length as u64).contains(&context.Rip)
        });
        if !inside {
            // An OS error holds no allocation.
            result = patch.take().map(|patch| patch());
        }
        for &handle in &handles {
            // SAFETY: as above.
            unsafe { ResumeThread(handle) };
        }
        if !inside {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    for handle in handles {
        // SAFETY: handles opened by `other_threads`.
        unsafe { CloseHandle(handle) };
    }
    match result {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(format!(
            "cannot write the code at {address:#X} ({error}): the shipped Overwatch.exe maps its \
             code SEC_NO_CHANGE; hooks need the debug build"
        )),
        None => Err(format!(
            "a thread stayed inside {address:#X} while patching"
        )),
    }
}

fn other_threads() -> Vec<HANDLE> {
    let mut handles = Vec::new();
    // SAFETY: plain snapshot request; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return handles;
    }
    // SAFETY: plain process and thread id queries.
    let (pid, me) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    // SAFETY: an all-zero THREADENTRY32 is valid; dwSize is set before use.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: `entry` is a properly sized THREADENTRY32.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid && entry.th32ThreadID != me {
            // SAFETY: plain handle request; closed by the caller.
            let handle = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if !handle.is_null() {
                handles.push(handle);
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot handle is valid and owned here.
    unsafe { CloseHandle(snapshot) };
    handles
}
