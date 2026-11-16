"""Start ORPHEUS serve + the debug build under the owdbg debugger with the relay, and keep it running.

usage: python -m tools.owdbg.launch [--duration S] [--screenshot PNG [--screenshot-at S]]
                                    [--enter-range] [--actions SCRIPT [--foreground-input]]
                                    [--no-server] [--no-login] [--account NAME] [--game-arg ARG] [--keep-state]

The game (the research repo's game/_retail_/Overwatch.debug.exe, locations in tools/paths.py) runs
windowed (--tank_NoFullScreen=1) with cwd _retail_; ORPHEUS's relay (target/release/owwfd_relay.dll)
is loaded by a remote LoadLibraryW thread created at the system breakpoint.
The client's first Battle.net attempt usually ends in "UNABLE TO LOG YOU IN / DISCONNECTED"; the
session then clicks LOGIN (PostMessage) until the relay log shows `relay CONNECTED` (--no-login
turns this off). Reports when the relay loaded, its `BYTE-STREAM ... host='127.0.0.1' port=1119`
line, the login, the game window and the exceptions passed to the game. Runs until --duration,
the game exits or Ctrl+C; then terminates the game and the server.
--enter-range goes into the Practice Range; --actions runs an in-match script (tools/owdbg/actions.py).
"""
import argparse

from . import actions
from .game import Session, add_session_args, find_window, screenshot, window_title


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    actions.add_action_args(p)
    add_session_args(p)
    p.add_argument("--screenshot", help="save a PNG of the game window")
    p.add_argument("--screenshot-at", type=float, default=60.0, help="seconds after start (default 60)")
    args = p.parse_args(argv)
    actions.prepare(args)

    with Session(args) as s:
        dbg = s.dbg
        act = actions.start(s, args)
        state = {"hooked": None, "window": None}

        def poll():
            if state["hooked"] is None and s.is_game and not args.no_relay:
                line = s.relay_hooked()
                if line:
                    state["hooked"] = line
                    s.log(f"[owdbg] relay hooked (t={dbg.elapsed:.1f}s): {line}")
            if state["window"] is None:
                hwnd = find_window(dbg.pid)
                if hwnd:
                    state["window"] = hwnd
                    s.log(f"[owdbg] window {hwnd:#x} '{window_title(hwnd)}' (t={dbg.elapsed:.1f}s)")

        def shot():
            hwnd = find_window(dbg.pid)
            if not hwnd:
                s.log("[owdbg] screenshot: no window")
                return
            wdt, hgt = screenshot(hwnd, args.screenshot)
            s.log(f"[owdbg] screenshot {wdt}x{hgt} -> {args.screenshot}")

        s.start()
        dbg.every(1.0, poll)
        if args.screenshot:
            dbg.after(args.screenshot_at, shot)
        alive = s.run(args.duration)
        poll()
        if s.is_game and not args.no_relay and state["hooked"] is None:
            s.log("[owdbg] relay log shows no BYTE-STREAM to 127.0.0.1 yet")
        if act is not None:
            s.log(f"[actions] {act.summary()}")
        s.log(f"[owdbg] alive={alive} t={dbg.elapsed:.0f}s")
        s.log_exceptions()


if __name__ == "__main__":
    main()
