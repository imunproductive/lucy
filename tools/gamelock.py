"""The game lock: one Overwatch at a time on this PC, taken in turn.

    python tools/gamelock.py acquire <name>    # waits for its turn, then holds the lock
    python tools/gamelock.py release <name>
    python tools/gamelock.py status

The lock is the folder `D:/Temp/lucy-game.lock` (the convention agents use) with
`owner.txt` inside: the holder's name and when it took the lock. Waiters queue in
`D:/Temp/lucy-game.queue/` (one file each, named by arrival time), so the lock goes
to whoever asked first instead of whoever retries at the right moment.

A lock held longer than `--stale` minutes (default 25) while no Overwatch process
runs is taken over: its holder died or forgot to release it. A holder or waiter
that was killed (a job deadline, a cancelled agent) can't clean up, so tickets and
`owner.txt` carry the process id: a dead waiter's ticket is dropped and a dead
holder's lock is taken over at once when no Overwatch runs.
"""

from __future__ import annotations

import argparse
import ctypes
import os
import subprocess
import sys
import time
from pathlib import Path

LOCK = Path(os.environ.get("LUCY_GAME_LOCK", "D:/Temp/lucy-game.lock"))
QUEUE = LOCK.with_name(LOCK.name.replace(".lock", ".queue"))
OWNER = LOCK / "owner.txt"


def overwatch_running() -> bool:
    out = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq Overwatch*", "/NH"], capture_output=True, text=True
    ).stdout
    return "Overwatch" in out


def alive(pid: int) -> bool:
    """Whether process `pid` still runs (Windows: an open handle whose exit code is STILL_ACTIVE)."""
    kernel = ctypes.windll.kernel32
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return False
    try:
        code = ctypes.c_ulong()
        return bool(kernel.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259
    finally:
        kernel.CloseHandle(handle)


def recorded_pid(path: Path, line: int) -> int | None:
    """The process id on `line` of a ticket or owner.txt, or None (none recorded, or an older file)."""
    try:
        words = path.read_text(encoding="utf-8").split()
        return int(words[line]) if len(words) > line and int(words[line]) > 0 else None
    except (OSError, ValueError):
        return None


def holder() -> tuple[str, float] | None:
    """The holder's name (the first word of owner.txt) and when it took the lock."""
    if not LOCK.exists():
        return None
    try:
        words = OWNER.read_text(encoding="utf-8").split()
        return (words[0] if words else "(unknown)"), OWNER.stat().st_mtime
    except OSError:
        return "(unknown)", LOCK.stat().st_mtime


def holder_died() -> bool:
    """Whether the holder recorded its process and that process is gone. A holder that took the lock from
    the command line (`acquire` then exit, holding it for a later game) records none."""
    pid = recorded_pid(OWNER, 2)
    return pid is not None and not alive(pid)


def take(name: str, pid: int) -> bool:
    try:
        LOCK.mkdir()
    except FileExistsError:
        return False
    OWNER.write_text(f"{name}\n{time.time()}\n{pid}\n", encoding="utf-8")
    return True


def drop() -> None:
    OWNER.unlink(missing_ok=True)
    try:
        LOCK.rmdir()
    except FileNotFoundError:
        pass


def prune_dead_tickets() -> None:
    """Drop the tickets of waiters that were killed while queueing (their `finally` never ran)."""
    for ticket in QUEUE.iterdir():
        pid = recorded_pid(ticket, 0)
        if pid is not None and not alive(pid):
            ticket.unlink(missing_ok=True)


def acquire(args: argparse.Namespace) -> None:
    """Wait for the lock in turn, then hold it. `args.hold_pid` is the process that holds it (default this
    one; 0 from the command line, whose process exits while the lock stays held)."""
    QUEUE.mkdir(parents=True, exist_ok=True)
    ticket = QUEUE / f"{time.time():017.6f}-{args.name}"
    ticket.write_text(f"{os.getpid()}\n", encoding="utf-8")
    hold_pid = getattr(args, "hold_pid", os.getpid())
    try:
        announced = None
        while True:
            prune_dead_tickets()
            first = min(QUEUE.iterdir(), default=None)
            if first == ticket and take(args.name, hold_pid):
                print(f"{args.name} holds the game lock")
                return
            held = holder()
            stale = held is not None and time.time() - held[1] > args.stale * 60
            if held and (stale or holder_died()) and not overwatch_running():
                print(f"taking over the {'stale' if stale else 'dead holder'} lock of {held[0]}")
                drop()
                continue
            state = (held[0] if held else None, first.name if first else None)
            if state != announced:
                print(f"waiting: held by {state[0]}, queue head {state[1]}", flush=True)
                announced = state
            time.sleep(5)
    finally:
        ticket.unlink(missing_ok=True)


def release(args: argparse.Namespace) -> None:
    held = holder()
    if held and held[0] != args.name:
        sys.exit(f"the lock is {held[0]}'s, not {args.name}'s")
    drop()
    print("released")


def status(_: argparse.Namespace) -> None:
    held = holder()
    if held:
        print(f"held by {held[0]} for {(time.time() - held[1]) / 60:.1f} min")
    else:
        print("free")
    for ticket in sorted(QUEUE.iterdir()) if QUEUE.exists() else []:
        pid = recorded_pid(ticket, 0)
        note = "" if pid is None else f" (pid {pid}{'' if alive(pid) else ', dead'})"
        print("  waiting:", ticket.name.split("-", 1)[1] + note)
    print("Overwatch running" if overwatch_running() else "no Overwatch running")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(required=True)
    command = commands.add_parser("acquire")
    command.add_argument("name")
    command.add_argument("--stale", type=float, default=25)
    command.set_defaults(run=acquire, hold_pid=0)
    command = commands.add_parser("release")
    command.add_argument("name")
    command.set_defaults(run=release)
    commands.add_parser("status").set_defaults(run=status)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
