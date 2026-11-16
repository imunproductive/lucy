"""Acceptance checks of the index: python -m tools.owindex.check [--codecs PATH]

- the verified config-var facts (STU_CD7F3617 / STU_3832D36C fields, records, evaluators);
- addr()/name() on known addresses;
- every state record against ORPHEUS crates/fixed/data/statescript_codecs.json `records` (list index, record,
  construct, destroy, typeinfo, priority, hash, winner, vtable, reader, block, writer);
- query times after open.
Exit code 1 when a check fails.
"""

import argparse
import json
import sys
import time
from pathlib import Path

from tools import paths

from . import Index

DEFAULT_CODECS = paths.CODECS_JSON
KEYS = ["record", "construct", "destroy", "typeinfo", "priority", "hash", "winner", "vtable", "reader", "block",
        "writer"]  # fmt: skip


def check_codecs(idx, path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    base = int(data["image_base"], 16)
    if base != idx.base:
        return [f"codec image base {base:#x} != index base {idx.base:#x}"], 0
    ours = {}
    for r in idx.records("state"):
        ours[r["index"]] = {
            "record": r["record_rva"] + base, "construct": r["construct_rva"] + base,
            "destroy": r["destroy_rva"] + base, "typeinfo": r["typeinfo_rva"] + base, "priority": r["priority"],
            "hash": r["class_hash"], "winner": r["winner"],
            "vtable": None if r["runtime_vtable_rva"] is None else r["runtime_vtable_rva"] + base,
            **{k: r["slots"][k] + base if k in r["slots"] else None for k in ("reader", "block", "writer")},
        }  # fmt: skip
    bad = []
    theirs = {r["index"]: r for r in data["records"]}
    for i in sorted(set(ours) | set(theirs)):
        if i not in ours or i not in theirs:
            bad.append(f"record {i}: only in {'index' if i in ours else 'codec json'}")
            continue
        for k in KEYS:
            v = theirs[i].get(k)
            if isinstance(v, str) and v.startswith("0x"):
                v = int(v, 16)
            if v != ours[i][k]:
                bad.append(f"record {i} {k}: codec {v!r} index {ours[i][k]!r}")
    return bad, len(theirs)


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--codecs", default=str(DEFAULT_CODECS), help="codec table (default %(default)s)")
    ap.add_argument("--index", default=None, help=f"index file (default {paths.INDEX})")
    args = ap.parse_args(argv)
    t0 = time.perf_counter()
    idx = Index.open(args.index)
    print(f"open: {1000 * (time.perf_counter() - t0):.1f} ms; built {idx.meta.get('built')} in {idx.meta.get('seconds')} s")
    fails = []

    def expect(cond, text):
        print(("ok   " if cond else "FAIL ") + text)
        if not cond:
            fails.append(text)

    base = idx.base
    for h, fields, rec, ev in (
        ("CD7F3617", [("A83C2C26", 0x10), ("B350E4FD", 0x18)], 0x2F867D0, 0x7FF789BE27C0),
        ("3832D36C", [("07DD813E", 0x10)], 0x2F86290, 0x7FF789BDFAC0),
    ):
        s = idx.stu(h)
        expect(s is not None, f"stu {h} exists")
        if s is None:
            continue
        got = [(f["hash"], f["offset"]) for f in s["fields"]]
        expect(got == fields, f"stu {h} fields {got}")
        cfg = [r for r in s["registrations"] if r["kind"] == "config"]
        expect(any(r["record_rva"] == rec and r["winner"] and r["slots"].get("evaluate") == ev - base for r in cfg),
               f"stu {h} config record {rec:#x} winner, slots.evaluate = {ev - base:#x}")  # fmt: skip
        a = idx.addr(ev)
        expect(a["what"] and a["what"].startswith(f"evaluator of STU_{h}"), f"addr({ev:#x}).what = {a['what']!r}")
    a = idx.addr(0x14249F530)
    expect(a["name"] == "Ingame_Watchdog" and "Ingame_Watchdog" in (a["what"] or ""), f"addr(0x14249F530): {a['name']}")
    expect(idx.lookup("Eval_STU_CD7F3617") == 0x7FF789BE27C0 - base, "lookup(Eval_STU_CD7F3617)")
    if idx.meta.get("net_status") == "ok":
        a = idx.addr(0x7FF7895D2B50)
        expect("message 25005" in (a["what"] or ""), f"addr(0x7FF7895D2B50) [{a['name']}]: {a['what']!r}")
        a = idx.addr(0x5BB3C0)
        expect("component 51 constructor" in (a["what"] or ""), f"addr(rva 0x5BB3C0) [{a['name']}]: {(a['what'] or '')[:160]!r}")
    else:
        expect(False, f"net tables present (net_status {idx.meta.get('net_status')!r})")

    bad, n = check_codecs(idx, args.codecs)
    ours = len(idx.records("state"))
    expect(not bad and n == ours, f"state records vs {args.codecs}: codec {n}, index {ours}, mismatches {len(bad)}")
    for line in bad[:20]:
        print("     " + line)

    times = []
    for fn, arg in ((idx.stu, "CD7F3617"), (idx.stu, "STUStatescriptStateAbility"), (idx.field, "m_B350E4FD"),
                    (idx.addr, 0x7FF789BE27C0), (idx.addr, 0x14249F530), (idx.addr, 0x7FF78BF28C28),
                    (idx.name, 0xCD27C0), (idx.lookup, "Ingame_Watchdog")):  # fmt: skip
        t = time.perf_counter()
        fn(arg)
        times.append((fn.__name__, arg, 1000 * (time.perf_counter() - t)))
    worst = max(ms for *_, ms in times)
    expect(worst < 50, "query times: " + ", ".join(f"{n}({a if isinstance(a, str) else hex(a)}) {ms:.1f} ms" for n, a, ms in times))
    print(f"{len(fails)} failed")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
