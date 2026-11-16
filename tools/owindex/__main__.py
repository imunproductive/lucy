"""Quick lookups: python -m tools.owindex <key> [<key> ...] [--json]

key: a class (CD7F3617, STU_CD7F3617, an OWLib class name), a field (m_B350E4FD, an OWLib field name), an
address (0x7FF789BE27C0, 0x14249F530, an RVA such as 0xCD27C0; strings are hex) or a name (Eval_STU_CD7F3617,
Ingame_Watchdog). Hashes without 0x are tried as classes first; '0x...' is always an address.
"""

import json
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


def print_addr(idx, a):
    print(f"rva {a['rva']:#x}  orpheus {a['orpheus']:#x}  ida {a['ida']:#x}  kjs {a['kjs']:#x}  name {a['name']}")
    if a.get("function"):
        f = a["function"]
        print(f"  function {_va(idx, f['root'])} {f['name'] or ''}  (.pdata {f['begin']:#x}-{f['end']:#x})")
    for w in a["whats"]:
        print("  " + w)


def run(idx, key, as_json):
    s = key.strip()
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


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    as_json = "--json" in argv
    keys = [a for a in argv if a != "--json"]
    if not keys:
        print(__doc__)
        return 2
    idx = Index.open()
    for key in keys:
        run(idx, key, as_json)
    return 0


if __name__ == "__main__":
    sys.exit(main())
