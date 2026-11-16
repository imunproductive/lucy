"""Differential coverage: one-shot breakpoints on function starts, split into phases.

usage: python -m tools.owdbg.cover run [--range A-B] [--list FILE] [--include-protector]
           [--include-obfuscated] [--max N] [--arm-delay S] [--after-login]
           --phase NAME:SECONDS[:ACTION] ... [--out FILE.json] [session options]
       python -m tools.owdbg.cover diff A.json [B.json] [--phase-a NAME,..] [--phase-b NAME,..] [--out FILE]

run: arms an int3 on every primary .pdata function start of the game in the selection (default
all game code, RVA 0xC0000-0x2400000, without the startup/in-game protector regions and the 396
strong obfuscated functions of the research repo's docs/analysis/obfuscated_functions.json),
--arm-delay seconds after start (default 0 = at the system breakpoint; with --after-login after the
relay connected to ORPHEUS, ~10 s before the main menu shows). --max N spreads N functions evenly
over the selection (the same N gives the same set in every run). Each breakpoint removes itself on
its first hit and records the current phase. Phases run back to back from arming; ACTION is sent to
the game window every 0.5 s during the phase: key=VK (hex/decimal virtual-key code or a letter),
click=X,Y (client coordinates), move (PostMessage, the user's desktop is not touched). Enter in the
console ends the current phase early. After the last phase the game is terminated. Output JSON:
phases with the functions first hit in each ({rva, orpheus, name, t, tid, thread}), counts,
functions never hit.
--range accepts RVAs or VAs (A-B), --list a file of RVAs/VAs/names (one per line).

diff: functions hit in A's phases (default all) but not in B's (default all), and the reverse. With
one file, compares two of its phases (each function appears only in its first phase).
"""
import argparse
import json
import os
import sys
import time

from tools import paths

from .game import Session, add_session_args, out_path, post_click, post_key, post_move, screenshot
from .names import Names, hx
from .pe import open_pe

GAME_CODE = (0xC0000, 0x2400000)  # between the startup protector and the in-game protector runtime


def load_obfuscated():
    path = paths.OBFUSCATED_JSON
    try:
        data = json.load(open(path, encoding="utf-8"))
    except (OSError, ValueError):
        return []
    out = []
    for f in data.get("functions", []):
        if f.get("confidence") == "strong":
            start = int(f["addr"], 16) - 0x140000000
            out.append((start, start + int(f["size"], 16)))
    return out


def parse_phase(text):
    parts = text.split(":", 2)
    if len(parts) < 2:
        raise argparse.ArgumentTypeError(f"phase must be NAME:SECONDS[:ACTION], got {text!r}")
    return {"name": parts[0], "seconds": float(parts[1]), "action": parts[2] if len(parts) > 2 else None}


def parse_action(action):
    if not action:
        return None
    kind, _, arg = action.partition("=")
    if kind == "key":
        if len(arg) == 1 and arg.isalnum():
            vk = ord(arg.upper())
        else:
            vk = int(arg, 0)
        return lambda hwnd, i: post_key(hwnd, vk)
    if kind == "click":
        x, y = (int(v) for v in arg.split(","))
        return lambda hwnd, i: post_click(hwnd, x, y)
    if kind == "move":
        return lambda hwnd, i: post_move(hwnd, 200 + (i * 37) % 600, 150 + (i * 53) % 400)
    raise SystemExit(f"unknown phase action {action!r} (key=VK, click=X,Y, move)")


def select_functions(args, names, pe, is_game=True):
    starts = pe.function_starts()
    if args.list:
        wanted = set()
        for line in open(args.list, encoding="utf-8"):
            line = line.split("#", 1)[0].strip()
            if line:
                kind, *rest = names.parse(line)
                if kind != "rva":
                    raise SystemExit(f"--list entry {line!r} is not in the game image")
                wanted.add(rest[0])
        starts = [s for s in starts if s in wanted]
    lo, hi = (0, pe.size_of_image) if args.include_protector or not is_game else GAME_CODE
    if args.range:
        a, b = args.range.split("-")
        ra, rb = names.parse(a)[1], names.parse(b)[1]
        lo, hi = max(lo, ra), min(hi, rb)
    starts = [s for s in starts if lo <= s < hi]
    if is_game and not args.include_obfuscated:
        obf = load_obfuscated()
        if obf:
            import bisect
            obf.sort()
            begins = [o[0] for o in obf]

            def inside(rva):
                i = bisect.bisect_right(begins, rva) - 1
                return i >= 0 and rva < obf[i][1]
            starts = [s for s in starts if not inside(s)]
    if args.max and len(starts) > args.max:
        step = len(starts) / args.max
        starts = [starts[int(i * step)] for i in range(args.max)]
    return starts


def run(args):
    with Session(args) as s:
        dbg, names = s.dbg, s.names
        pe = open_pe(s.exe)
        starts = select_functions(args, names, pe, s.is_game)
        phases = [dict(p, start=None, end=None, functions=[]) for p in args.phase]
        actions = [parse_action(p["action"]) for p in phases]
        state = {"phase": -1, "armed": 0, "arm_t": None, "phase_end": None, "ticks": 0}
        hits = {}

        def on_hit(hit):
            rva = hit.va - dbg.main.base
            i = state["phase"]
            hits[rva] = (i, round(dbg.elapsed, 3), hit.tid, hit.thread.name)

        def next_phase():
            i = state["phase"]
            now = dbg.elapsed
            if i >= 0:
                phases[i]["end"] = round(now, 3)
                n = sum(1 for v in hits.values() if v[0] == i)
                s.log(f"[cover] phase {phases[i]['name']} ended: {n} new functions")
                hwnd = s.window() if args.screenshots else None
                if hwnd:
                    os.makedirs(args.screenshots, exist_ok=True)
                    path = os.path.join(args.screenshots, f"phase-{i + 1}-{phases[i]['name']}.png")
                    screenshot(hwnd, path)
                    phases[i]["screenshot"] = path
            i += 1
            state["phase"] = i
            if i >= len(phases):
                dbg.stop()
                return
            phases[i]["start"] = round(now, 3)
            state["phase_end"] = now + phases[i]["seconds"]
            s.log(f"[cover] phase {phases[i]['name']} ({phases[i]['seconds']} s"
                  + (f", action {phases[i]['action']}" if phases[i]["action"] else "") + ")")

        def arm():
            t = time.monotonic()
            base = dbg.main.base
            state["armed"] = dbg.add_bps([base + r for r in starts], on_hit, oneshot=True)
            state["arm_t"] = round(dbg.elapsed, 3)
            s.log(f"[cover] armed {state['armed']} one-shot breakpoints in {time.monotonic() - t:.2f} s")
            next_phase()
            dbg.every(0.5, tick)

        def tick():
            if state["phase"] >= len(phases):
                return
            if dbg.elapsed >= state["phase_end"] or console_enter():
                next_phase()
                if state["phase"] >= len(phases):
                    return
            act = actions[state["phase"]]
            if act:
                hwnd = s.window()
                if hwnd:
                    state["ticks"] += 1
                    act(hwnd, state["ticks"])

        if args.after_login:
            def wait_login():
                if state.get("login_seen"):
                    return
                if s.logged_in or dbg.elapsed > 180:
                    state["login_seen"] = True
                    s.log(f"[cover] {'logged in' if s.logged_in else 'NOT logged in after 180 s'}; "
                          f"arming in {args.arm_delay} s")
                    dbg.after(args.arm_delay, arm)
            dbg.every(1.0, wait_login)
        elif args.arm_delay:
            dbg.after(args.arm_delay, arm)
        else:
            dbg.on("system_breakpoint", arm)
        s.start()
        s.run(None if args.duration is None else args.duration)
        alive = dbg.alive
        if state["phase"] < len(phases) and state["phase"] >= 0:
            phases[state["phase"]]["end"] = round(dbg.elapsed, 3)
        for rva, (i, t, tid, thread) in sorted(hits.items(), key=lambda kv: kv[1][1]):
            if 0 <= i < len(phases):
                d = names.describe(dbg.main.base + rva)
                phases[i]["functions"].append({"rva": hx(rva), "orpheus": d.get("orpheus"), "name": d.get("name"),
                                               "t": t, "tid": tid, "thread": thread})
        before = [rva for rva, v in hits.items() if v[0] < 0]
        result = {"exe": str(s.exe), "base": hx(dbg.main.base) if dbg.main else None,
                  "selection": {"candidates": len(starts), "armed": state["armed"], "range": args.range,
                                "list": args.list, "max": args.max},
                  "armed_at": state["arm_t"], "alive_at_end": alive, "exit_code": dbg.exit_code,
                  "hits": len(hits), "not_hit": state["armed"] - len(hits), "late_hits": dbg.stats["late_hits"],
                  "phases": [{k: p.get(k) for k in ("name", "seconds", "action", "start", "end", "screenshot")}
                             | {"count": len(p["functions"]), "functions": p["functions"]} for p in phases],
                  "before_first_phase": len(before), "exceptions_passed": dbg.summary()}
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(result, f, indent=1)
        s.log(f"[cover] alive={alive} armed={state['armed']} hit={len(hits)} -> {args.out}")
        for p in result["phases"]:
            s.log(f"  phase {p['name']}: {p['count']} functions first hit")
            for fn in p["functions"][:8]:
                s.log(f"      {fn['rva']:>10} {fn['name'] or ''}")
        s.log_exceptions(5)


def console_enter():
    try:
        import msvcrt
    except ImportError:
        return False
    if not sys.stdin.isatty():
        return False
    pressed = False
    while msvcrt.kbhit():
        pressed |= msvcrt.getwch() in "\r\n"
    return pressed


def phase_set(data, wanted):
    names = [n for n in wanted.split(",")] if wanted else [p["name"] for p in data["phases"]]
    out = {}
    for p in data["phases"]:
        if p["name"] in names:
            for f in p["functions"]:
                out[int(f["rva"], 16)] = f
    missing = set(names) - {p["name"] for p in data["phases"]}
    if missing:
        raise SystemExit(f"no phase {sorted(missing)} in the file (has {[p['name'] for p in data['phases']]})")
    return out


def diff(args):
    a = json.load(open(args.a, encoding="utf-8"))
    b = json.load(open(args.b, encoding="utf-8")) if args.b else a
    if not args.b and not (args.phase_a and args.phase_b):
        raise SystemExit("with one file give --phase-a and --phase-b")
    sa, sb = phase_set(a, args.phase_a), phase_set(b, args.phase_b)
    names = Names(use_index=not args.no_index)

    def rows(keys, src):
        out = []
        for rva in sorted(keys):
            f = src[rva]
            out.append({"rva": f["rva"], "orpheus": f.get("orpheus"), "name": f.get("name") or names.game_name(rva),
                        "t": f.get("t")})
        return out

    only_a, only_b = set(sa) - set(sb), set(sb) - set(sa)
    result = {"a": {"file": args.a, "phases": args.phase_a or "all", "functions": len(sa)},
              "b": {"file": args.b or args.a, "phases": args.phase_b or "all", "functions": len(sb)},
              "common": len(set(sa) & set(sb)), "only_a": rows(only_a, sa), "only_b": rows(only_b, sb)}
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(result, f, indent=1)
    print(f"A {result['a']}\nB {result['b']}\ncommon {result['common']}, only A {len(only_a)}, only B {len(only_b)}")
    for label, lst in (("only A", result["only_a"]), ("only B", result["only_b"])):
        print(f"{label}:")
        for r in lst[:args.show]:
            print(f"  {r['rva']:>10} {r['orpheus'] or '':>16} {r['name'] or ''}")
        if len(lst) > args.show:
            print(f"  ... {len(lst) - args.show} more" + (f" (all in {args.out})" if args.out else ""))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="record coverage")
    r.add_argument("--range", help="RVA/VA range A-B")
    r.add_argument("--list", help="file with function RVAs/VAs/names")
    r.add_argument("--include-protector", action="store_true", help="also the protector regions")
    r.add_argument("--include-obfuscated", action="store_true", help="also the strong obfuscated functions")
    r.add_argument("--max", type=int, help="at most N functions (evenly spread over the selection)")
    r.add_argument("--arm-delay", type=float, default=0, help="arm N seconds after start (or after the login)")
    r.add_argument("--after-login", action="store_true",
                   help="count --arm-delay from the relay's connection to ORPHEUS (the main menu follows)")
    r.add_argument("--phase", type=parse_phase, action="append", required=True, help="NAME:SECONDS[:ACTION]")
    r.add_argument("--out", help="JSON output (default research/out/owdbg/cover-<time>.json)")
    r.add_argument("--screenshots", help="directory for a PNG of the game window at the end of each phase")
    add_session_args(r)
    d = sub.add_parser("diff", help="compare phases of two runs (or of one run)")
    d.add_argument("a")
    d.add_argument("b", nargs="?")
    d.add_argument("--phase-a", help="comma-separated phase names of A (default all)")
    d.add_argument("--phase-b", help="comma-separated phase names of B (default all)")
    d.add_argument("--out", help="JSON output")
    d.add_argument("--show", type=int, default=40, help="rows to print per side")
    d.add_argument("--no-index", action="store_true")
    args = p.parse_args(argv)
    if args.cmd == "run":
        args.out = args.out or str(out_path("cover", ".json"))
        run(args)
    else:
        diff(args)


if __name__ == "__main__":
    main()
