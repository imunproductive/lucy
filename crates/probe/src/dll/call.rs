//! Calling a client function: up to 16 integer arguments (the first four
//! also in xmm0-xmm3, for float parameters), on the frame sampler's thread
//! or the probe's own. The call goes through the in-image gadget
//! `call rax; jmp r12` when there is one, so the client's caller checks
//! (return address in the code, a CALL before it) pass.
//!
//! A guarded call survives a fault in the called function: a vectored
//! exception handler sees an access violation, a divide error or an illegal
//! or privileged instruction on the calling thread while the call runs, and
//! resumes the caller stub at its recovery label with the stack and the
//! callee-saved registers it had before the call. Proper unwinding (an SEH
//! handler for the stub) is not possible through the gadget: it has no unwind
//! data, and its leaf unwind would read the callee's first home slot. What
//! the faulting function held (locks, half-written state) stays as it was.

use std::{
    cell::Cell,
    ptr,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use iced_x86::{BlockEncoderOptions, IcedError};
use windows_sys::Win32::{
    Foundation::{
        EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION, EXCEPTION_IN_PAGE_ERROR,
        EXCEPTION_INT_DIVIDE_BY_ZERO, EXCEPTION_INT_OVERFLOW, EXCEPTION_PRIV_INSTRUCTION,
    },
    System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS, RtlLookupFunctionEntry,
    },
};

use super::{Probe, mem};

pub const MAX_ARGUMENTS: usize = 16;

pub struct Request {
    pub target: u64,
    pub arguments: [u64; MAX_ARGUMENTS],
    pub reply: mpsc::Sender<Result<(u64, u64), String>>,
}

/// The stub's word with the vectored handler: while `rsp` is set, a fault on
/// this thread resumes the stub at `recover` with that stack.
#[repr(C)]
#[derive(Default)]
struct Guard {
    /// The stub's stack after it saved the callee-saved registers (0: off).
    rsp: u64,
    code: u32,
    address: u64,
}

/// `caller(target, arguments, via, guard) -> rax`, xmm0's low 64 bits in
/// `arguments[0]`.
type Caller = unsafe extern "C" fn(u64, *mut u64, u64, *mut Guard) -> u64;

struct Stub {
    entry: u64,
    recover: u64,
}

static STUB: OnceLock<Result<Stub, String>> = OnceLock::new();

thread_local! {
    /// The guard of the call this thread is running (null: none).
    static GUARD: Cell<*mut Guard> = const { Cell::new(ptr::null_mut()) };
}

fn stub(probe: &Probe) -> Result<&'static Stub, String> {
    STUB.get_or_init(|| {
        let mut arena = probe.arena.lock().map_err(|_| "arena lock")?;
        let at = arena.alloc(1024)?;
        let (code, recover) = assemble(at).map_err(|e| e.to_string())?;
        mem::put_code(at, &code);
        // SAFETY: registers a handler that only acts on a thread with an armed
        // guard (see `on_exception`).
        let handler = unsafe { AddVectoredExceptionHandler(1, Some(on_exception)) };
        if handler.is_null() {
            return Err("cannot add the call guard's exception handler".into());
        }
        Ok(Stub { entry: at, recover })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// The caller and its recovery label: the callee-saved registers (xmm6-15
/// too) are saved below the return address and the stack after them goes into
/// the guard, so the recovery can restore them however far the callee got.
fn assemble(at: u64) -> Result<(Vec<u8>, u64), IcedError> {
    use iced_x86::code_asm::*;
    const SAVED: [AsmRegister64; 8] = [rbx, rbp, rsi, rdi, r12, r13, r14, r15];
    const XMM: [AsmRegisterXmm; 10] = [
        xmm6, xmm7, xmm8, xmm9, xmm10, xmm11, xmm12, xmm13, xmm14, xmm15,
    ];
    let mut a = CodeAssembler::new(64)?;
    let mut back = a.create_label();
    let mut through_gadget = a.create_label();
    let mut recover = a.create_label();
    let mut restore = a.create_label();
    // Entry rsp is 8 mod 16; 8 pushes keep it there, 0xA8 makes it 0 mod 16.
    for register in SAVED {
        a.push(register)?;
    }
    a.sub(rsp, 0xA8)?;
    for (index, register) in XMM.into_iter().enumerate() {
        a.movdqu(xmmword_ptr(rsp + 16 * index as i32), register)?;
    }
    a.mov(rbp, rsp)?;
    a.mov(qword_ptr(r9), rsp)?;
    a.sub(rsp, 0x80)?;
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
    a.jmp(restore)?;
    // The handler resumes here with rsp = the guard's.
    a.set_label(&mut recover)?;
    a.xor(eax, eax)?;
    a.set_label(&mut restore)?;
    for (index, register) in XMM.into_iter().enumerate() {
        a.movdqu(register, xmmword_ptr(rsp + 16 * index as i32))?;
    }
    a.add(rsp, 0xA8)?;
    for register in SAVED.into_iter().rev() {
        a.pop(register)?;
    }
    a.ret()?;
    let result = a.assemble_options(at, BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS)?;
    let recover = result.label_ip(&recover)?;
    Ok((result.inner.code_buffer, recover))
}

/// Faults that end a guarded call.
const FATAL: [i32; 6] = [
    EXCEPTION_ACCESS_VIOLATION,
    EXCEPTION_INT_DIVIDE_BY_ZERO,
    EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_PRIV_INSTRUCTION,
    EXCEPTION_IN_PAGE_ERROR,
    EXCEPTION_INT_OVERFLOW,
];

unsafe extern "system" fn on_exception(pointers: *mut EXCEPTION_POINTERS) -> i32 {
    let guard = GUARD.with(Cell::get);
    // SAFETY: the guard is the running call's (`call_here` keeps it alive
    // until the call returns) and the pointers are the system's for this
    // exception.
    unsafe {
        if guard.is_null() || (*guard).rsp == 0 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let record = &*(*pointers).ExceptionRecord;
        if !FATAL.contains(&record.ExceptionCode) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let Some(Ok(stub)) = STUB.get() else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let context = &mut *(*pointers).ContextRecord;
        (*guard).code = record.ExceptionCode as u32;
        (*guard).address = record.ExceptionAddress as u64;
        context.Rsp = (*guard).rsp;
        context.Rip = stub.recover;
        (*guard).rsp = 0;
    }
    EXCEPTION_CONTINUE_EXECUTION
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

/// The in-image gadget calls go through, if the image is writable.
pub fn gadget(probe: &Probe) -> Option<u64> {
    probe.shared().ok().and_then(|shared| shared.call_gadget)
}

/// Call now, on this thread, guarded.
pub fn call_here(
    probe: &Probe,
    target: u64,
    mut arguments: [u64; MAX_ARGUMENTS],
) -> Result<(u64, u64), String> {
    let stub = stub(probe)?;
    let via = gadget(probe).unwrap_or(target);
    let mut guard = Guard::default();
    let outer = GUARD.with(|slot| slot.replace(&mut guard));
    // SAFETY: the stub sets up the Microsoft x64 call; what the target
    // does with the arguments is the requester's responsibility (`check`
    // refused anything but a function start unless forced). The guard lives
    // until the call is back.
    let rax = unsafe {
        let caller: Caller = std::mem::transmute::<u64, Caller>(stub.entry);
        caller(target, arguments.as_mut_ptr(), via, &mut guard)
    };
    GUARD.with(|slot| slot.set(outer));
    if guard.code != 0 {
        return Err(format!(
            "the call of {target:#X} faulted ({:#010X} at {:#X}); the probe resumed after it",
            guard.code, guard.address
        ));
    }
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
