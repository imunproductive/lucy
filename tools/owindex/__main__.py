"""Quick lookups: python -m tools.owindex <key> [<key> ...] [--json]
                 python -m tools.owindex holds <function> [...] [--json]   (function tables holding it)

key: a class (CD7F3617, STU_CD7F3617, an OWLib class name), a field (m_B350E4FD, an OWLib field name), an
address (0x7FF789BE27C0, 0x14249F530, an RVA such as 0xCD27C0; strings are hex), a name (Eval_STU_CD7F3617,
Ingame_Watchdog, GetComp74) or an entity component (comp74). Hashes without 0x are tried as classes first;
'0x...' is always an address.
"""

import json
import re
import sys

from . import Index

HEX = set("0123456789abcdefABCDEF")


def _va(idx, rva):
    return "-" if rva is None else f"{rva:#x} ({idx.base + rva:#x})"


def print_stu(idx, s):
    lab = f" ({s['name']}, owlib label)" if s["name_source"] == "owlib" else ""
    print(f"STU_{s['hash']}{lab}  size {s['size']:#x}  parent {s['parent_hash'] and 'STU_' + s['parent_hash']}"
          f"{' (' + s['parent_name'] + ')' if s['parent_name'] and s['parent_hash'] and not s['parent_name'].startswith('STU_') else ''}")  # fmt: skip
    print(f"  descriptor {_va(idx, s['descriptor_rva'])}  node typeinfo {_va(idx, s['node_typeinfo_rva'])}"
          f"  node vtable {_va(idx, s['node_vtable_rva'])}")  # fmt: skip
    if s["children"]:
        print(f"  children: {len(s['children'])}: " + " ".join(s["children"][:12]) + (" ..." if len(s["children"]) > 12 else ""))
    for f in s["fields"]:
        t = f["type"]
        desc = t.get("name") or (("STU_" if t["kind"] == "stu" else "") + t["hash"] if t.get("hash") else "?")
        tk = t["kind"] + ("[]" if t.get("array") else "") + (f"/{t['mode']}" if t.get("mode") else "")
        label = f" (owlib: {f['name']})" if f["name_source"] == "owlib" else ""
        print(f"  +{f['offset']:#06x} m_{f['hash']}{label}: {tk} {desc}{' ' + t['label'] if t.get('label') else ''}"
              f"  flags {f['flags']:#x}/{f['flags2']:#x}")  # fmt: skip
    for r in s["registrations"]:
        slots = "  ".join(f"{k} {_va(idx, v)}" for k, v in r["slots"].items())
        print(f"  {r['kind']} record #{r['index']} {_va(idx, r['record_rva'])} prio {r['priority']}"
              f" {'WINNER' if r['winner'] else 'overridden'}  vtable {_va(idx, r['runtime_vtable_rva'])}")  # fmt: skip
        print(f"    factory {_va(idx, r['factory_rva'])}  construct {_va(idx, r['construct_rva'])}  {slots}")
    e = s["effective_registration"]
    if e and not e["own"]:
        print(f"  no own registration: uses STU_{e['via_hash']}'s ({e['kind']} record #{e['record_index']})")
    elif not e:
        print("  no registration on its class chain")
    n = idx.component_of_stu(s["hash"])
    if n is not None:
        c = idx.component(n)
        role = "data class" if s["hash"] in c["data_classes"] else f"mirror (at component +{c['mirror_offset'] or 0:#x})"
        print(f"  {role} of entity component {n} (python -m tools.owindex comp{n})")


def print_component(idx, c):
    va = lambda r: "-" if r is None else f"{idx.base + r:#x}"  # noqa: E731
    print(f"component {c['id']}{' ' + c['name'] if c.get('name') else ''}{'  (' + c['label'] + ')' if c.get('label') else ''}"
          f"  size {c['size'] if c.get('size') is None else hex(c['size'])}")
    print(f"  constructor {va(c.get('construct_rva'))}  create {va(c.get('create_rva'))}  vtable {va(c.get('vtable_rva'))}")
    data = ", ".join(idx.label(int(h, 16)) for h in c["data_classes"]) or "none (only the network creates it)"
    print(f"  data class: {data}")
    if c.get("mirror_class"):
        mo = "?" if c.get("mirror_offset") is None else f"+{c['mirror_offset']:#x}"
        print(f"  mirror: {idx.label(int(c['mirror_class'], 16))} at component {mo} ({len(c['fields'])} fields)")
        for f in c["fields"]:
            t = f["type"]
            tn = t.get("name") or (t.get("hash") and ("STU_" if t["kind"] == "stu" else "") + t["hash"]) or ""
            off = "?" if f["offset"] is None else f"+{f['offset']:#06x}"
            label = f" ({f['name']})" if f["name_source"] == "owlib" else ""
            print(f"    {off}  field {f['index']}  mirror +{f['mirror_offset']:#x}  m_{f['hash']}{label}: "
                  f"{t['kind']}{'[]' if t.get('array') else ''} {tn}  flags {f['flags']:#x}")
    else:
        print("  mirror: none found")
    for a in c["accessors"]:
        print(f"  {a['kind']} {va(a['rva'])} {a['name'] or ''} ({a['calls']} call sites)")
    if not any(a["kind"] == "getter" for a in c["accessors"]):
        print("  getter: none found")
    funcs = {}
    for u in c["uses"]:
        funcs.setdefault(u["func_rva"], []).append(u["kind"])
    n = {k: sum(1 for u in c["uses"] if u["kind"] == k) for k in ("getter call", "id stub call", "has call")}
    print(f"  used by {len(funcs)} functions ({n['getter call']} getter calls, {n['id stub call']} id stub calls "
          f"(factory, pools), {n['has call']} has-component tests):")
    for r in sorted(funcs)[:40]:
        print(f"    {va(r)} {idx.name(r) or ''}  {', '.join(sorted(set(funcs[r])))}")
    if len(funcs) > 40:
        print(f"    ... {len(funcs) - 40} more (--json)")


def print_addr(idx, a):
    print(f"rva {a['rva']:#x}  lucy {a['lucy']:#x}  ida {a['ida']:#x}  kjs {a['kjs']:#x}  name {a['name']}")
    if a.get("function"):
        f = a["function"]
        print(f"  function {_va(idx, f['root'])} {f['name'] or ''}  (.pdata {f['begin']:#x}-{f['end']:#x})")
    for w in a["whats"]:
        print("  " + w)


def run(idx, key, as_json):
    s = key.strip()
    m = re.fullmatch(r"(?i)comp(?:onent)?_?(\d+)", s)
    if m:
        c = idx.component(int(m.group(1)))
        if c is None:
            return print(f"{key}: no such component")
        return print(json.dumps(c, indent=1)) if as_json else print_component(idx, c)
    plain_hex = all(ch in HEX for ch in s) and len(s) <= 8
    if not s.lower().startswith("0x") and (s.upper().startswith("STU_") or plain_hex or not all(ch in HEX for ch in s)):
        st = idx.stu(s)
        if st:
            return print(json.dumps(st, indent=1)) if as_json else print_stu(idx, st)
        if s.startswith("m_") or not plain_hex:
            fs = idx.field(s)
            if fs:
                if as_json:
                    return print(json.dumps(fs, indent=1))
                for f in fs:
                    print(f"STU_{f['class_hash']} ({f['class_name']}) +{f['offset']:#x} m_{f['hash']} ({f['name']}):"
                          f" {f['type']['kind']} {f['type'].get('name') or f['type'].get('hash') or ''}")  # fmt: skip
                return None
            rva = idx.lookup(s)
            if rva is None:
                return print(f"{key}: no class, field or name")
            s = hex(rva)
    try:
        a = idx.addr(s)
    except ValueError as e:
        return print(f"{key}: {e}")
    return print(json.dumps(a, indent=1)) if as_json else print_addr(idx, a)


def run_holds(idx, key, as_json):
    rva = idx.lookup(key)
    try:
        rva = idx.to_rva(key) if rva is None else rva
    except ValueError as e:
        return print(f"{key}: not a name or an address ({e})")
    hs = idx.holders(rva)
    if as_json:
        return print(json.dumps(hs, indent=1))
    print(f"{idx.base + rva:#x} {idx.name(rva) or ''}: held by {len(hs)} table slot(s)")
    for h in hs:
        what = "; ".join(h["whats"]) or "unknown table"
        roles = f"  slot role: {', '.join(h['slot_roles'])}" if h["slot_roles"] else ""
        print(f"  {idx.base + h['table_rva']:#x} {h['table_name'] or ''} slot {h['slot']} (+{h['slot'] * 8:#x}) "
              f"at {idx.base + h['slot_rva']:#x}  [{h['table_source']}]{roles}")
        print(f"      {what}")


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    as_json = "--json" in argv
    keys = [a for a in argv if a != "--json"]
    if not keys:
        print(__doc__)
        return 2
    idx = Index.open()
    if keys[0] == "holds":
        for key in keys[1:]:
            run_holds(idx, key, as_json)
        return 0
    for key in keys:
        run(idx, key, as_json)
    return 0


if __name__ == "__main__":
    sys.exit(main())
