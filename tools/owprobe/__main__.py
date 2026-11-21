"""owprobe CLI: talk to owprobe.dll in a running game (`orpheus play --probe`, or `launch` below).

Addresses: ORPHEUS VAs (0x7FF7...), index/docs names (GetComp4, Msg25005_Handler), `c59`/`ent`/`c4` (the local
player's component 59, entity, component 4, once the frame sampler ran). Run from the ORPHEUS root.
"""
import argparse
import json
import sys
import time
from collections import Counter
from pathlib import Path

from tools import paths

from . import script
from .client import Probe, ProbeError
from .screenshot import screenshot

OUT = paths.OUT / "owprobe"


def out_path(stem, suffix):
    OUT.mkdir(parents=True, exist_ok=True)
    return OUT / f"{stem}-{time.strftime('%Y%m%d-%H%M%S')}{suffix}"


def hx(value):
    return f"0x{value:X}" if isinstance(value, int) else str(value)


def number(text):
    return int(text, 0)


def connect(args):
    return Probe(args.port)


def cmd_info(args):
    p = connect(args)
    info = p.info()
    for key, value in info.items():
        print(f"{key:14} {hx(value) if isinstance(value, int) and value > 0xFFFF else value}")


def cmd_modules(args):
    for m in connect(args).modules():
        print(f"{m['base']:016X} {m['size']:9X} {m['path']}")


def cmd_regions(args):
    p = connect(args)
    for r in p.regions(args.start, args.end, committed=not args.all):
        print(f"{r['base']:016X} {r['size']:10X} {r['state']:7} {r['type']:8} prot={r['protect']:#x}")


def cmd_read(args):
    p = connect(args)
    address = p.addr(args.addr)
    if args.type:
        print(p.eval(f"[{address:#x}]:{args.type}")[0])
        return
    data = p.read(address, args.len, partial=True)
    for at in range(0, len(data), 16):
        row = data[at:at + 16]
        text = "".join(chr(b) if 32 <= b < 127 else "." for b in row)
        print(f"{address + at:016X}  {row.hex(' '):48}  {text}")


def cmd_write(args):
    p = connect(args)
    print(p.write(args.addr, bytes.fromhex(args.hex)))


def cmd_eval(args):
    p = connect(args)
    for expr, value in zip(args.exprs, p.eval(*args.exprs)):
        print(f"{expr} = {hx(value)}")


def cmd_disasm(args):
    p = connect(args)
    for line in p.disasm(args.addr, args.count):
        print(f"{p.orpheus(line['addr']):016X}  {line['bytes']:24} {line['text']}")


def cmd_scan(args):
    p = connect(args)
    found = p.scan(value=args.value, pattern=args.pattern, start=args.start, end=args.end, max=args.max)
    for at in found:
        print(f"{at:016X}  {p.describe(at) if args.pattern else ''}")
    print(f"{len(found)} found", file=sys.stderr)


def cmd_components(args):
    p = connect(args)
    vtable = args.vtable
    if vtable.lower().startswith("comp"):
        from tools.owindex import Index
        info = Index.open().component(int(vtable[4:]))
        vtable = info.get("vtable") or (info["vtable_rva"] + 0x7FF788F10000)
    for c in p.components(vtable if isinstance(vtable, int) else p.addr(vtable)):
        print(f"{c['component']:016X} entity {c['entity']:016X} id {c['id']:08X}")


def cmd_component(args):
    from .statescript import resolve_entity
    p = connect(args)
    found = p.component(resolve_entity(p, args.entity), args.kind)
    print(f"entity {hx(found['entity'])} component {args.kind} {hx(found['component'])}")


def cmd_entity_vars(args):
    from .statescript import entity_vars, variable_name
    p = connect(args)
    values = entity_vars(p, args.entity, {int(v.rstrip("e")) for v in args.ids} if args.ids else None)
    for ident in sorted(values):
        name = variable_name(ident)
        print(f"#{ident}e{f' {name}' if name else ''} = {json.dumps(values[ident])}")


def describe_record(p, r):
    kind = r.get("kind")
    head = f"#{r['seq']} t={r['t']:.4f} tid={r['thread']}"
    if kind == "hit":
        caller = f" from {p.describe(r['caller'])}" if r.get("caller") else ""
        regs = r["registers"]
        text = f"{head} hit {r['hook']}{caller} rcx={hx(regs['rcx'])} rdx={hx(regs['rdx'])} r8={hx(regs['r8'])}"
        if r["reads"]:
            text += " reads=" + json.dumps(r["reads"])
        for frame in r.get("stack", []):
            text += f"\n      <- {p.describe(frame)}"
        return text
    if kind == "return":
        return f"{head} return {r['hook']} rax={hx(r['rax'])} to {p.describe(r['caller'])}" + (
            f" reads={json.dumps(r['reads'])}" if r["reads"] else "")
    if kind == "frame":
        pos = r.get("position")
        at = "None" if pos is None else "(" + ", ".join(f"{v:.3f}" for v in pos) + ")"
        text = f"{head} frame {r['frame']} pos={at} view={r['view']} buttons={r['buttons']:#x}"
        mover = r.get("mover")
        if mover:
            feet = ", ".join(f"{v:.3f}" for v in mover["position"])
            speed = ", ".join(f"{v:.2f}" for v in mover["velocity"])
            text += (f" mover f{mover['frame']} feet=({feet}) v=({speed}) flags={mover['flags']:#x}"
                     f"{' grounded' if mover['grounded'] else ''}")
        camera = r.get("camera")
        if camera:
            text += " camera=(" + ", ".join(f"{v:.3f}" for v in camera["position"]) + f") yaw={camera['yaw']:.1f}"
        for s in r.get("scripts") or ():
            text += f" {s['graph']}#{s['instance']}:{s['states']}"
        return text
    return f"{head} {kind} {json.dumps({k: v for k, v in r.items() if k not in ('seq', 't', 'thread', 'kind')})}"


def cmd_hook(args):
    p = connect(args)
    sets = {}
    for item in args.set:
        register, _, value = item.partition("=")
        sets[register] = int(value, 0)
    hook = p.hook(args.addr, name=args.name, reads=args.read, ret=args.ret, ret_reads=args.ret_read,
                  stack=args.stack, max_hits=args.max_hits, entry=not args.mid, set=sets)
    print(f"hook {hook['id']} at {hx(hook['address'])} ({hook['name']})")
    if not args.seconds:
        return
    out = Path(args.out) if args.out else out_path(f"hook{hook['id']}", ".jsonl")
    callers = Counter()
    count = 0
    try:
        with open(out, "w", encoding="utf-8") as f:
            for r in p.records(kinds=("hit", "return"), hooks=(hook["id"],), seconds=args.seconds):
                f.write(json.dumps(r) + "\n")
                if r.get("kind") == "hit":
                    count += 1
                    if r.get("caller"):
                        callers[r["caller"]] += 1
                if not args.quiet and count <= args.show:
                    print(describe_record(p, r))
    finally:
        if not args.keep:
            p.unhook(hook["id"])
    print(f"{count} hits in {args.seconds} s -> {out}")
    for caller, n in callers.most_common(10):
        print(f"  {n:6}  {p.describe(caller)}")


def cmd_unhook(args):
    connect(args).unhook(args.id)


def cmd_hooks(args):
    for h in connect(args).hooks():
        print(f"{h['id']:3} {h['address']:016X} hits={h['hits']} returns={h['returns']} {h['name']}"
              f"{' (frame sampler)' if h['frame'] else ''}")


def cmd_fetch(args):
    p = connect(args)
    got = p.fetch(args.since, kinds=args.kind, max=args.max)
    for r in got["records"]:
        print(describe_record(p, r))
    print(f"next {got['next']}, lost {got['lost']}", file=sys.stderr)


# The client's correction points (OwnerPrediction.md): the movement correction gate (returns 1 when it takes a server
# record that differs from its prediction and replays; [rdx+0x78] the record's frame), the owner chunk applier and the
# statescript rollback.
CORRECTION_HOOKS = (
    ("correction", 0x7FF78970F660, {"reads": ["[rdx+0x78]:u32"], "ret": True}),
    ("applier", 0x7FF78991E3C0, {}),
    ("rollback", 0x7FF78991F5D0, {}),
)


def trace(p, seconds, out, every=1, reads=(), input_text=None, log=print, scripts=(), camera=True,
          corrections=False, names=True):
    """Record frame samples (and any hook records) for `seconds` into a JSONL file; with `input_text` the input
    script starts with the trace; `scripts` graphs are recorded each frame (Probe.sampler); `corrections` hooks
    the client's correction points for the trace (`compare`). With `names` the file ends with a `names` record
    (the recorded states' classes and variables' names). Returns (frames, hits)."""
    p.sampler(every=every, reads=list(reads), scripts=list(scripts), camera=camera)
    seq = p.note(f"trace starts ({seconds} s)")
    hooks = []
    if corrections:
        for role, address, options in CORRECTION_HOOKS:
            hook = p.hook(address, name=role, **options)
            hooks.append(hook["id"])
            p.note(f"hook {hook['id']} {role}")
    if input_text:
        p.input(script.parse(input_text), clear=True)
    frames = hits = 0
    recorded = {}
    try:
        with open(out, "w", encoding="utf-8") as f:
            for r in p.records(seconds=seconds, since=seq - 1):
                f.write(json.dumps(r) + "\n")
                frames += r.get("kind") == "frame"
                hits += r.get("kind") == "hit"
                for s in r.get("scripts") or ():
                    entry = recorded.setdefault((s["entity"], s["graph"], s["instance"]),
                                                dict(s, vars={}, entity_vars={}))
                    entry["vars"].update(s.get("vars") or {})
                    entry["entity_vars"].update(s.get("entity_vars") or {})
            if names and recorded:
                from .statescript import name_scripts
                named = name_scripts(p, list(recorded.values()))
                f.write(json.dumps({"kind": "names", "scripts": [
                    {"entity": e, "graph": g, "instance": i, **v} for (e, g, i), v in named.items()]}) + "\n")
    finally:
        for hook in hooks:
            p.unhook(hook)
        p.sampler(every=0)
    log(f"{frames} frame samples ({frames / seconds:.1f}/s), {hits} hook hits in {seconds} s -> {out}")
    return frames, hits


def cmd_trace(args):
    p = connect(args)
    out = Path(args.out) if args.out else out_path("trace", ".jsonl")
    trace(p, args.seconds, out, args.every, args.read, args.input, scripts=args.script, camera=not args.no_camera,
          corrections=args.corrections)


def cmd_compare(args):
    from .compare import report
    report(args.server, args.trace, body=args.body, threshold=args.threshold, out=args.out)


def cmd_input(args):
    p = connect(args)
    steps = script.parse(args.script)
    print(p.input(steps, clear=args.clear))
    if args.wait:
        p.wait_input()


def cmd_frame(args):
    print(json.dumps(connect(args).frame(), indent=1))


def cmd_screenshot(args):
    p = connect(args)
    out = Path(args.out) if args.out else out_path("screenshot", ".png")
    width, height = screenshot(p.info()["pid"], out)
    print(f"{width}x{height} -> {out}")


def cmd_call(args):
    p = connect(args)
    values = [int(a, 0) for a in args.args]
    print(p.call(args.addr, *values, thread=args.thread, force=args.force))


def cmd_launch(args):
    from .session import Session
    with Session(orpheus=args.orpheus, game=args.game, port=args.port, enter_range=args.enter_range,
                 hero=args.hero, keep_state=args.keep_state, seed_state=args.seed_state,
                 rust_log=args.rust_log) as s:
        print(f"[owprobe] ready: PID {s.pid}, probe on 127.0.0.1:{s.port}; state {s.state}", flush=True)
        end = time.monotonic() + args.duration
        while time.monotonic() < end and s.proc.poll() is None:
            time.sleep(1)


def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m tools.owprobe", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, help="the probe's port (default ORPHEUS_PROBE_PORT or 21150)")
    sub = ap.add_subparsers(dest="command", required=True)

    def command(name, fn, help):
        p = sub.add_parser(name, help=help)
        p.set_defaults(fn=fn)
        return p

    command("info", cmd_info, "the process, image base, frame sampler state")
    command("modules", cmd_modules, "loaded modules")
    c = command("regions", cmd_regions, "the address space")
    c.add_argument("--start")
    c.add_argument("--end")
    c.add_argument("--all", action="store_true", help="also reserved and free regions")
    c = command("read", cmd_read, "hex dump, or one typed value (--type f32x3)")
    c.add_argument("addr")
    c.add_argument("len", type=number, nargs="?", default=64)
    c.add_argument("--type")
    c = command("write", cmd_write, "write hex bytes (code too, in the debug build)")
    c.add_argument("addr")
    c.add_argument("hex")
    c = command("eval", cmd_eval, "read expressions: '[c4+0x160]:f32x3' '[[c59+8]]'")
    c.add_argument("exprs", nargs="+")
    c = command("disasm", cmd_disasm, "disassemble the live code")
    c.add_argument("addr")
    c.add_argument("--count", type=int, default=20)
    c = command("scan", cmd_scan, "heap words equal to --value, or a code pattern")
    c.add_argument("--value", type=number)
    c.add_argument("--pattern")
    c.add_argument("--start")
    c.add_argument("--end")
    c.add_argument("--max", type=int, default=1000)
    c = command("components", cmd_components, "components with a vtable (or compN) and their entities")
    c.add_argument("vtable")
    c = command("component", cmd_component, "an entity's component N, as the client's own lookup finds it")
    c.add_argument("entity", help="the entity id in hex (A0000101), player or body")
    c.add_argument("kind", type=int, help="the component id (44)")
    c = command("entity-vars", cmd_entity_vars, "an entity's statescript entity variables (#Ne)")
    c.add_argument("entity", help="the entity id in hex (A0000100), player or body")
    c.add_argument("ids", nargs="*", help="variable ids (2438 or 2438e; default all)")
    c = command("hook", cmd_hook, "hook a function (or --mid a site) and log its hits")
    c.add_argument("addr")
    c.add_argument("--name")
    c.add_argument("--read", action="append", default=[], help="expression read at each hit (repeatable)")
    c.add_argument("--ret", action="store_true", help="also log the return (rax, xmm0)")
    c.add_argument("--ret-read", action="append", default=[], help="expression read at the return")
    c.add_argument("--stack", type=int, default=0, help="unwound callers to log")
    c.add_argument("--max-hits", type=int, default=0)
    c.add_argument("--mid", action="store_true", help="a site inside a function (no caller, no return)")
    c.add_argument("--set", action="append", default=[], help="REG=VALUE written at each hit")
    c.add_argument("--seconds", type=float, help="stream the hits for this long, then remove the hook")
    c.add_argument("--out", help="JSONL of the hits (default research/out/owprobe/hookN-*.jsonl)")
    c.add_argument("--keep", action="store_true", help="leave the hook in after --seconds")
    c.add_argument("--show", type=int, default=20, help="hits to print")
    c.add_argument("--quiet", action="store_true")
    c = command("unhook", cmd_unhook, "remove a hook")
    c.add_argument("id", type=int)
    command("hooks", cmd_hooks, "the hooks in place")
    c = command("fetch", cmd_fetch, "records from the ring buffer")
    c.add_argument("--since", type=int, default=0)
    c.add_argument("--kind", action="append", default=[])
    c.add_argument("--max", type=int, default=200)
    c = command("trace", cmd_trace, "frame samples (position, view, input) for N seconds into JSONL")
    c.add_argument("--seconds", type=float, default=5.0)
    c.add_argument("--out")
    c.add_argument("--every", type=int, default=1, help="record every Nth input sample")
    c.add_argument("--read", action="append", default=[], help="extra expression per frame ('[c4+0x170]:f32x3')")
    c.add_argument("--input", help="an input script to run with the trace (see tools/owprobe/script.py)")
    c.add_argument("--script", action="append", default=[],
                   help="a statescript graph to record each frame: 0D06 (the body's), player:0C90, "
                        "0D06:2083,11432 (only those variables), player:288B:1962,31435e (e: the entity's "
                        "entity variables)")
    c.add_argument("--no-camera", action="store_true", help="don't read the camera")
    c.add_argument("--corrections", action="store_true",
                   help="hook the movement correction gate, the owner chunk applier and the rollback (for compare)")
    c = command("compare", cmd_compare, "the server's orpheus::pose trace against a probe trace, frame by frame")
    c.add_argument("--server", required=True, help="the server's log with orpheus::pose debug lines (play.log)")
    c.add_argument("--trace", required=True, help="a probe trace (trace --corrections)")
    c.add_argument("--body", help="the body's entity id in hex (default the first in the log)")
    c.add_argument("--threshold", type=float, default=0.01, help="print frames farther apart than this (m)")
    c.add_argument("--out", help="JSONL of every compared frame")
    c = command("input", cmd_input, "queue an input script (see tools/owprobe/script.py)")
    c.add_argument("script")
    c.add_argument("--clear", action="store_true", help="drop the queued steps and held input first")
    c.add_argument("--wait", action="store_true", help="return when the script is done")
    command("frame", cmd_frame, "the latest frame sample")
    c = command("screenshot", cmd_screenshot, "PNG of the game window")
    c.add_argument("--out")
    c = command("call", cmd_call, "call a client function (integers; floats as bits)")
    c.add_argument("addr")
    c.add_argument("args", nargs="*")
    c.add_argument("--thread", choices=("frame", "probe"), default="frame")
    c.add_argument("--force", action="store_true", help="also a target that is not a function start")
    c = command("launch", cmd_launch, "orpheus play --probe on a temp state copy (debug build), optionally into "
                                      "the Practice Range; stays up for --duration")
    c.add_argument("--orpheus", help="orpheus.exe with owwfd_relay.dll and owprobe.dll next to it (ORPHEUS_BIN)")
    c.add_argument("--game", help=f"the game (default {paths.GAME_EXE})")
    c.add_argument("--enter-range", action="store_true")
    c.add_argument("--hero")
    c.add_argument("--duration", type=float, default=600)
    c.add_argument("--keep-state", action="store_true")
    c.add_argument("--seed-state", help="state folder to copy the derived game data and profiles from "
                                        "(default %%APPDATA%%/orpheus, read only)")
    c.add_argument("--rust-log", help="the server's RUST_LOG (info,orpheus::pose=debug for compare's server poses "
                                      "in <state>/play.log; with --keep-state)")
    args = ap.parse_args(argv)
    try:
        args.fn(args)
    except ProbeError as e:
        sys.exit(f"owprobe: {e}")
    except ConnectionRefusedError:
        sys.exit("owprobe: no probe listening (orpheus play --probe, or python -m tools.owprobe launch)")


if __name__ == "__main__":
    main()
