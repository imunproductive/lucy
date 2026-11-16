"""Logging breakpoints: arguments, memory reads, caller and return value per hit, without pausing.

usage: python -m tools.owdbg.probe [site options] --at SITE [site options] [--at SITE ...]
           [--duration 30] [--out FILE.jsonl] [--arm-delay S] [session options]
       python -m tools.owdbg.probe --msg ID [--send JSON [--send-every S]] [options]
site options: [--read '[rcx+0x18]:8' ...] [--ret-read '[rax]:8' ...] [--no-ret] [--stack N]
           [--stack-args N] [--caller-in SITE ...] [--max-hits 2000] [--max-events N]

Site options given after an --at belong to that site only; given before the first --at they
apply to every site (and to the --msg site): --read/--ret-read add to the global ones, the
others replace them. E.g. `--at A --read "[rdx]:16" --at B --no-ret` reads [rdx] at A only and
keeps B's return breakpoint off.

SITE: a game name (index or docs/analysis, e.g. Ingame_Watchdog), an RVA, a VA at the IDB base
0x140000000 or the ORPHEUS base 0x7FF788F10000, 'module!export' or 'va:0x...'. Breakpoints are
armed at the system breakpoint (or --arm-delay seconds after start); a one-shot breakpoint on
the return address records rax. --caller-in keeps only calls whose return address lies in the
function containing SITE (e.g. --at kernel32!WaitForSingleObjectEx --caller-in Ingame_Watchdog:
the watchdog's 2.5 s wait; Ingame_Watchdog itself is entered once and loops). Output: one JSON
line per call ({site, rva, t, tid, thread, args{rcx,rdx,r8,r9}, stack_args, reads, caller, stack,
ret, ret_reads, dt_ms}; calls still running at the end have "ret": null, "returned": false) and a
summary per site (hits, callers, return values, threads) printed and written as the last line.
A site hit more than --max-hits times is disarmed (each hit is a debug event: hot functions slow
the game down). Calls owdbg itself makes in the game (--actions' set-view) are not recorded
(counted as "injected" in the summary).
--msg ID adds the message's client handler (tools.owindex) as site "msg ID": each hit records
"message": {reg, object, fields (decoded from the index layout), markers}; the message object is
the argument register whose [reg] is the message class vtable (r8 for every dispatcher so far),
hits of other ids sharing the handler are skipped. --send makes ORPHEUS send the message with
marker values (see tools/owdbg/msg.py) and logs the handler hits that carry them.
"""
import argparse
import json
import os
import time
from collections import Counter, defaultdict

from . import actions
from .game import Session, add_session_args, out_path
from .msg import Message, Sender, add_msg_args, decode, message_reg
from .names import Expr, hx

ARG_REGS = ("rcx", "rdx", "r8", "r9")
# Options that an --at given before them binds to its site: dest -> (kind, default).
SITE_OPTIONS = {"read": ("list", None), "ret_read": ("list", None), "caller_in": ("list", None),
                "no_ret": ("flag", False), "stack": ("int", 0), "stack_args": ("int", 0),
                "max_hits": ("int", 2000), "max_events": ("int", 200000)}


class _SiteAt(argparse.Action):
    """--at: a new site; the site options that follow bind to it."""

    def __call__(self, parser, ns, value, option_string=None):
        ns.at = list(ns.at or []) + [{"spec": value}]


class _SiteOption(argparse.Action):
    """A site option: the last --at's when one came before it, else global."""

    def __call__(self, parser, ns, value, option_string=None):
        kind = SITE_OPTIONS[self.dest][0]
        if kind == "flag":
            value = True
        if ns.at:
            target = ns.at[-1]
            if kind == "list":
                target.setdefault(self.dest, []).append(value)
            else:
                target[self.dest] = value
        elif kind == "list":
            setattr(ns, self.dest, list(getattr(ns, self.dest) or []) + [value])
        else:
            setattr(ns, self.dest, value)


class Site:
    """One breakpoint site with its options (global ones merged in)."""

    def __init__(self, label, parsed, args, own=None):
        own = own or {}
        self.label, self.parsed = label, parsed
        self.reads = [Expr(t) for t in args.read + own.get("read", [])]
        self.ret_reads = [Expr(t) for t in args.ret_read + own.get("ret_read", [])]
        self.caller_specs = own.get("caller_in", args.caller_in)
        for name in ("no_ret", "stack", "stack_args", "max_hits", "max_events"):
            setattr(self, name, own.get(name, getattr(args, name)))
        self.caller_funcs = set()  # VAs of the functions calls must come from (--caller-in)

    def options(self):
        """The options that differ from the defaults, for the summary."""
        out = {}
        if self.reads:
            out["read"] = [e.text for e in self.reads]
        if self.ret_reads:
            out["ret_read"] = [e.text for e in self.ret_reads]
        for name, (_, default) in SITE_OPTIONS.items():
            value = self.caller_specs if name == "caller_in" else getattr(self, name, None)
            if name not in ("read", "ret_read") and value not in (default, [], None):
                out[name] = value
        return out


class Probe:
    def __init__(self, s, args):
        self.s, self.args = s, args
        self.dbg, self.names = s.dbg, s.names
        self.sites = []
        for own in args.at:
            site = Site(own["spec"], self.names.parse(own["spec"]), args, own)
            if any(x.label == site.label for x in self.sites):
                site.label += f" #{len(self.sites) + 1}"  # the same SITE twice, other options
            for spec in site.caller_specs:
                self.names.parse(spec)  # unknown names fail here, not at arm time
            self.sites.append(site)
        self.msg = self.msg_label = self.sender = None
        self.marker_hits = []  # (t, {path: matched})
        if args.msg is not None:
            self.msg = Message(self.names.idx, args.msg, args.msg_link)
            self.msg_label = f"msg {args.msg}"
            self.sites.append(Site(self.msg_label, ("rva", self.msg.handler_rva), args))
            if args.send:
                self.sender = Sender(s, args, self.msg)
        self.out = open(args.out, "w", encoding="utf-8")
        self.counts = Counter()
        self.callers = defaultdict(Counter)
        self.rets = defaultdict(Counter)
        self.threads = defaultdict(Counter)
        self.tokens = {}  # Site -> breakpoint token
        self.waiters = {}  # return address -> [(tid, rsp, record, token)]
        self.ret_tokens = {}
        self.pending = 0
        self.next_id = 1
        self._caller_func = {}  # return address -> containing function VA
        self.filtered = Counter()
        self.injected = Counter()

    def func_of(self, va):
        f = self._caller_func.get(va)
        if f is None:
            mod = self.dbg.module_at(va)
            start = mod.pe.function_start(va - mod.base) if mod is not None and mod.pe else None
            f = self._caller_func[va] = (mod.base + start) if start is not None else 0
        return f

    def arm(self):
        for site in self.sites:
            for spec in site.caller_specs:
                va = self.names.resolve(self.names.parse(spec))
                if va is not None and self.func_of(va):
                    site.caller_funcs.add(self.func_of(va))
            va = self.names.resolve(site.parsed)
            if va is None:
                self.s.log(f"[probe] {site.label}: module not loaded yet; skipped")
                continue
            self.tokens[site] = self.dbg.add_bp(va, lambda hit, site=site: self.on_call(hit, site))
            opts = site.options()
            self.s.log(f"[probe] armed {site.label} at 0x{va:X} ({self.names.text(va)})"
                       + (f" {opts}" if opts else ""))
        if self.sender is not None:
            self.sender.start()

    def on_message(self, hit):
        """Message object, decoded fields and marker matches at a --msg handler hit; None when no
        argument register holds an object of the message's class (a handler shared with other ids)."""
        reg = message_reg(self.dbg, hit, self.dbg.main.base, self.msg)
        if reg is None:
            return None
        obj = hit.reg(reg)
        mrec = {"reg": reg, "object": hx(obj), "fields": decode(self.dbg, obj, self.msg.fields)}
        if self.sender is not None and self.sender.markers:
            match = {p: self.dbg.try_read(obj + m["off"], m["size"]) == m["bytes"]
                     for p, m in self.sender.markers.items()}
            mrec["markers"] = match
            if any(match.values()):
                self.marker_hits.append((round(hit.time, 1), reg, match))
                self.s.log(f"[probe] t={hit.time:.1f}s message {self.msg.id} with the marker: {reg}=0x{obj:X} "
                           f"{match} fields {json.dumps(mrec['fields'])[:300]}")
        return mrec

    def on_call(self, hit, site):
        dbg = self.dbg
        rsp = hit.rsp
        ret = dbg.read_u64(rsp)
        label = site.label
        if ret in dbg.injected_returns:
            self.injected[label] += 1  # owdbg's own call (actions' set-view): not the game's
            return
        if site.caller_specs and self.func_of(ret) not in site.caller_funcs:
            self.filtered[label] += 1
            if self.filtered[label] >= site.max_events and site in self.tokens:
                dbg.remove_bp(self.tokens.pop(site))
                self.s.log(f"[probe] {label}: {self.filtered[label]} calls from other callers, "
                           "breakpoint removed (--max-events)")
            return
        mrec = None
        if label == self.msg_label:
            mrec = self.on_message(hit)
            if mrec is None:
                self.filtered[label] += 1
                return
        n = self.counts[label] = self.counts[label] + 1
        if n >= site.max_hits and site in self.tokens:
            dbg.remove_bp(self.tokens.pop(site))
            self.s.log(f"[probe] {label}: {n} hits, breakpoint removed (--max-hits)")
        va = hit.va
        caller = self.names.describe(ret)
        rec = {"id": self.next_id, "site": label, "rva": self.names.describe(va).get("rva"),
               "t": round(hit.time, 4), "tid": hit.tid, "thread": hit.thread.name,
               "args": {r: hx(hit.reg(r)) for r in ARG_REGS},
               "caller": caller["text"], "caller_va": hx(ret), "caller_rva": caller.get("rva")}
        self.next_id += 1
        if site.stack_args:
            vals = []
            for i in range(site.stack_args):
                try:
                    vals.append(hx(dbg.read_u64(rsp + 0x28 + 8 * i)))
                except Exception:
                    vals.append(None)
            rec["stack_args"] = vals
        if site.reads:
            rec["reads"] = {e.text: e.value(hit) for e in site.reads}
        if site.stack:
            rec["stack"] = [self.names.text(f[0]) for f in dbg.stack(hit.ctx, site.stack)[1:]]
        if mrec is not None:
            rec["message"] = mrec
        self.callers[label][caller["text"]] += 1
        self.threads[label][hit.thread.name or hit.tid] += 1
        if site.no_ret:
            self.emit(rec)
            return
        rec["_t0"] = time.monotonic()
        lst = self.waiters.setdefault(ret, [])
        lst.append((hit.tid, rsp + 8, rec, site))
        if ret not in self.ret_tokens:
            self.ret_tokens[ret] = dbg.add_bp(ret, self.on_return)
        self.pending += 1

    def on_return(self, hit):
        lst = self.waiters.get(hit.va)
        if not lst:
            return
        for i, (tid, rsp, rec, site) in enumerate(lst):
            if tid == hit.tid and rsp == hit.rsp:
                del lst[i]
                self.pending -= 1
                rax = hit.rax
                rec["ret"] = hx(rax)
                rec["dt_ms"] = round((time.monotonic() - rec.pop("_t0")) * 1000, 3)
                if site.ret_reads:
                    rec["ret_reads"] = {e.text: e.value(hit) for e in site.ret_reads}
                self.rets[rec["site"]][hx(rax)] += 1
                self.emit(rec)
                break
        if not lst:
            del self.waiters[hit.va]
            self.dbg.remove_bp(self.ret_tokens.pop(hit.va))

    def emit(self, rec):
        self.out.write(json.dumps(rec) + "\n")

    def finish(self):
        for lst in self.waiters.values():
            for tid, rsp, rec, _ in lst:
                rec.pop("_t0", None)
                rec["ret"] = None
                rec["returned"] = False
                self.emit(rec)
        summary = {}
        for site in self.sites:
            spec = site.label
            summary[spec] = {
                "address": self.names.spec_text(site.parsed), "hits": self.counts[spec],
                "filtered_out": self.filtered[spec], "injected": self.injected[spec],
                "options": site.options(),
                "callers": self.callers[spec].most_common(10), "returns": self.rets[spec].most_common(10),
                "threads": [[str(k), v] for k, v in self.threads[spec].most_common(10)],
            }
        extra = {}
        if self.msg is not None:
            extra["message"] = {"id": self.msg.id, "link": self.msg.endpoint_link,
                                "handler_rva": hx(self.msg.handler_rva), "marker_hits": self.marker_hits}
        if self.sender is not None:
            extra["sends"] = self.sender.history
            extra["send_errors"] = self.sender.errors
        self.out.write(json.dumps({"summary": summary, "alive": self.dbg.alive,
                                   "duration": round(self.dbg.elapsed, 1),
                                   "exceptions_passed": self.dbg.summary(), **extra}) + "\n")
        self.out.close()
        return summary


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--at", action=_SiteAt, default=[],
                   help="breakpoint site (repeatable); the site options after it are its own")
    site = p.add_argument_group("site options (after an --at: that site's; before any --at: every site's)")
    site.add_argument("--read", action=_SiteOption, default=[],
                      help="memory expression at entry, e.g. '[rcx+0x18]:8' (repeatable)")
    site.add_argument("--ret-read", action=_SiteOption, default=[],
                      help="memory expression at return, e.g. '[rax]:8' (repeatable)")
    site.add_argument("--stack-args", action=_SiteOption, type=int, default=0, help="stack arguments 5.. to record")
    site.add_argument("--stack", action=_SiteOption, type=int, default=0,
                      help="record N unwound frames per hit (slower)")
    site.add_argument("--no-ret", action=_SiteOption, nargs=0, default=False, help="no return breakpoint")
    site.add_argument("--max-hits", action=_SiteOption, type=int, default=2000,
                      help="remove the site's breakpoint after N hits")
    site.add_argument("--caller-in", action=_SiteOption, default=[],
                      help="record only calls whose return address lies in the function containing SITE "
                           "(repeatable)")
    site.add_argument("--max-events", action=_SiteOption, type=int, default=200000,
                      help="with --caller-in: remove the site's breakpoint after N skipped calls")
    p.add_argument("--arm-delay", type=float, default=0, help="arm N seconds after start instead of at once")
    p.add_argument("--out", help="JSONL output (default research/out/owdbg/probe-<time>.jsonl)")
    add_msg_args(p)
    actions.add_action_args(p)
    add_session_args(p, duration=30)
    args = p.parse_args(argv)
    if not args.at and args.msg is None:
        p.error("give --at and/or --msg")
    if args.send and args.msg is None:
        p.error("--send needs --msg")
    if args.send:
        args.server_env = dict(os.environ, ORPHEUS_DEBUG_SEND="1")
    for text in args.read + args.ret_read + [t for own in args.at for t in own.get("read", []) + own.get("ret_read", [])]:
        try:
            Expr(text)
        except ValueError as e:
            p.error(f"--read/--ret-read {text!r}: {e}")
    args.out = args.out or str(out_path("probe", ".jsonl"))
    actions.prepare(args)

    with Session(args) as s:
        probe = Probe(s, args)
        act = actions.start(s, args)
        if args.arm_delay:
            s.dbg.after(args.arm_delay, probe.arm)
        else:
            s.dbg.on("system_breakpoint", probe.arm)
        s.start()
        s.run(args.duration)
        summary = probe.finish()
        s.log(f"[probe] alive={s.dbg.alive} t={s.dbg.elapsed:.0f}s -> {args.out}")
        for spec, d in summary.items():
            s.log(f"  {spec} = {d['address']}: {d['hits']} hits"
                  + (f" ({d['filtered_out']} from other callers skipped)" if d["options"].get("caller_in") else "")
                  + (f" ({d['injected']} owdbg calls skipped)" if d["injected"] else ""))
            for text, n in d["callers"][:5]:
                s.log(f"      caller {text}: {n}")
            for val, n in d["returns"][:5]:
                s.log(f"      ret {val}: {n}")
            for th, n in d["threads"][:3]:
                s.log(f"      thread {th}: {n}")
        if act is not None:
            s.log(f"[actions] {act.summary()}")
        if probe.msg is not None:
            s.log(f"[probe] {probe.msg.describe()}; hits with the marker: {len(probe.marker_hits)}"
                  + (f", sends {probe.sender.sent} (errors {probe.sender.errors})" if probe.sender else ""))


if __name__ == "__main__":
    main()
