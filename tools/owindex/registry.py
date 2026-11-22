"""The statescript/config-var registration list: records, kinds, winners, runtime vtables and named slots.

List (Lucy extract_statescript_codecs.py): head qword at RVA 0x32969D8, nodes {next, record}, records
{+0 factory, +8 construct, +0x10 destroy (returns the object size), +0x18 node typeinfo, +0x20 priority}.
The client's builder keeps per typeinfo the record with the highest priority, ties to the later record.

Kinds: the record's typeinfo chain (typeinfo+8 = parent) contains the typeinfo of one base class (KIND_BASES,
first match wins). Runtime vtables: states = the last state-shaped vtable the construct function stores
(Lucy primary_vtable); the others are 8-byte handler objects whose factory and construct store one vtable.

Named slots (runtime vtable index):
- state: writer +0x150, reader +0x158, block +0x168 (Lucy; read in 0x7FF78AA82060).
- config: evaluate = slot 4 (verified: STU_CD7F3617 -> 0x7FF789BE27C0, STU_3832D36C -> 0x7FF789BDFAC0).
- action: slots 0-8 and 11-14 are the same in all 260 action vtables; slot 10 is per class (225 distinct, the
  base STUStatescriptAction's is `mov al, 1; ret`) and slot 9 (233 of 260 share the base's) calls
  `[vt+0x50]` = slot 10, then follows the output plug (+0x88): slot 10 = execute, slot 9 = run.
- condition: slot 8 is per class; slot 9 (shared) calls `[vt+0x40]` = slot 8 and follows +0x88 when true or
  +0x90 when false: slot 8 = evaluate, slot 9 = run.
- entry: slot 9 (shared) follows the output plug +0x80: run. Slot 10 differs per class (meaning not read).
- all handler kinds: slot 1 = `lea rax, [rip+tag]; ret` (runtime type tag), slot 0 shared.
The builder re-checks the run -> execute/evaluate calls by disassembly and records the result in the index.
"""

import bisect

from .image import stored_vtable, this_events, trace

LIST_HEAD_RVA = 0x32969D8
STATE_TYPEINFO_RVA = 0x7FF78BCE53F0 - 0x7FF788F10000

KIND_BASES = [  # (kind, base class hash); OWLib labels in the comments
    ("state", 0x7C37840C),  # STUStatescriptState
    ("action", 0xDD856C32),  # STUStatescriptAction
    ("condition", 0x142A27A8),  # STUStatescriptCondition
    ("entry", 0x1E50E1A8),  # STUStatescriptEntry
    ("config", 0xD05AEF31),  # STUConfigVar
    ("statescript", 0xC2DC60F6),  # STUStatescriptBase (anything else below it)
]

SLOTS = {
    "state": {"writer": 0x150, "reader": 0x158, "block": 0x168},
    "config": {"type_tag": 8, "evaluate": 0x20},
    "action": {"type_tag": 8, "run": 0x48, "execute": 0x50},
    "condition": {"type_tag": 8, "evaluate": 0x40, "run": 0x48},
    "entry": {"type_tag": 8, "run": 0x48},
    "statescript": {"type_tag": 8},
}
# (kind, run slot offset, the slot it must call)
RUN_CHECKS = [("action", 0x48, 0x50), ("condition", 0x48, 0x40)]


def typeinfo_chain(img, ti):
    out = []
    while ti and img.inimg(ti) and ti not in out and len(out) < 40:
        out.append(ti)
        parent = img.q(ti + 8)
        ti = parent if parent != ti else 0
    return out


def looks_like_state_vtable(img, vt):
    return all(img.intext(img.qs(vt + 8 * k)) for k in range(0x170 // 8))


def primary_vtable(img, construct):
    stores = [v for kind, _, v in this_events(img, construct) if kind == "vt" and looks_like_state_vtable(img, v)]
    return stores[-1] if stores else None


def calls_slot(img, fn, slot_off, maxins=40):
    """True when fn does `call qword ptr [rax + slot_off]` within its first instructions."""
    want = f"qword ptr [rax + {slot_off:#x}]"
    off = fn - img.base
    for n, ins in enumerate(img.cs.disasm(img.data[off : off + 400], fn)):
        if ins.mnemonic == "call" and ins.op_str == want:
            return True
        if ins.mnemonic == "ret" or n > maxins:
            break
    return False


def vtable_size(img, vt, starts, cap=400):
    """Slots up to the next known vtable start (`starts` sorted) or the first non-code qword."""
    i = bisect.bisect_right(starts, vt)
    nxt = starts[i] if i < len(starts) else vt + 8 * cap
    n = 0
    while n < cap and vt + 8 * n < nxt and img.intext(img.qs(vt + 8 * n)):
        n += 1
    return n


def build_registry(img, stu):
    """-> (records, kinds_meta, problems, evidence)"""
    problems, evidence = [], []
    kind_ti = {}
    kinds_meta = []
    for kind, h in KIND_BASES:
        desc = stu.by_hash.get(h)
        ti = img.q(desc + 8) if desc else None
        kind_ti[kind] = ti
        kinds_meta.append({"kind": kind, "base_hash": f"{h:08X}", "base_typeinfo_rva": ti - img.base if ti else None})
        if not ti:
            problems.append(f"kind {kind}: base STU_{h:08X} has no typeinfo")
    if kind_ti["state"] != img.base + STATE_TYPEINFO_RVA:
        problems.append(f"state base typeinfo {kind_ti['state']} != 0x7FF78BCE53F0 (Lucy)")

    records, node = [], img.q(img.base + LIST_HEAD_RVA)
    while node and img.inimg(node) and len(records) < 10000:
        rec = img.q(node + 8)
        ti = img.q(rec + 0x18)
        chain = typeinfo_chain(img, ti)
        kind = next((k for k, _ in KIND_BASES if kind_ti[k] and kind_ti[k] in chain), "other")
        records.append(
            {
                "index": len(records),
                "node": node,
                "record": rec,
                "factory": img.q(rec),
                "construct": img.q(rec + 8),
                "destroy": img.q(rec + 0x10),
                "typeinfo": ti,
                "priority": img.d(rec + 0x20),
                "hash": stu.ti_to_hash.get(ti),
                "kind": kind,
                "chain": chain,
            }
        )
        node = img.q(node)
    winners = {}
    for r in records:
        held = winners.get(r["typeinfo"])
        if held is None or r["priority"] >= records[held]["priority"]:
            winners[r["typeinfo"]] = r["index"]
    for r in records:
        r["winner"] = winners[r["typeinfo"]] == r["index"]
        if r["hash"] is None:
            problems.append(f"record {r['index']} {r['record']:#x}: typeinfo {r['typeinfo']:#x} has no descriptor")
        if r["kind"] == "state":
            vt = primary_vtable(img, r["construct"])
        else:
            vt = stored_vtable(img, r["construct"])
            vf = stored_vtable(img, r["factory"])
            if vt is None:
                vt = vf
            elif vf is not None and vf != vt:
                problems.append(f"record {r['index']}: factory vtable {vf:#x} != construct vtable {vt:#x}")
        r["vtable"] = vt
        if vt is None:
            problems.append(f"record {r['index']} ({r['kind']} {r['hash']}): no runtime vtable")
        r["slots"] = {name: img.qs(vt + off) for name, off in SLOTS.get(r["kind"], {}).items()} if vt else {}

    starts = sorted({r["vtable"] for r in records if r["vtable"]})
    for r in records:
        r["vtable_size"] = vtable_size(img, r["vtable"], starts) if r["vtable"] else 0

    for kind, run_off, target_off in RUN_CHECKS:
        runs = {img.qs(r["vtable"] + run_off) for r in records if r["kind"] == kind and r["vtable"]}
        direct = sorted(f for f in runs if calls_slot(img, f, target_off))
        # class overrides of run that end in a base run (call/jmp to a direct one) still execute/evaluate
        via = []
        for f in sorted(runs - set(direct)):
            targets = {i.op_str for b in trace(img, f, maxblocks=60, maxins=600) for i in b if i.mnemonic in ("call", "jmp")}
            if any(f"{d:#x}" in targets for d in direct):
                via.append(f)
        evidence.append(
            f"{kind}: {len(direct)}/{len(runs)} distinct run functions (slot {run_off // 8}) call [vt+{target_off:#x}]"
            f" (slot {target_off // 8}) themselves ({', '.join(f'{f:#x}' for f in direct[:4])}); {len(via)} more are"
            f" class overrides that call/jump to one of those; {len(runs) - len(direct) - len(via)} reach neither"
            " within the traced blocks"
        )
    return records, kinds_meta, problems, evidence
