"""Crash report: registers, named stack, disassembly and a minidump at a second-chance exception
(or at first-chance exceptions you choose).

usage: python -m tools.owdbg.crash [--on CODE[@SITE]] ... [--duration S] [--out-dir DIR]
           [--full-dump] [--no-dump] [--continue] [--max-reports N] [session options]

Second-chance exceptions always report. --on C0000005 also reports first-chance access
violations; --on C0000005@SITE only those raised inside the function containing SITE (a game
name/RVA/VA as in probe). Exceptions the protector raises for itself (C0000096, 80000004,
C000001D, handled C0000005) are passed to the game as usual and only counted. After a report the
process is terminated, unless --continue (the exception is then passed on). Writes
<out-dir>/crash-N.json, crash-N.txt and crash-N.dmp (default research/out/owdbg/crash-<time>/).
"""
import argparse
import ctypes
import json
import os

from . import win32 as w
from .disasm import disasm_around
from .game import Session, add_session_args, out_path
from .names import hx


def parse_on(spec, names):
    code, _, site = spec.partition("@")
    return int(code, 16), (names.parse(site) if site else None)


class Crash:
    def __init__(self, s, args):
        self.s, self.args = s, args
        self.dbg, self.names = s.dbg, s.names
        self.on = [parse_on(x, self.names) for x in args.on]
        self.reports = []
        self.out_dir = args.out_dir

    def wanted(self, ev):
        for code, site in self.on:
            if ev.code != code:
                continue
            if site is None:
                return True
            target = self.names.resolve(site)
            if target is None:
                continue
            mod = self.dbg.module_at(ev.address)
            if ev.address == target:
                return True
            if mod is not None and mod.pe is not None and mod.base <= target < mod.base + mod.size:
                f1 = mod.pe.function_start(ev.address - mod.base)
                f2 = mod.pe.function_start(target - mod.base)
                if f1 is not None and f1 == f2:
                    return True
        return False

    def on_exception(self, ev):
        if self.wanted(ev):
            return self.report(ev)
        return None

    def on_crash(self, ev):
        return self.report(ev)

    def report(self, ev):
        dbg, names = self.dbg, self.names
        n = len(self.reports) + 1
        os.makedirs(self.out_dir, exist_ok=True)
        base = os.path.join(self.out_dir, f"crash-{n}")
        th = ev.thread
        rep = {"exception": f"{ev.code:08X}", "name": ev.name, "first_chance": ev.first_chance,
               "address": names.describe(ev.address), "access": ev.describe_access(),
               "params": [hx(p) for p in ev.params], "tid": ev.tid, "thread": th.name if th else None,
               "t": round(dbg.elapsed, 3)}
        ctx = None
        if th is not None:
            ctx = w.new_context(w.CONTEXT_ALL)
            if w.GetThreadContext(th.handle, ctypes.byref(ctx)):
                rep["registers"] = {k: hx(v) for k, v in
                                    ((r, getattr(ctx, w.CONTEXT_FIELD[r])) for r in w.GPR_NAMES + ("rip", "eflags"))}
                rep["registers_named"] = {r: names.text(int(v, 16)) for r, v in rep["registers"].items()
                                          if dbg.module_at(int(v, 16))}
                frames = dbg.stack(ctx, self.args.frames)
                rep["stack"] = [dict(names.describe(va), rsp=hx(sp), method=m) for va, sp, m in frames]
            else:
                ctx = None
        rep["disasm"] = [{"va": hx(a), "where": names.text(a), "insn": t, "current": cur}
                         for a, s, t, cur in disasm_around(dbg, ev.address, 8, 6)]
        rep["modules"] = [{"name": m.name, "base": hx(m.base), "size": hx(m.size), "path": m.path}
                          for m in sorted(dbg.modules.values(), key=lambda m: m.base)]
        if not self.args.no_dump:
            try:
                size = dbg.write_minidump(base + ".dmp", ev.tid, ev.record, ctx, full=self.args.full_dump)
                rep["minidump"] = {"path": base + ".dmp", "bytes": size, "full": self.args.full_dump}
            except Exception as e:
                rep["minidump"] = {"error": str(e)}
        with open(base + ".json", "w", encoding="utf-8") as f:
            json.dump(rep, f, indent=1)
        text = format_report(rep)
        with open(base + ".txt", "w", encoding="utf-8") as f:
            f.write(text)
        self.s.log(text)
        self.s.log(f"[crash] report -> {base}.json/.txt" + (f", dump {base}.dmp" if not self.args.no_dump else ""))
        self.reports.append(rep)
        if not self.args.cont or len(self.reports) >= self.args.max_reports:
            dbg.terminate()
        return None  # pass the exception on (the default)


def format_report(rep):
    lines = [f"{rep['name']} ({rep['exception']}) {'first' if rep['first_chance'] else 'second'} chance at "
             f"{rep['address']['text']} [{rep['address']['va']}" +
             (f", rva {rep['address']['rva']}" if rep['address'].get('rva') else "") + "]" +
             (f", {rep['access']}" if rep.get("access") else "") +
             f", thread {rep['tid']}" + (f" '{rep['thread']}'" if rep.get("thread") else "") +
             f", t={rep['t']}s"]
    regs = rep.get("registers") or {}
    names = list(regs)
    for i in range(0, len(names), 4):
        lines.append("  " + "  ".join(f"{r:>3}={regs[r]:>18}" for r in names[i:i + 4]))
    for r, t in (rep.get("registers_named") or {}).items():
        lines.append(f"  {r} -> {t}")
    lines.append("stack:")
    for i, f in enumerate(rep.get("stack") or []):
        extra = f" (lucy {f['lucy']})" if f.get("lucy") else ""
        lines.append(f"  #{i:<2} {f['va']:>18} {f['text']}{extra} [{f['method']}]")
    lines.append("disassembly:")
    for d in rep["disasm"]:
        lines.append(f"  {'=>' if d['current'] else '  '} {d['va']:>18} {d['insn']:<40} {d['where']}")
    return "\n".join(lines) + "\n"


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--on", action="append", default=[], help="also report first-chance CODE[@SITE] (hex code)")
    p.add_argument("--out-dir", help="report directory (default research/out/owdbg/crash-<time>)")
    p.add_argument("--full-dump", action="store_true", help="minidump with all memory (GBs for the game)")
    p.add_argument("--no-dump", action="store_true", help="no minidump")
    p.add_argument("--frames", type=int, default=48, help="max stack frames")
    p.add_argument("--continue", dest="cont", action="store_true", help="keep running after a report")
    p.add_argument("--max-reports", type=int, default=5, help="with --continue: stop after N reports")
    add_session_args(p)
    args = p.parse_args(argv)
    args.out_dir = args.out_dir or str(out_path("crash", ""))

    with Session(args) as s:
        c = Crash(s, args)
        s.dbg.on("exception", c.on_exception)
        s.dbg.on("crash", c.on_crash)
        s.start()
        alive = s.run(args.duration)
        s.log(f"[crash] {len(c.reports)} report(s); alive={alive} t={s.dbg.elapsed:.0f}s")
        s.log_exceptions()


if __name__ == "__main__":
    main()
