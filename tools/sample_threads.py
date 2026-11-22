"""Sampling profiler for a running Windows process, without admin rights.

ETW profilers (samply, WPR) need an elevated session on Windows. This one
does what Very Sleepy does, from Python and dbghelp: every interval it
suspends each thread whose name matches, reads its context, walks its stack
with the image's unwind data (`StackWalk64`), and resumes it. A sample is
kept only if the thread used CPU since the last one (`GetThreadTimes`), so
waits don't count. At the end the program counters are named from the PDB
(`SymFromAddr`, with the inlined frames of each address), and it prints the
functions with the most samples of their own (self) and under them
(inclusive), and writes the stacks folded for a flame graph.

Build the target with symbols: the `profiling` profile (release code with
line tables; `just load` uses it).

    python tools/sample_threads.py --process lucy_server --thread "lucy-game|lucy-room" \
        --wait-thread --seconds 60 --folded research/out/profile.folded

Each sample stops the thread for the walk (about 0.1 ms), so the profiled
thread runs a little slower; the shares stay right.
"""

from __future__ import annotations

import argparse
import collections
import ctypes
import re
import sys
import time
from ctypes import wintypes as w

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
dbghelp = ctypes.WinDLL("dbghelp", use_last_error=True)
winmm = ctypes.WinDLL("winmm")

TH32CS_SNAPPROCESS = 0x2
TH32CS_SNAPTHREAD = 0x4
PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
THREAD_SUSPEND_RESUME = 0x0002
THREAD_GET_CONTEXT = 0x0008
THREAD_QUERY_INFORMATION = 0x0040
CONTEXT_FULL = 0x10000B
IMAGE_FILE_MACHINE_AMD64 = 0x8664
SYMOPT_UNDNAME = 0x2
SYMOPT_DEFERRED_LOADS = 0x4
SYMOPT_LOAD_LINES = 0x10
MAX_FRAMES = 96


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", w.DWORD),
        ("cntUsage", w.DWORD),
        ("th32ProcessID", w.DWORD),
        ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", w.DWORD),
        ("cntThreads", w.DWORD),
        ("th32ParentProcessID", w.DWORD),
        ("pcPriClassBase", w.LONG),
        ("dwFlags", w.DWORD),
        ("szExeFile", w.WCHAR * 260),
    ]


class THREADENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", w.DWORD),
        ("cntUsage", w.DWORD),
        ("th32ThreadID", w.DWORD),
        ("th32OwnerProcessID", w.DWORD),
        ("tpBasePri", w.LONG),
        ("tpDeltaPri", w.LONG),
        ("dwFlags", w.DWORD),
    ]


class ADDRESS64(ctypes.Structure):
    _fields_ = [("Offset", ctypes.c_uint64), ("Segment", w.WORD), ("Mode", ctypes.c_int)]


class KDHELP64(ctypes.Structure):
    _fields_ = [
        ("Thread", ctypes.c_uint64),
        ("ThCallbackStack", w.DWORD),
        ("ThCallbackBStore", w.DWORD),
        ("NextCallback", w.DWORD),
        ("FramePointer", w.DWORD),
        ("KiCallUserMode", ctypes.c_uint64),
        ("KeUserCallbackDispatcher", ctypes.c_uint64),
        ("SystemRangeStart", ctypes.c_uint64),
        ("KiUserExceptionDispatcher", ctypes.c_uint64),
        ("StackBase", ctypes.c_uint64),
        ("StackLimit", ctypes.c_uint64),
        ("BuildVersion", w.DWORD),
        ("RetpolineStubFunctionTableSize", w.DWORD),
        ("RetpolineStubFunctionTable", ctypes.c_uint64),
        ("RetpolineStubOffset", w.DWORD),
        ("RetpolineStubSize", w.DWORD),
        ("Reserved0", ctypes.c_uint64 * 2),
    ]


class STACKFRAME64(ctypes.Structure):
    _fields_ = [
        ("AddrPC", ADDRESS64),
        ("AddrReturn", ADDRESS64),
        ("AddrFrame", ADDRESS64),
        ("AddrStack", ADDRESS64),
        ("AddrBStore", ADDRESS64),
        ("FuncTableEntry", ctypes.c_void_p),
        ("Params", ctypes.c_uint64 * 4),
        ("Far", w.BOOL),
        ("Virtual", w.BOOL),
        ("Reserved", ctypes.c_uint64 * 3),
        ("KdHelp", KDHELP64),
    ]


class SYMBOL_INFOW(ctypes.Structure):
    _fields_ = [
        ("SizeOfStruct", w.ULONG),
        ("TypeIndex", w.ULONG),
        ("Reserved", ctypes.c_uint64 * 2),
        ("Index", w.ULONG),
        ("Size", w.ULONG),
        ("ModBase", ctypes.c_uint64),
        ("Flags", w.ULONG),
        ("Value", ctypes.c_uint64),
        ("Address", ctypes.c_uint64),
        ("Register", w.ULONG),
        ("Scope", w.ULONG),
        ("Tag", w.ULONG),
        ("NameLen", w.ULONG),
        ("MaxNameLen", w.ULONG),
        ("Name", w.WCHAR * 1024),
    ]


# CONTEXT (x64): 1232 bytes, 16-byte aligned; ContextFlags at 0x30, Rip at 0xF8,
# Rsp at 0x98, Rbp at 0xA0.
CONTEXT_SIZE = 1232


class Context:
    def __init__(self) -> None:
        self.raw = ctypes.create_string_buffer(CONTEXT_SIZE + 16)
        address = ctypes.addressof(self.raw)
        self.address = (address + 15) & ~15

    def field(self, offset: int, kind=ctypes.c_uint64):
        return kind.from_address(self.address + offset)


kernel32.CreateToolhelp32Snapshot.restype = w.HANDLE
kernel32.OpenProcess.restype = w.HANDLE
kernel32.OpenThread.restype = w.HANDLE
kernel32.SuspendThread.restype = w.DWORD
kernel32.GetThreadContext.argtypes = [w.HANDLE, ctypes.c_void_p]
kernel32.GetThreadDescription.argtypes = [w.HANDLE, ctypes.POINTER(w.LPWSTR)]
kernel32.GetThreadTimes.argtypes = [w.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
dbghelp.SymInitializeW.argtypes = [w.HANDLE, w.LPCWSTR, w.BOOL]
dbghelp.SymFunctionTableAccess64.restype = ctypes.c_void_p
dbghelp.SymFunctionTableAccess64.argtypes = [w.HANDLE, ctypes.c_uint64]
dbghelp.SymGetModuleBase64.restype = ctypes.c_uint64
dbghelp.SymGetModuleBase64.argtypes = [w.HANDLE, ctypes.c_uint64]
dbghelp.StackWalk64.argtypes = [
    w.DWORD,
    w.HANDLE,
    w.HANDLE,
    ctypes.POINTER(STACKFRAME64),
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_void_p,
    ctypes.c_void_p,
]
dbghelp.SymFromAddrW.argtypes = [
    w.HANDLE,
    ctypes.c_uint64,
    ctypes.POINTER(ctypes.c_uint64),
    ctypes.POINTER(SYMBOL_INFOW),
]
dbghelp.SymAddrIncludeInlineTrace.restype = w.DWORD
dbghelp.SymAddrIncludeInlineTrace.argtypes = [w.HANDLE, ctypes.c_uint64]
dbghelp.SymQueryInlineTrace.argtypes = [
    w.HANDLE,
    ctypes.c_uint64,
    w.DWORD,
    ctypes.c_uint64,
    ctypes.c_uint64,
    ctypes.POINTER(w.DWORD),
    ctypes.POINTER(w.DWORD),
]
dbghelp.SymFromInlineContextW.argtypes = [
    w.HANDLE,
    ctypes.c_uint64,
    w.ULONG,
    ctypes.POINTER(ctypes.c_uint64),
    ctypes.POINTER(SYMBOL_INFOW),
]

FUNCTION_TABLE = ctypes.cast(dbghelp.SymFunctionTableAccess64, ctypes.c_void_p)
MODULE_BASE = ctypes.cast(dbghelp.SymGetModuleBase64, ctypes.c_void_p)


def processes(name: str) -> list[tuple[int, str]]:
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    entry = PROCESSENTRY32W()
    entry.dwSize = ctypes.sizeof(entry)
    found = []
    ok = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
    while ok:
        if name.lower() in entry.szExeFile.lower():
            found.append((entry.th32ProcessID, entry.szExeFile))
        ok = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    kernel32.CloseHandle(snapshot)
    return found


def thread_ids(pid: int) -> list[int]:
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    entry = THREADENTRY32()
    entry.dwSize = ctypes.sizeof(entry)
    ids = []
    ok = kernel32.Thread32First(snapshot, ctypes.byref(entry))
    while ok:
        if entry.th32OwnerProcessID == pid:
            ids.append(entry.th32ThreadID)
        ok = kernel32.Thread32Next(snapshot, ctypes.byref(entry))
    kernel32.CloseHandle(snapshot)
    return ids


def thread_name(handle) -> str:
    text = w.LPWSTR()
    if kernel32.GetThreadDescription(handle, ctypes.byref(text)) < 0 or not text.value:
        return ""
    name = text.value
    kernel32.LocalFree(text)
    return name


def cpu_time(handle) -> int:
    times = [ctypes.c_uint64() for _ in range(4)]
    kernel32.GetThreadTimes(handle, *[ctypes.byref(t) for t in times])
    return times[2].value + times[3].value


class Sampler:
    def __init__(self, pid: int, pattern: re.Pattern) -> None:
        self.pid = pid
        self.pattern = pattern
        self.process = kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
        if not self.process:
            raise OSError(ctypes.get_last_error(), "OpenProcess")
        dbghelp.SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES)
        if not dbghelp.SymInitializeW(self.process, None, True):
            raise OSError(ctypes.get_last_error(), "SymInitialize")
        self.threads: dict[int, tuple[object, str, int]] = {}
        self.seen: set[int] = set()
        self.context = Context()
        self.stacks: collections.Counter = collections.Counter()
        self.by_thread: collections.Counter = collections.Counter()
        self.idle = 0

    def refresh(self) -> None:
        for tid in thread_ids(self.pid):
            if tid in self.seen:
                continue
            self.seen.add(tid)
            access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION
            handle = kernel32.OpenThread(access, False, tid)
            if not handle:
                continue
            name = thread_name(handle)
            if self.pattern.search(name):
                self.threads[tid] = (handle, name, cpu_time(handle))
            else:
                kernel32.CloseHandle(handle)

    def sample(self) -> None:
        for tid, (handle, name, last) in list(self.threads.items()):
            if kernel32.SuspendThread(handle) == 0xFFFFFFFF:
                kernel32.CloseHandle(handle)
                del self.threads[tid]
                continue
            try:
                used = cpu_time(handle)
                if used == last:
                    self.idle += 1
                    continue
                self.threads[tid] = (handle, name, used)
                self.context.field(0x30, ctypes.c_uint32).value = CONTEXT_FULL
                if not kernel32.GetThreadContext(handle, self.context.address):
                    continue
                frame = STACKFRAME64()
                frame.AddrPC.Offset = self.context.field(0xF8).value
                frame.AddrPC.Mode = 3
                frame.AddrStack.Offset = self.context.field(0x98).value
                frame.AddrStack.Mode = 3
                frame.AddrFrame.Offset = self.context.field(0xA0).value
                frame.AddrFrame.Mode = 3
                pcs = []
                while len(pcs) < MAX_FRAMES and dbghelp.StackWalk64(
                    IMAGE_FILE_MACHINE_AMD64,
                    self.process,
                    handle,
                    ctypes.byref(frame),
                    self.context.address,
                    None,
                    FUNCTION_TABLE,
                    MODULE_BASE,
                    None,
                ):
                    if frame.AddrPC.Offset == 0:
                        break
                    pcs.append(frame.AddrPC.Offset)
            finally:
                kernel32.ResumeThread(handle)
            if pcs:
                self.stacks[tuple(pcs)] += 1
                self.by_thread[re.sub(r"\d+$", "N", name)] += 1


class Names:
    """Function names of program counters, with their inlined frames."""

    def __init__(self, process) -> None:
        self.process = process
        self.cache: dict[tuple[int, bool], list[str]] = {}
        self.symbol = SYMBOL_INFOW()

    def _symbol(self, call, *args) -> str | None:
        # sizeof(SYMBOL_INFOW) with its one-character name.
        self.symbol.SizeOfStruct = 88
        self.symbol.MaxNameLen = 1024
        displacement = ctypes.c_uint64()
        if call(self.process, *args, ctypes.byref(displacement), ctypes.byref(self.symbol)):
            return self.symbol.Name
        return None

    def frames(self, pc: int, leaf: bool) -> list[str]:
        """Outermost first: the function, then the inlined calls to the pc."""
        key = (pc, leaf)
        if key in self.cache:
            return self.cache[key]
        # A return address points after the call: name the call itself.
        address = pc if leaf else pc - 1
        outer = self._symbol(dbghelp.SymFromAddrW, ctypes.c_uint64(address)) or f"0x{pc:X}"
        names = [outer]
        inlined = dbghelp.SymAddrIncludeInlineTrace(self.process, address)
        if inlined:
            context, index = w.DWORD(), w.DWORD()
            if dbghelp.SymQueryInlineTrace(
                self.process, address, 0, address, address, ctypes.byref(context), ctypes.byref(index)
            ):
                chain = []
                for offset in range(inlined):
                    name = self._symbol(
                        dbghelp.SymFromInlineContextW, ctypes.c_uint64(address), context.value + offset
                    )
                    if name:
                        chain.append(name)
                # The trace lists the innermost first.
                names.extend(reversed(chain))
        self.cache[key] = names
        return names


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--process", required=True, help="part of the executable's name")
    parser.add_argument("--thread", default=".", help="regex on the thread names")
    parser.add_argument("--seconds", type=float, default=30.0)
    parser.add_argument("--interval", type=float, default=1.0, help="ms between samples")
    parser.add_argument("--wait-thread", action="store_true", help="wait for a matching thread")
    parser.add_argument("--delay", type=float, default=0.0, help="seconds to wait once it runs")
    parser.add_argument("--top", type=int, default=40)
    parser.add_argument("--folded", help="write the folded stacks here")
    args = parser.parse_args()

    winmm.timeBeginPeriod(1)
    deadline = time.time() + 600
    while not (found := processes(args.process)):
        if time.time() > deadline:
            print("no such process", file=sys.stderr)
            return 1
        time.sleep(0.2)
    pid, exe = found[0]
    sampler = Sampler(pid, re.compile(args.thread))
    sampler.refresh()
    while args.wait_thread and not sampler.threads:
        time.sleep(0.2)
        sampler.refresh()
    time.sleep(args.delay)
    print(f"sampling {exe} ({pid}): {sorted({n for _, n, _ in sampler.threads.values()})}", file=sys.stderr)
    start = time.perf_counter()
    refreshed = start
    ticks = 0
    while time.perf_counter() - start < args.seconds:
        sampler.sample()
        ticks += 1
        now = time.perf_counter()
        if now - refreshed > 1.0:
            sampler.refresh()
            refreshed = now
        time.sleep(args.interval / 1000)
    elapsed = time.perf_counter() - start
    total = sum(sampler.stacks.values())
    print(
        f"{total} busy samples, {sampler.idle} idle, {ticks / elapsed:.0f} rounds/s; by thread: "
        f"{dict(sampler.by_thread)}"
    )
    if not total:
        return 1

    names = Names(sampler.process)
    own = collections.Counter()
    under = collections.Counter()
    folded = collections.Counter()
    for pcs, count in sampler.stacks.items():
        # Outermost first.
        frames = []
        for depth, pc in enumerate(reversed(pcs)):
            frames.extend(names.frames(pc, depth == len(pcs) - 1))
        own[frames[-1]] += count
        for name in set(frames):
            under[name] += count
        folded[";".join(frames)] += count

    def table(title: str, counter: collections.Counter) -> None:
        print(f"\n{title}")
        for name, count in counter.most_common(args.top):
            print(f"{100 * count / total:6.2f}%  {count:7d}  {name[:160]}")

    table("Self (the function the thread was in):", own)
    table("Inclusive (the function was on the stack):", under)
    if args.folded:
        with open(args.folded, "w", encoding="utf-8") as out:
            for stack, count in folded.items():
                out.write(f"{stack} {count}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
