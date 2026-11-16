"""A small Win32 debugger for x64 processes (ctypes, single debugger thread).

    dbg = Debugger()
    dbg.on("system_breakpoint", lambda: dbg.add_bp(dbg.main.base + rva, handler))
    dbg.create(exe, args, cwd)
    dbg.run(duration=30)

Exception policy (the game's protector relies on its own exceptions; see AGENTS.md):
- first-chance exceptions that are not ours are passed to the program (DBG_EXCEPTION_NOT_HANDLED):
  0xC0000096 hlt/wbinvd traps, 0x80000004 single steps of the protector's TF trace chain,
  0xC000001D `ud2` checks, 0xC0000005 handled access violations, foreign int3s;
- 0x406D1388 (SetThreadName) is swallowed: bink2w64 raises it without a handler under a debugger;
- second-chance exceptions are crashes ("crash" hook); they are passed on (the process dies)
  unless a hook terminates first.
Breakpoint handlers run inside the debug event while the whole process is frozen: keep them short
(the watchdog flags 2.5 s waits that take more than 10 s).
"""
import ctypes
import os
import struct
import time
import traceback
from bisect import bisect_left
from collections import Counter

from . import win32 as w
from .pe import open_pe

DR7_RW = {"x": 0, "w": 1, "rw": 3, "r": 3}
DR7_LEN = {1: 0, 2: 1, 4: 3, 8: 2}


class DebugError(Exception):
    pass


class Thread:
    __slots__ = ("tid", "handle", "start", "teb", "name", "step_va", "step_had_tf", "created")

    def __init__(self, tid, handle, start, teb):
        self.tid, self.handle, self.start, self.teb = tid, handle, start, teb
        self.name = None
        self.step_va = None      # breakpoint we are single-stepping over
        self.step_had_tf = False  # TF was set by the program before our step
        self.created = time.monotonic()


class Module:
    __slots__ = ("base", "size", "path", "name", "_pe")

    def __init__(self, base, size, path):
        self.base, self.size, self.path = base, size, path
        self.name = os.path.basename(path) if path else f"mod_{base:X}"
        self._pe = None

    @property
    def pe(self):
        if self._pe is None and self.path and os.path.isfile(self.path):
            try:
                self._pe = open_pe(self.path)
            except (OSError, ValueError):
                self._pe = False
        return self._pe or None

    def __contains__(self, va):
        return self.base <= va < self.base + self.size

    def __repr__(self):
        return f"<Module {self.name} {self.base:#x}+{self.size:#x}>"


class SwBreakpoint:
    __slots__ = ("va", "orig", "handlers", "armed", "steppers")

    def __init__(self, va, orig):
        self.va, self.orig = va, orig
        self.handlers = {}  # token -> (fn, oneshot)
        self.armed = False
        self.steppers = 0  # threads currently stepping over this address


class HwSlot:
    __slots__ = ("va", "kind", "size", "handler")

    def __init__(self, va, kind, size, handler):
        self.va, self.kind, self.size, self.handler = va, kind, size, handler


class ExceptionEvent:
    """A first- or second-chance exception (not one of our breakpoints)."""
    __slots__ = ("tid", "thread", "code", "address", "first_chance", "flags", "params", "record")

    def __init__(self, dbg, de):
        info = de.u.Exception
        rec = info.ExceptionRecord
        self.tid = de.dwThreadId
        self.thread = dbg.threads.get(self.tid)
        self.code = rec.ExceptionCode
        self.address = rec.ExceptionAddress or 0
        self.first_chance = bool(info.dwFirstChance)
        self.flags = rec.ExceptionFlags
        self.params = [rec.ExceptionInformation[i] for i in range(min(rec.NumberParameters, 15))]
        self.record = w.EXCEPTION_RECORD.from_buffer_copy(rec)

    @property
    def name(self):
        return w.exception_name(self.code)

    def describe_access(self):
        """'read 0x...' / 'write 0x...' / 'execute 0x...' for access violations."""
        if self.code in (w.STATUS_ACCESS_VIOLATION, 0xC0000006) and len(self.params) >= 2:
            kind = {0: "read", 1: "write", 8: "execute"}.get(self.params[0], str(self.params[0]))
            return f"{kind} {self.params[1]:#x}"
        return None


class Hit:
    """A breakpoint or hardware hit. Register reads come from the thread's CONTEXT; writes
    (set) are applied when the handler returns."""

    def __init__(self, dbg, thread, va, ctx, slot=None):
        self.dbg, self.thread, self.va, self.ctx, self.slot = dbg, thread, va, ctx, slot
        self.tid = thread.tid
        self.time = time.monotonic() - dbg.t0
        self.dirty = False

    def reg(self, name):
        return getattr(self.ctx, w.CONTEXT_FIELD[name.lower()])

    def set(self, name, value):
        setattr(self.ctx, w.CONTEXT_FIELD[name.lower()], value & 0xFFFFFFFFFFFFFFFF)
        self.dirty = True

    def __getattr__(self, name):
        if name in w.CONTEXT_FIELD:
            return getattr(self.ctx, w.CONTEXT_FIELD[name])
        raise AttributeError(name)

    def arg(self, i):
        """Integer argument i (0-based) at a function entry (Microsoft x64 convention)."""
        if i < 4:
            return getattr(self.ctx, ("Rcx", "Rdx", "R8", "R9")[i])
        return self.dbg.read_u64(self.ctx.Rsp + 0x28 + 8 * (i - 4))

    def return_address(self):
        """[rsp] (valid at a function entry)."""
        return self.dbg.read_u64(self.ctx.Rsp)

    def regs(self):
        return {n: getattr(self.ctx, w.CONTEXT_FIELD[n]) for n in w.GPR_NAMES + ("rip", "eflags")}


class Debugger:
    def __init__(self, log=None):
        self.log = log or (lambda msg: print(msg, flush=True))
        self.process = None
        self.pid = None
        self.main_tid = None
        self.threads = {}
        self.modules = {}  # base -> Module
        self.main = None
        self.bps = {}  # va -> SwBreakpoint
        self.retired = set()  # former breakpoint addresses (late hits from other threads)
        self._bp_sorted = None
        self._next_token = 1
        self.hw = [None, None, None, None]
        self.hw_fixes = 0  # debug registers found changed by the program and rewritten
        self.alive = False
        self.stop_requested = False
        self.exit_code = None
        self.system_bp_seen = False
        self.in_event = False
        self.t0 = time.monotonic()
        self.exceptions = Counter()  # (code, first_chance) -> count, exceptions passed to the program
        self.exception_sites = Counter()  # (code, address) -> count, passed exceptions except single steps
        self.stats = Counter()
        self._hooks = {}
        self._timers = []  # [due, interval or None, fn]
        self._handler_errors = Counter()
        self._event = w.DEBUG_EVENT()
        self._pending = None  # (pid, tid) of the event being handled
        self._terminating = False
        self._hw_timer = False
        # Debug registers are written only this many seconds after start (the game's protector
        # rewrites them during its startup); set_hw before then is deferred.
        self.hw_hold = 0.0
        self._hw_deferred = False
        self.exe = None
        # Return addresses of calls owdbg itself makes in the game (a handler that points a
        # thread at a game function, e.g. --actions' set-view): probes skip such calls.
        self.injected_returns = set()

    # ---------------------------------------------------------------- hooks / timers
    def on(self, event, fn):
        """Subscribe to: create_process(), system_breakpoint(), load_dll(module),
        unload_dll(module), thread_created(thread), thread_exited(thread, code),
        exception(ExceptionEvent) -> optional continue status (first chance, not ours),
        crash(ExceptionEvent) -> optional continue status (second chance), debug_string(text),
        exit(code)."""
        self._hooks.setdefault(event, []).append(fn)

    def _emit(self, event, *args):
        result = None
        for fn in self._hooks.get(event, ()):
            try:
                r = fn(*args)
            except Exception:
                self.log(f"[owdbg] hook {event} failed:\n{traceback.format_exc()}")
                continue
            if r is not None:
                result = r
        return result

    def after(self, seconds, fn):
        """Run fn() once, `seconds` from now (between debug events, process running)."""
        self._timers.append([time.monotonic() + seconds, None, fn])

    def every(self, seconds, fn, first=None):
        self._timers.append([time.monotonic() + (seconds if first is None else first), seconds, fn])

    def _run_timers(self, now):
        for t in list(self._timers):
            if t[0] <= now:
                if t[1] is None:
                    self._timers.remove(t)
                else:
                    t[0] = now + t[1]
                try:
                    t[2]()
                except Exception:
                    self.log(f"[owdbg] timer failed:\n{traceback.format_exc()}")

    @property
    def elapsed(self):
        return time.monotonic() - self.t0

    # ---------------------------------------------------------------- process start
    def create(self, exe, args=(), cwd=None, env=None, flags=0):
        """CreateProcess under the debugger (DEBUG_ONLY_THIS_PROCESS). Returns the pid."""
        exe = os.path.abspath(exe)
        cmdline = " ".join(_quote(a) for a in [exe, *args])
        si = w.STARTUPINFOW()
        si.cb = ctypes.sizeof(si)
        pi = w.PROCESS_INFORMATION()
        env_block = None
        if env is not None:
            block = "".join(f"{k}={v}\0" for k, v in env.items()) + "\0"
            env_block = ctypes.create_unicode_buffer(block, len(block))
            flags |= w.CREATE_UNICODE_ENVIRONMENT
        buf = ctypes.create_unicode_buffer(cmdline)
        if not w.CreateProcessW(exe, buf, None, None, False, w.DEBUG_ONLY_THIS_PROCESS | flags,
                                env_block, cwd, ctypes.byref(si), ctypes.byref(pi)):
            raise DebugError(f"CreateProcess({exe}) failed: {w.last_error()}")
        w.DebugSetProcessKillOnExit(True)
        self.process = pi.hProcess
        # Kill-on-exit is not enough: the game queries ProcessDebugObjectHandle and keeps a handle
        # to our debug object, so it would survive a killed Python. The job closes with us.
        self.job = w.kill_on_close_job(pi.hProcess)
        self.pid = pi.dwProcessId
        self.main_tid = pi.dwThreadId
        w.CloseHandle(pi.hThread)
        self.alive = True
        self.t0 = time.monotonic()
        self.exe = exe
        return self.pid

    # ---------------------------------------------------------------- memory
    def read(self, va, n, mask=True):
        """Bytes at va. Raises DebugError when unreadable. mask: show original bytes under our
        int3 breakpoints."""
        buf = ctypes.create_string_buffer(n)
        got = ctypes.c_size_t()
        if not w.ReadProcessMemory(self.process, va, buf, n, ctypes.byref(got)) or got.value != n:
            raise DebugError(f"cannot read {n} bytes at {va:#x}")
        data = buf.raw
        if mask and self.bps:
            data = self._mask(va, data)
        return data

    def try_read(self, va, n, mask=True):
        try:
            return self.read(va, n, mask)
        except DebugError:
            return None

    def read_partial(self, va, n, mask=True):
        """Readable prefix of [va, va+n) (page by page)."""
        out = b""
        while n > 0:
            chunk = min(n, 0x1000 - (va & 0xFFF))
            data = self.try_read(va, chunk, mask)
            if data is None:
                break
            out += data
            va += chunk
            n -= chunk
        return out

    def _mask(self, va, data):
        if self._bp_sorted is None:
            self._bp_sorted = sorted(self.bps)
        keys = self._bp_sorted
        i = bisect_left(keys, va)
        if i >= len(keys) or keys[i] >= va + len(data):
            return data
        data = bytearray(data)
        while i < len(keys) and keys[i] < va + len(data):
            bp = self.bps.get(keys[i])
            if bp is not None and bp.armed:
                data[keys[i] - va] = bp.orig
            i += 1
        return bytes(data)

    def read_u64(self, va):
        return struct.unpack("<Q", self.read(va, 8))[0]

    def read_u32(self, va):
        return struct.unpack("<I", self.read(va, 4))[0]

    def read_cstr(self, va, limit=256, wide=False):
        data = self.read_partial(va, limit * (2 if wide else 1))
        if wide:
            text = data.decode("utf-16-le", "replace")
            return text.split("\0", 1)[0]
        return data.split(b"\0", 1)[0].decode("utf-8", "replace")

    def write(self, va, data):
        got = ctypes.c_size_t()
        if not w.WriteProcessMemory(self.process, va, data, len(data), ctypes.byref(got)) or got.value != len(data):
            raise DebugError(f"cannot write {len(data)} bytes at {va:#x}: {w.last_error()}")
        w.FlushInstructionCache(self.process, va, len(data))

    def query(self, va):
        mbi = w.MEMORY_BASIC_INFORMATION()
        if not w.VirtualQueryEx(self.process, va, ctypes.byref(mbi), ctypes.sizeof(mbi)):
            return None
        return mbi

    def is_executable(self, va):
        mbi = self.query(va)
        return bool(mbi and mbi.State == w.MEM_COMMIT and mbi.Protect & w.EXECUTABLE_PROTECT
                    and not mbi.Protect & w.PAGE_GUARD)

    # ---------------------------------------------------------------- modules
    def module_at(self, va):
        for m in self.modules.values():
            if m.base <= va < m.base + m.size:
                return m
        return None

    def module(self, name):
        name = name.lower()
        for m in self.modules.values():
            if m.name.lower() == name or m.name.lower() == name + ".dll":
                return m
        return None

    # ---------------------------------------------------------------- contexts
    def get_context(self, thread, flags=w.CONTEXT_CONTROL | w.CONTEXT_INTEGER):
        ctx = w.new_context(flags)
        if not w.GetThreadContext(thread.handle, ctypes.byref(ctx)):
            raise DebugError(f"GetThreadContext({thread.tid}) failed: {w.last_error()}")
        return ctx

    def set_context(self, thread, ctx, flags=None):
        if flags is not None:
            ctx.ContextFlags = flags
        if not w.SetThreadContext(thread.handle, ctypes.byref(ctx)):
            raise DebugError(f"SetThreadContext({thread.tid}) failed: {w.last_error()}")

    # ---------------------------------------------------------------- software breakpoints
    def add_bp(self, va, handler, oneshot=False):
        """int3 at va. handler(hit) runs on every hit (oneshot: first hit only). Returns a token
        for remove_bp. Several handlers may share one address."""
        bp = self.bps.get(va)
        if bp is None:
            orig = self.read(va, 1, mask=False)[0]
            bp = self.bps[va] = SwBreakpoint(va, orig)
            self._bp_sorted = None
            self.retired.discard(va)
            self._arm(bp)
        token = self._next_token
        self._next_token += 1
        bp.handlers[token] = (handler, oneshot)
        return (va, token)

    def add_bps(self, vas, handler, oneshot=True, chunk=0x10000):
        """Many breakpoints at once (original bytes read in chunks). Returns the number armed;
        addresses that cannot be written are skipped."""
        vas = sorted(set(vas))
        armed = 0
        i = 0
        while i < len(vas):
            start = vas[i]
            j = i
            while j < len(vas) and vas[j] < start + chunk:
                j += 1
            data = self.read_partial(start, vas[j - 1] - start + 1, mask=False)
            for va in vas[i:j]:
                if va in self.bps:
                    bp = self.bps[va]
                elif va - start < len(data):
                    bp = self.bps[va] = SwBreakpoint(va, data[va - start])
                    self.retired.discard(va)
                    try:
                        self._arm(bp)
                    except DebugError:
                        del self.bps[va]
                        continue
                else:
                    continue
                token = self._next_token
                self._next_token += 1
                bp.handlers[token] = (handler, oneshot)
                armed += 1
            i = j
        self._bp_sorted = None
        return armed

    def remove_bp(self, token):
        va, tok = token
        bp = self.bps.get(va)
        if bp is None or tok not in bp.handlers:
            return
        del bp.handlers[tok]
        if not bp.handlers:
            self._drop(bp)

    def _arm(self, bp):
        self.write(bp.va, b"\xCC")
        bp.armed = True

    def _drop(self, bp):
        if bp.armed:
            try:
                self.write(bp.va, bytes([bp.orig]))
            except DebugError:
                pass
            bp.armed = False
        self.bps.pop(bp.va, None)
        self._bp_sorted = None
        self.retired.add(bp.va)

    # ---------------------------------------------------------------- hardware breakpoints
    def set_hw(self, va, kind, size, handler, slot=None):
        """Debug-register breakpoint on all threads (also threads created later; rewritten when the
        program changes them). kind: 'x' execute, 'w' write, 'rw' read/write. size 1/2/4/8
        (va aligned to size; 1 for 'x'). Returns the slot index."""
        if kind not in DR7_RW or size not in DR7_LEN or va % size or (kind == "x" and size != 1):
            raise DebugError(f"bad hardware breakpoint {kind}/{size} at {va:#x}")
        if slot is None:
            free = [i for i, s in enumerate(self.hw) if s is None]
            if not free:
                raise DebugError("all 4 debug registers are in use")
            slot = free[0]
        self.hw[slot] = HwSlot(va, kind, size, handler)
        self.apply_hw()
        if not self._hw_timer:
            # The protector rewrites debug registers (startup, VEH context writes): check them.
            self._hw_timer = True
            self.every(2.0, self._refresh_hw)
        return slot

    def clear_hw(self, slot):
        self.hw[slot] = None
        self.apply_hw()

    def _dr_values(self):
        drs = [0, 0, 0, 0]
        dr7 = 0
        for i, s in enumerate(self.hw):
            if s is None:
                continue
            drs[i] = s.va
            dr7 |= 1 << (2 * i)
            dr7 |= DR7_RW[s.kind] << (16 + 4 * i)
            dr7 |= DR7_LEN[s.size] << (18 + 4 * i)
        return drs, dr7

    def _apply_hw_thread(self, th, drs, dr7, check_only=False):
        suspend = not self.in_event
        if suspend and w.SuspendThread(th.handle) == 0xFFFFFFFF:
            return False
        try:
            ctx = w.new_context(w.CONTEXT_DEBUG_REGISTERS)
            if not w.GetThreadContext(th.handle, ctypes.byref(ctx)):
                return False
            if [ctx.Dr0, ctx.Dr1, ctx.Dr2, ctx.Dr3] == drs and (ctx.Dr7 & 0xFFFF00FF) == dr7:
                return True
            if check_only:
                self.hw_fixes += 1
            ctx.Dr0, ctx.Dr1, ctx.Dr2, ctx.Dr3 = drs
            ctx.Dr7 = dr7
            ctx.Dr6 = 0
            ctx.ContextFlags = w.CONTEXT_DEBUG_REGISTERS
            return bool(w.SetThreadContext(th.handle, ctypes.byref(ctx)))
        finally:
            if suspend:
                w.ResumeThread(th.handle)

    def apply_hw(self, thread=None, check_only=False):
        """Write the debug registers to one thread or all threads (deferred until hw_hold
        seconds after start)."""
        if self.elapsed < self.hw_hold:
            if not self._hw_deferred:
                self._hw_deferred = True
                self.after(self.hw_hold - self.elapsed, self.apply_hw)
            return
        drs, dr7 = self._dr_values()
        for th in ([thread] if thread else list(self.threads.values())):
            self._apply_hw_thread(th, drs, dr7, check_only)

    def hw_active(self):
        return any(s is not None for s in self.hw)

    def _refresh_hw(self):
        if self.alive and self.hw_active() and self.elapsed >= self.hw_hold:
            self.apply_hw(check_only=True)

    # ---------------------------------------------------------------- stack
    def stack(self, ctx_or_regs, max_frames=32):
        """Unwind a thread's stack. Returns [(rip, rsp, method)], method 'unwind' (via .pdata of
        the module on disk), 'leaf' (no .pdata: rip = [rsp]) or 'scan' (call-preceded return
        addresses found on the stack after the unwind failed)."""
        if isinstance(ctx_or_regs, dict):
            regs = dict(ctx_or_regs)
        else:
            regs = {n: getattr(ctx_or_regs, w.CONTEXT_FIELD[n]) for n in w.GPR_NAMES + ("rip",)}
        frames = [(regs["rip"], regs["rsp"], "context")]
        cache = {}

        def read_u64(va):
            page = va & ~0xFFF
            data = cache.get(page)
            if data is None:
                data = cache[page] = self.read(page, 0x1000)
            off = va - page
            if off > 0xFF8:
                return struct.unpack("<Q", self.read(va, 8))[0]
            return struct.unpack_from("<Q", data, off)[0]

        while len(frames) < max_frames:
            rip, rsp = regs["rip"], regs["rsp"]
            mod = self.module_at(rip)
            method = "unwind"
            try:
                if mod is None or mod.pe is None or not mod.pe.unwind_frame(regs, mod.base, read_u64):
                    if mod is None or mod.pe is None or mod.pe.runtime_function(rip - mod.base) is None:
                        method = "leaf"
                        regs["rip"] = read_u64(rsp)
                        regs["rsp"] = rsp + 8
            except DebugError:
                break
            if not regs["rip"] or regs["rsp"] <= rsp or not self._plausible_return(regs["rip"]):
                frames += self._scan_stack(rsp, max_frames - len(frames))
                break
            frames.append((regs["rip"], regs["rsp"], method))
        return frames

    def _plausible_return(self, va):
        return self.module_at(va) is not None or self.is_executable(va)

    def _scan_stack(self, rsp, limit, depth=0x2000):
        data = self.read_partial(rsp, depth)
        out = []
        for off in range(0, len(data) - 7, 8):
            va = struct.unpack_from("<Q", data, off)[0]
            if va < 0x10000 or self.module_at(va) is None:
                continue
            if self._after_call(va):
                out.append((va, rsp + off + 8, "scan"))
                if len(out) >= limit:
                    break
        return out

    def _after_call(self, va):
        """Is va preceded by a call instruction (E8 rel32 or FF /2 with 0-5 extra bytes)?"""
        before = self.try_read(va - 7, 7)
        if not before:
            return False
        if before[2] == 0xE8:
            return True
        return any(before[7 - k] == 0xFF and before[8 - k] & 0x38 == 0x10 for k in range(2, 8))

    # ---------------------------------------------------------------- control
    def terminate(self, code=1):
        """TerminateProcess and drain the remaining debug events (from a hook: after it returns)."""
        if not self.alive:
            return
        w.TerminateProcess(self.process, code)
        if self.in_event:
            self.stop_requested = True
            self._terminating = True
            return
        if self._pending:
            w.ContinueDebugEvent(*self._pending, w.DBG_CONTINUE)
            self._pending = None
        end = time.monotonic() + 5
        while self.alive and time.monotonic() < end:
            if w.WaitForDebugEvent(ctypes.byref(self._event), 100):
                de = self._event
                if de.dwDebugEventCode == w.EXIT_PROCESS_DEBUG_EVENT:
                    self.alive = False
                    self.exit_code = de.u.ExitProcess.dwExitCode
                self._close_event_handles(de)
                w.ContinueDebugEvent(de.dwProcessId, de.dwThreadId, w.DBG_CONTINUE)
        w.WaitForSingleObject(self.process, 10000)
        self.alive = False

    def stop(self):
        """Make run() return after the current event."""
        self.stop_requested = True

    def run(self, duration=None):
        """Debug loop until the process exits, stop() or `duration` seconds after start.
        Returns True while the process is still alive."""
        self.stop_requested = False
        deadline = None if duration is None else self.t0 + duration
        de = self._event
        while self.alive and not self.stop_requested:
            now = time.monotonic()
            if deadline is not None and now >= deadline:
                break
            if self._timers:
                self._run_timers(now)
            if not w.WaitForDebugEvent(ctypes.byref(de), 50):
                err = w.last_error()
                if err in (121, 258):  # ERROR_SEM_TIMEOUT / WAIT_TIMEOUT
                    continue
                raise DebugError(f"WaitForDebugEvent failed: {err}")
            self._pending = (de.dwProcessId, de.dwThreadId)
            self.in_event = True
            try:
                status = self._dispatch(de)
            except DebugError:
                self.log(f"[owdbg] event {de.dwDebugEventCode} failed:\n{traceback.format_exc()}")
                status = (w.DBG_EXCEPTION_NOT_HANDLED if de.dwDebugEventCode == w.EXCEPTION_DEBUG_EVENT
                          else w.DBG_CONTINUE)
            finally:
                self.in_event = False
            w.ContinueDebugEvent(de.dwProcessId, de.dwThreadId, status)
            self._pending = None
        if self._terminating:
            self._terminating = False
            self.terminate()
        return self.alive

    # ---------------------------------------------------------------- event dispatch
    def _close_event_handles(self, de):
        code = de.dwDebugEventCode
        if code == w.CREATE_PROCESS_DEBUG_EVENT and de.u.CreateProcessInfo.hFile:
            w.CloseHandle(de.u.CreateProcessInfo.hFile)
        elif code == w.LOAD_DLL_DEBUG_EVENT and de.u.LoadDll.hFile:
            w.CloseHandle(de.u.LoadDll.hFile)

    def _dispatch(self, de):
        code = de.dwDebugEventCode
        tid = de.dwThreadId
        if code == w.EXCEPTION_DEBUG_EVENT:
            return self._on_exception(de, tid)
        if code == w.CREATE_THREAD_DEBUG_EVENT:
            info = de.u.CreateThread
            th = self.threads[tid] = Thread(tid, info.hThread, info.lpStartAddress or 0, info.lpThreadLocalBase or 0)
            if self.hw_active() and self.elapsed >= self.hw_hold:
                self.apply_hw(th)
            self._emit("thread_created", th)
        elif code == w.EXIT_THREAD_DEBUG_EVENT:
            th = self.threads.pop(tid, None)
            if th is not None:
                if th.step_va is not None:
                    self._step_done(th.step_va)
                self._emit("thread_exited", th, de.u.ExitThread.dwExitCode)
        elif code == w.LOAD_DLL_DEBUG_EVENT:
            info = de.u.LoadDll
            path = w.handle_path(info.hFile) if info.hFile else None
            if info.hFile:
                w.CloseHandle(info.hFile)
            mod = self._add_module(info.lpBaseOfDll, path)
            self._emit("load_dll", mod)
        elif code == w.UNLOAD_DLL_DEBUG_EVENT:
            mod = self.modules.pop(de.u.UnloadDll.lpBaseOfDll, None)
            if mod is not None:
                self._emit("unload_dll", mod)
        elif code == w.CREATE_PROCESS_DEBUG_EVENT:
            info = de.u.CreateProcessInfo
            path = w.handle_path(info.hFile) if info.hFile else None
            if info.hFile:
                w.CloseHandle(info.hFile)
            self.main = self._add_module(info.lpBaseOfImage, path or self.exe)
            th = self.threads[tid] = Thread(tid, info.hThread, info.lpStartAddress or 0, info.lpThreadLocalBase or 0)
            th.name = "main"
            self._emit("create_process")
        elif code == w.EXIT_PROCESS_DEBUG_EVENT:
            self.exit_code = de.u.ExitProcess.dwExitCode
            self.alive = False
            self._emit("exit", self.exit_code)
        elif code == w.OUTPUT_DEBUG_STRING_EVENT and "debug_string" in self._hooks:
            info = de.u.DebugString
            try:
                text = self.read_cstr(info.lpDebugStringData, info.nDebugStringLength or 512,
                                      wide=bool(info.fUnicode))
            except DebugError:
                text = None
            if text:
                self._emit("debug_string", text)
        return w.DBG_CONTINUE

    def _add_module(self, base, path):
        size = 0
        if path and os.path.isfile(path):
            try:
                size = open_pe(path).size_of_image
            except (OSError, ValueError):
                size = 0
        if not size:
            try:
                hdr = self.read(base, 0x400, mask=False)
                pe = struct.unpack_from("<I", hdr, 0x3C)[0]
                size = struct.unpack_from("<I", hdr, pe + 24 + 56)[0]
            except (DebugError, struct.error):
                size = 0x1000
        mod = Module(base, size, path)
        self.modules[base] = mod
        return mod

    def _on_exception(self, de, tid):
        info = de.u.Exception
        rec = info.ExceptionRecord
        code = rec.ExceptionCode
        addr = rec.ExceptionAddress or 0
        first = bool(info.dwFirstChance)
        if first:
            if code == w.STATUS_BREAKPOINT or code == w.STATUS_WX86_BREAKPOINT:
                if not self.system_bp_seen:
                    self.system_bp_seen = True
                    self._emit("system_breakpoint")
                    return w.DBG_CONTINUE
                if addr in self.bps or addr in self.retired:
                    return self._on_sw_breakpoint(tid, addr)
            elif code == w.STATUS_SINGLE_STEP:
                th = self.threads.get(tid)
                if th is not None and (th.step_va is not None or self.hw_active()):
                    status = self._on_single_step(th, addr)
                    if status is not None:
                        return status
            elif code == w.MS_VC_EXCEPTION:
                self._thread_name(rec)
                return w.DBG_CONTINUE
        if code != w.STATUS_SINGLE_STEP:
            self.exception_sites[(code, addr)] += 1
        if first:
            self.exceptions[(code, True)] += 1
            if "exception" not in self._hooks:
                return w.DBG_EXCEPTION_NOT_HANDLED
            status = self._emit("exception", ExceptionEvent(self, de))
            return w.DBG_EXCEPTION_NOT_HANDLED if status is None else status
        self.exceptions[(code, False)] += 1
        status = self._emit("crash", ExceptionEvent(self, de))
        return w.DBG_EXCEPTION_NOT_HANDLED if status is None else status

    def _thread_name(self, rec):
        # THREADNAME_INFO {dwType=0x1000, szName, dwThreadID, dwFlags}
        if rec.NumberParameters >= 3 and rec.ExceptionInformation[0] == 0x1000:
            tid = rec.ExceptionInformation[2] & 0xFFFFFFFF
            try:
                name = self.read_cstr(rec.ExceptionInformation[1], 64)
            except DebugError:
                return
            th = self.threads.get(tid)
            if th is not None and name:
                th.name = name

    def _call(self, fn, hit):
        try:
            fn(hit)
        except Exception:
            key = getattr(fn, "__qualname__", repr(fn))
            self._handler_errors[key] += 1
            if self._handler_errors[key] <= 3:
                self.log(f"[owdbg] handler {key} failed:\n{traceback.format_exc()}")

    def _on_sw_breakpoint(self, tid, va):
        th = self.threads.get(tid)
        bp = self.bps.get(va)
        if th is None:
            return w.DBG_EXCEPTION_NOT_HANDLED
        ctx = self.get_context(th)
        ctx.Rip = va
        if bp is None:  # late hit of a breakpoint removed meanwhile: re-execute the original byte
            self.set_context(th, ctx, w.CONTEXT_CONTROL)
            self.stats["late_hits"] += 1
            return w.DBG_CONTINUE
        self.stats["bp_hits"] += 1
        hit = Hit(self, th, va, ctx)
        for token, (fn, oneshot) in list(bp.handlers.items()):
            if oneshot:
                bp.handlers.pop(token, None)
            self._call(fn, hit)
        flags = w.CONTEXT_CONTROL | (w.CONTEXT_INTEGER if hit.dirty else 0)
        if self.bps.get(va) is bp and bp.handlers and ctx.Rip != va:
            # A handler sent the thread elsewhere (an injected call): the original instruction
            # does not run now, so the breakpoint stays armed and nothing is stepped over. A
            # single step here would run the new target's first instruction instead and, if a
            # breakpoint sits there, leave this one disarmed for good.
            pass
        elif self.bps.get(va) is bp and bp.handlers:
            # Persistent: execute the original instruction with TF, re-arm on the single step.
            if bp.armed:
                self.write(va, bytes([bp.orig]))
                bp.armed = False
            bp.steppers += 1
            th.step_va = va
            th.step_had_tf = bool(ctx.EFlags & w.EFLAGS_TF)
            ctx.EFlags |= w.EFLAGS_TF
        elif self.bps.get(va) is bp:
            self._drop(bp)
        self.set_context(th, ctx, flags)
        return w.DBG_CONTINUE

    def _step_done(self, va):
        bp = self.bps.get(va)
        if bp is None:
            return
        bp.steppers -= 1
        if bp.steppers <= 0:
            bp.steppers = 0
            if bp.handlers and not bp.armed:
                try:
                    self._arm(bp)
                except DebugError:
                    pass

    def _on_single_step(self, th, addr):
        """Returns a continue status if the single step was (also) ours, else None."""
        ours_step = th.step_va is not None
        flags = w.CONTEXT_CONTROL | w.CONTEXT_INTEGER
        hw = self.hw_active()
        if hw:
            flags |= w.CONTEXT_DEBUG_REGISTERS
        ctx = self.get_context(th, flags)
        passthrough = False
        dirty = False
        if ours_step:
            va = th.step_va
            th.step_va = None
            self._step_done(va)
            if th.step_had_tf:
                passthrough = True  # the program's own trap flag: let its handler see the step
            else:
                ctx.EFlags &= ~w.EFLAGS_TF
        hits = (ctx.Dr6 & 0xF) if hw else 0
        handled_hw = False
        if hits:
            for i in range(4):
                s = self.hw[i]
                if hits & (1 << i) and s is not None:
                    handled_hw = True
                    self.stats["hw_hits"] += 1
                    hit = Hit(self, th, ctx.Rip, ctx, slot=i)
                    self._call(s.handler, hit)
                    dirty |= hit.dirty
                    if s.kind == "x":
                        ctx.EFlags |= w.EFLAGS_RF
        if not ours_step and not handled_hw:
            return None
        if hw:
            if not ours_step and ctx.Dr6 & 0x4000:
                passthrough = True  # the program's trap flag coincided with our data hit
            # Our bits are consumed; a passed-through step keeps its BS bit for the program.
            ctx.Dr6 = ctx.Dr6 & ~0xF if passthrough else 0
            # Handlers may have changed the slots: write the current configuration.
            if self.elapsed >= self.hw_hold:
                (ctx.Dr0, ctx.Dr1, ctx.Dr2, ctx.Dr3), ctx.Dr7 = self._dr_values()
        self.set_context(th, ctx, flags)
        return w.DBG_EXCEPTION_NOT_HANDLED if passthrough else w.DBG_CONTINUE

    # ---------------------------------------------------------------- misc
    def summary(self):
        """Counts of exceptions passed to the program, by code."""
        out = {}
        for (code, first), n in sorted(self.exceptions.items()):
            out[f"{code:08X} {w.exception_name(code)} {'first' if first else 'second'}"] = n
        return out

    def thread_name(self, tid):
        th = self.threads.get(tid)
        return th.name if th else None

    def write_minidump(self, path, tid=None, record=None, ctx=None, full=False):
        """MiniDumpWriteDump of the (frozen) debuggee. With tid+record+ctx the dump carries the
        exception. full: all memory (the game in a match: several GB)."""
        dbghelp = ctypes.WinDLL("dbghelp", use_last_error=True)

        class EXCEPTION_POINTERS(ctypes.Structure):
            _fields_ = [("ExceptionRecord", ctypes.c_void_p), ("ContextRecord", ctypes.c_void_p)]

        class MINIDUMP_EXCEPTION_INFORMATION(ctypes.Structure):
            _pack_ = 4
            _fields_ = [("ThreadId", ctypes.c_uint32), ("ExceptionPointers", ctypes.c_void_p),
                        ("ClientPointers", ctypes.c_int)]

        dbghelp.MiniDumpWriteDump.restype = ctypes.c_int
        dbghelp.MiniDumpWriteDump.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32,
                                              ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
        # handles, unloaded modules, indirectly referenced memory, process/thread data, memory info, thread info
        kind = 0x4 | 0x20 | 0x40 | 0x100 | 0x800 | 0x1000
        if full:
            kind |= 0x2
        import msvcrt
        param = None
        if tid is not None and record is not None and ctx is not None:
            ptrs = EXCEPTION_POINTERS(ctypes.addressof(record), ctypes.addressof(ctx))
            info = MINIDUMP_EXCEPTION_INFORMATION(tid, ctypes.addressof(ptrs), 0)
            param = ctypes.byref(info)
        with open(path, "wb") as f:
            ok = dbghelp.MiniDumpWriteDump(self.process, self.pid, msvcrt.get_osfhandle(f.fileno()), kind,
                                           param, None, None)
        if not ok:
            raise DebugError(f"MiniDumpWriteDump failed: {ctypes.get_last_error():#x}")
        return os.path.getsize(path)


def _quote(arg):
    if arg and not any(c in arg for c in ' \t"'):
        return arg
    return '"' + arg.replace('"', '\\"') + '"'
