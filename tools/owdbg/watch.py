"""Hardware read/write watch: which instructions (and functions) access an address or a field.

usage: python -m tools.owdbg.watch --addr SPEC --size N --access r|w|rw [options]
       python -m tools.owdbg.watch --arm-at SITE --field EXPR:SIZE --access r|w|rw [--arm-count N]
           [--this-vtable X] [--arm-if EXPR=VALUE ...] [options]
options: [--duration 60] [--out FILE.jsonl] [--max-hits 5000] [--arm-delay S] [--stack N] [session options]

--addr: a game global (name, RVA, VA at 0x140000000/0x7FF788F10000) or va:0x... (absolute).
--arm-at/--field: when SITE (a breakpoint, as in probe) hits, the watch moves to EXPR evaluated in
that thread's registers, e.g. --arm-at Eval_STU_76F2C784 --field rcx+0x48:8 (EXPR: registers,
numbers, + -, [deref]). The first N hits arm (default 1), then the breakpoint is removed.
--this-vtable X / --arm-if EXPR=VALUE (or EXPR!=VALUE; repeatable, all must hold): only hits that
meet the condition arm (and count for --arm-count), e.g. --this-vtable 0x7FF78B531228 arms a
shared begin only for that state class: [rcx] == X. EXPR as in --field (no size: its value, ':N'
reads N bytes); VALUE a number (a Lucy or IDB VA in the game image is moved to this run's
base) or a game name. Hits that don't match are counted in the summary ("handler hits not armed").
SIZE up to 32 bytes (split into aligned 8/4/2/1-byte debug registers, at most 4). Access 'r'
traps reads and writes (x86 has no read-only watch) and keeps reads; 'w' writes; 'rw' both.
Debug registers are written to every thread (also later ones) after --hw-delay (default 8 s for
the game) and checked every 2 s (the protector rewrites them during startup). A data trap reports
the next instruction; the accessing one is found among: the instruction before RIP decoded from
the .pdata function start, a call whose return address is at [rsp] (calls trap at their target),
and back-scanned single instructions; the one whose memory operand (computed from the thread's
registers) overlaps the watch is 'verified' (unverified rows are marked '?'). Each hit records
that instruction, its function name, the value after the access and the thread. Output JSONL + a
summary per instruction (last line).
"""
import argparse
import json
import re
from collections import Counter, defaultdict

from .disasm import access_insn
from . import actions
from .game import Session, add_session_args, out_path
from .names import IDA_BASE, LUCY_BASE, Expr, field_spec, hx, split_ranges


class Watch:
    def __init__(self, s, args):
        self.s, self.args = s, args
        self.dbg, self.names = s.dbg, s.names
        self.out = open(args.out, "w", encoding="utf-8")
        self.slots = []
        self.range = None  # (va, size)
        self.counts = Counter()
        self.info = {}  # insn va -> dict
        self.samples = defaultdict(list)
        self.threads = defaultdict(Counter)
        self.filtered = 0
        self.hits = 0
        self.arms = 0
        self.skipped = Counter()  # arm hits that did not arm, by reason
        self.arm_token = None
        self._insn_cache = {}
        if args.field:
            self.field_expr, self.field_size = field_spec(args.field)
            self.arm_site = self.names.parse(args.arm_at)
        else:
            self.addr = self.names.parse(args.addr)
        self.conditions = []  # (Expr, op, value text, value)
        for text in args.arm_if + [f"[rcx]={x}" for x in args.this_vtable]:
            expr, op, value = parse_condition(text)
            self.conditions.append([expr, op, value, None])

    def start(self):
        if self.args.field:
            self.arm()
        else:
            self.set_watch(self.names.resolve(self.addr), self.args.size)

    def arm(self):
        if self.arm_token is None:
            for cond in self.conditions:
                cond[3] = self.condition_value(cond[2])
                self.s.log(f"[watch] arms only when {cond[0].text} {cond[1]} 0x{cond[3]:X}")
            va = self.names.resolve(self.arm_site)
            self.arm_token = self.dbg.add_bp(va, self.on_arm)
            self.s.log(f"[watch] arm breakpoint at {self.names.text(va)} (0x{va:X})")

    def condition_value(self, text):
        """A number (a Lucy/IDB VA in the game image -> this run's VA) or a game name."""
        try:
            v = int(text, 0)
        except ValueError:
            return self.names.resolve(self.names.parse(text))
        main = self.dbg.main
        for base in (LUCY_BASE, IDA_BASE):
            if base <= v < base + main.size:
                return main.base + v - base
        return v

    def conditions_hold(self, hit):
        for expr, op, _, value in self.conditions:
            got = expr.number(hit)
            if (got == value) != (op == "="):
                return False
        return True

    def set_watch(self, va, size):
        self.clear_watch()
        ranges = split_ranges(va, size)
        if len(ranges) > 4:
            raise SystemExit(f"[watch] {size} bytes at 0x{va:X} need {len(ranges)} debug registers (max 4)")
        kind = "w" if self.args.access == "w" else "rw"
        for a, n in ranges:
            self.slots.append(self.dbg.set_hw(a, kind, n, self.on_hit))
        self.range = (va, size)
        self.s.log(f"[watch] watching {size} bytes at 0x{va:X} ({self.names.text(va)}) access {self.args.access}")

    def clear_watch(self):
        for slot in self.slots:
            self.dbg.clear_hw(slot)
        self.slots = []

    def on_arm(self, hit):
        if self.conditions and not self.conditions_hold(hit):
            self.skipped["arm condition false"] += 1
            return
        va = self.field_expr.address(hit) if self.field_expr.size else self.field_expr._eval(self.field_expr.tree, hit)
        self.arms += 1
        ret = hit.return_address()
        self.emit({"event": "arm", "t": round(hit.time, 4), "tid": hit.tid, "address": hx(va),
                   "at": self.names.text(hit.va), "caller": self.names.text(ret)})
        self.set_watch(va, self.field_size)
        if self.arms >= self.arm_count and self.arm_token:
            self.dbg.remove_bp(self.arm_token)
            self.arm_token = None

    @property
    def arm_count(self):
        return self.args.arm_count if self.args.arm_count is not None else 1

    def on_hit(self, hit):
        rip = hit.rip
        acc = self._insn_cache.get(rip)
        if acc is None:
            watched = [(self.dbg.hw[i].va, self.dbg.hw[i].size) for i in self.slots if self.dbg.hw[i]]
            acc = access_insn(self.dbg, rip, hit.regs(), watched) or (None, "?", "?", False)
            if acc[3]:  # verified: the same RIP is the same instruction
                self._insn_cache[rip] = acc
        insn_va, text, kind, verified = acc
        if self.args.access == "r" and "r" not in kind and kind != "?":
            self.filtered += 1
            return
        self.hits += 1
        key = insn_va if insn_va is not None else rip
        if key not in self.info:
            d = self.names.describe(key)
            self.info[key] = {"insn_va": hx(key), "rva": d.get("rva"), "lucy": d.get("lucy"),
                              "where": d["text"], "function": d.get("name"), "func_rva": d.get("func_rva"),
                              "insn": text, "kind": kind, "verified": verified}
        self.counts[key] += 1
        self.threads[key][hit.thread.name or hit.tid] += 1
        va, size = self.range
        data = self.dbg.try_read(va, size)
        value = data.hex() if data is not None else None
        if value not in self.samples[key] and len(self.samples[key]) < 5:
            self.samples[key].append(value)
        rec = {"event": "hit", "t": round(hit.time, 4), "tid": hit.tid, "thread": hit.thread.name,
               "insn_va": hx(key), "where": self.info[key]["where"], "insn": text, "kind": kind,
               "rip_after": hx(rip), "value": value}
        if self.args.stack:
            rec["stack"] = [self.names.text(f[0]) for f in self.dbg.stack(hit.ctx, self.args.stack)[1:]]
        self.emit(rec)
        if self.hits >= self.args.max_hits:
            self.clear_watch()
            self.s.log(f"[watch] {self.hits} hits: watch removed (--max-hits)")

    def emit(self, rec):
        self.out.write(json.dumps(rec) + "\n")

    def finish(self):
        rows = []
        for key, n in self.counts.most_common():
            rows.append(dict(self.info[key], hits=n, samples=self.samples[key],
                             threads=[[str(k), v] for k, v in self.threads[key].most_common(5)]))
        summary = {"range": [hx(self.range[0]), self.range[1]] if self.range else None,
                   "access": self.args.access, "hits": self.hits, "filtered_writes": self.filtered,
                   "arms": self.arms, "hw_rewrites": self.dbg.hw_fixes, "instructions": rows,
                   "alive": self.dbg.alive, "duration": round(self.dbg.elapsed, 1)}
        self.out.write(json.dumps({"summary": summary}) + "\n")
        self.out.close()
        return summary


def parse_condition(text):
    """'EXPR=VALUE' / 'EXPR==VALUE' / 'EXPR!=VALUE' -> (Expr, '=' or '!=', value text)."""
    m = re.fullmatch(r"\s*(.+?)\s*(==|!=|=)\s*(\S+)\s*", text)
    if not m:
        raise ValueError(f"--arm-if {text!r}: give EXPR=VALUE or EXPR!=VALUE")
    return Expr(m.group(1)), "!=" if m.group(2) == "!=" else "=", m.group(3)


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--addr", help="address to watch (game name/RVA/VA, or va:0x...)")
    p.add_argument("--size", type=int, default=8, help="bytes at --addr (default 8)")
    p.add_argument("--arm-at", help="breakpoint that arms the --field watch")
    p.add_argument("--field", help="EXPR:SIZE evaluated when --arm-at hits, e.g. rcx+0xA8:8")
    p.add_argument("--arm-count", type=int, help="re-arm on the first N hits of --arm-at (default 1)")
    p.add_argument("--access", choices=("r", "w", "rw"), default="rw")
    p.add_argument("--arm-delay", type=float, default=0, help="set up N seconds after start instead of at once")
    p.add_argument("--stack", type=int, default=0, help="record N unwound frames per hit")
    p.add_argument("--max-hits", type=int, default=5000, help="remove the watch after N hits")
    p.add_argument("--arm-if", action="append", default=[], metavar="EXPR=VALUE",
                   help="arm only when EXPR equals VALUE (EXPR!=VALUE: differs); repeatable, all must hold")
    p.add_argument("--this-vtable", action="append", default=[], metavar="X",
                   help="arm only when [rcx] == X (the hit object's vtable), as --arm-if '[rcx]=X'")
    p.add_argument("--out", help="JSONL output (default research/out/owdbg/watch-<time>.jsonl)")
    actions.add_action_args(p)
    add_session_args(p, duration=60)
    args = p.parse_args(argv)
    if bool(args.addr) == bool(args.field) or bool(args.field) != bool(args.arm_at):
        p.error("give --addr, or --arm-at with --field")
    try:
        for text in args.arm_if + [f"[rcx]={x}" for x in args.this_vtable]:
            parse_condition(text)
    except ValueError as e:
        p.error(str(e))
    args.out = args.out or str(out_path("watch", ".jsonl"))
    actions.prepare(args)

    with Session(args) as s:
        wt = Watch(s, args)
        act = actions.start(s, args)
        if args.arm_delay:
            s.dbg.after(args.arm_delay, wt.start)
        else:
            s.dbg.on("system_breakpoint", wt.start)
        s.start()
        s.run(args.duration)
        summary = wt.finish()
        s.log(f"[watch] alive={s.dbg.alive} t={s.dbg.elapsed:.0f}s hits={summary['hits']} "
              f"(filtered writes {summary['filtered_writes']}, debug registers rewritten "
              f"{summary['hw_rewrites']}x, arms {summary['arms']}) -> {args.out}")
        if act is not None:
            s.log(f"[actions] {act.summary()}")
        if wt.skipped:
            s.log(f"[watch] handler hits not armed: {dict(wt.skipped)}")
        for r in summary["instructions"][:30]:
            s.log(f"  {r['hits']:>6}  {r['kind']:<2}{'' if r['verified'] else '?'} {r['insn_va']:>16} "
                  f"{r['where']:<40} {r['insn']}   samples {r['samples'][:2]}")


if __name__ == "__main__":
    main()
