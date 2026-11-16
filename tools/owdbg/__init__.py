"""owdbg: a small Win32 debugger (Python + ctypes) for the Overwatch 1.74 debug build.

CLIs (run from the repo root):
    python -m tools.owdbg.launch   ORPHEUS serve + the game under the debugger with the relay
    python -m tools.owdbg.probe    logging breakpoints: args, return value, caller, counts
    python -m tools.owdbg.crash    named stack, registers, disassembly and a minidump on an exception
    python -m tools.owdbg.watch    hardware read/write watch -> accessing instructions and functions
    python -m tools.owdbg.cover    one-shot breakpoints on function starts, per-phase coverage, diffs

Library: core.Debugger (process, events, breakpoints, debug registers, memory, stack walk),
game.Session (server + game + relay, cleanup), names.Names (index/docs names, address specs),
msg (message handlers/layouts from the index, ORPHEUS marker sends, Practice Range entry for
`watch --msg` / `probe --msg`).
"""
from .core import Debugger, DebugError, Hit
from .game import Session, add_session_args
from .names import Names, Expr

__all__ = ["Debugger", "DebugError", "Hit", "Session", "add_session_args", "Names", "Expr"]
