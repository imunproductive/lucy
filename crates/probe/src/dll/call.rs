//! Calling a client function: up to 16 integer arguments (the first four
//! also in xmm0-xmm3, for float parameters), on the frame sampler's thread
//! or the probe's own. The call goes through the in-image gadget
//! `call rax; jmp r12` when there is one, so the client's caller checks
//! (return address in the code, a CALL before it) pass.

use std::{
    sync::{OnceLock, mpsc},
    time::Duration,
};

use iced_x86::IcedError;
use windows_sys::Win32::System::Diagnostics::Debug::RtlLookupFunctionEntry;

use super::{Probe, mem};

pub const MAX_ARGUMENTS: usize = 16;

pub struct Request {
    pub target: u64,
    pub arguments: [u64; MAX_ARGUMENTS],
    pub reply: mpsc::Sender<Result<(u64, u64), String>>,
}

/// `caller(target, arguments, via) -> rax`, xmm0's low 64 bits in
/// `arguments[0]`.
type Caller = unsafe extern "C" fn(u64, *mut u64, u64) -> u64;

static CALLER: OnceLock<Result<u64, String>> = OnceLock::new();

fn caller(probe: &Probe) -> Result<Caller, String> {
    let address = CALLER
        .get_or_init(|| {
            let mut arena = probe.arena.lock().map_err(|_| "arena lock")?;
            let at = arena.alloc(512)?;
            let code = assemble(at).map_err(|e| e.to_string())?;
            mem::put_code(at, &code);
            Ok(at)
        })
        .clone()?;
    // SAFETY: the code at `address` is the caller assembled below, with the
    // `Caller` signature.
    Ok(unsafe { std::mem::transmute::<u64, Caller>(address) })
}

fn assemble(at: u64) -> Result<Vec<u8>, IcedError> {
    use iced_x86::code_asm::*;
    let mut a = CodeAssembler::new(64)?;
    let mut back = a.create_label();
    let mut through_gadget = a.create_label();
    a.push(rbx)?;
    a.push(rbp)?;
    a.push(r12)?;
    a.mov(rbp, rsp)?;
    a.sub(rsp, 0x80)?;
    a.and(rsp, -16)?;
    a.mov(rbx, rdx)?;
    a.mov(rax, rcx)?;
    a.mov(r11, r8)?;
    for index in 4..MAX_ARGUMENTS as i32 {
        a.mov(r10, qword_ptr(rbx + 8 * index))?;
        a.mov(qword_ptr(rsp + (0x20 + 8 * (index - 4))), r10)?;
    }
    a.mov(rcx, qword_ptr(rbx))?;
    a.mov(rdx, qword_ptr(rbx + 8))?;
    a.mov(r8, qword_ptr(rbx + 16))?;
    a.mov(r9, qword_ptr(rbx + 24))?;
    a.movq(xmm0, rcx)?;
    a.movq(xmm1, rdx)?;
    a.movq(xmm2, r8)?;
    a.movq(xmm3, r9)?;
    a.cmp(r11, rax)?;
    a.jne(through_gadget)?;
    a.call(rax)?;
    a.jmp(back)?;
    // The gadget's `call rax` pushes its own return address (in the image)
    // and its `jmp r12` comes back here; r12 is callee-saved.
    a.set_label(&mut through_gadget)?;
    a.lea(r12, ptr(back))?;
    a.jmp(r11)?;
    a.set_label(&mut back)?;
    a.movq(qword_ptr(rbx), xmm0)?;
    a.mov(rsp, rbp)?;
    a.pop(r12)?;
    a.pop(rbp)?;
    a.pop(rbx)?;
    a.ret()?;
    a.assemble(at)
}

/// Refuse a target that is not a function start the image's unwind data
/// knows, unless forced.
pub fn check(probe: &Probe, target: u64, force: bool) -> Result<(), String> {
    if force {
        return Ok(());
    }
    if !(probe.base..probe.base + probe.image_size).contains(&target) {
        return Err(format!(
            "{target:#X} is outside the client image (force to call it)"
        ));
    }
    let mut image_base = 0u64;
    // SAFETY: looks up the unwind data of a code address; null when none.
    let entry = unsafe { RtlLookupFunctionEntry(target, &mut image_base, std::ptr::null_mut()) };
    if entry.is_null() {
        return Err(format!("{target:#X} has no unwind data (force to call it)"));
    }
    // SAFETY: a non-null entry the lookup returned.
    let begin = image_base + u64::from(unsafe { (*entry).BeginAddress });
    if begin != target {
        return Err(format!(
            "{target:#X} is inside the function at {begin:#X} (force to call it)"
        ));
    }
    Ok(())
}

/// Call now, on this thread.
pub fn call_here(
    probe: &Probe,
    target: u64,
    mut arguments: [u64; MAX_ARGUMENTS],
) -> Result<(u64, u64), String> {
    let caller = caller(probe)?;
    let via = probe
        .shared()
        .ok()
        .and_then(|shared| shared.call_gadget)
        .unwrap_or(target);
    // SAFETY: the caller sets up the Microsoft x64 call; what the target
    // does with the arguments is the requester's responsibility (`check`
    // refused anything but a function start unless forced).
    let rax = unsafe { caller(target, arguments.as_mut_ptr(), via) };
    Ok((rax, arguments[0]))
}

/// The frame sampler's turn: call and answer.
pub fn run(probe: &Probe, request: Request) {
    let result = call_here(probe, request.target, request.arguments);
    let _ = request.reply.send(result);
}

/// Queue a call for the next frame and wait for it.
pub fn on_frame(
    probe: &Probe,
    target: u64,
    arguments: [u64; MAX_ARGUMENTS],
    timeout: Duration,
) -> Result<(u64, u64), String> {
    let (reply, answer) = mpsc::channel();
    probe
        .game
        .state
        .lock()
        .map_err(|_| "state lock")?
        .calls
        .push_back(Request {
            target,
            arguments,
            reply,
        });
    answer.recv_timeout(timeout).map_err(|_| {
        "no frame ran the call in time (is the frame hook in and a match running?)".to_string()
    })?
}
