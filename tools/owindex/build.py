"""Build the Overwatch 1.74 client index (research/out/index/ow_index.sqlite) from the runtime image dump.

    python -m tools.owindex.build [--dump DUMP] [--out research/out/index/ow_index.sqlite]

Run from the ORPHEUS root. Reads only the dump (default: the research repo's
dumps/ingame_7FF788F10000.exe), OWLib's labels (crates/stu/data/types.txt) and the research repo's
docs/analysis/*.json (locations: tools/paths.py); calls tools/owindex/net.py when present. See stu.py /
registry.py for the layouts.
"""

import argparse
import importlib
import json
import os
import sqlite3
import struct
import sys
import time
import traceback
from collections import Counter, defaultdict
from pathlib import Path

from tools import paths

from .image import Image
from .netnames import net_names
from .registry import build_registry
from .stu import StuTables

DEFAULT_DUMP = paths.DUMP
DEFAULT_OUT = paths.INDEX
DEFAULT_OWLIB = paths.OWLIB_TYPES
IDA_BASE = 0x140000000
SCHEMA_VERSION = 1

# function roles (role, kind or None for any kind) -> name prefix; Index._describe has the matching what-texts
FUNC_PREFIX = {
    ("evaluate", "config"): "Eval",
    ("evaluate", "condition"): "Cond",
    ("execute", "action"): "Action",
    ("run", "action"): "RunAction",
    ("run", "condition"): "RunCond",
    ("run", "entry"): "RunEntry",
    ("reader", "state"): "Read",
    ("block", "state"): "Block",
    ("writer", "state"): "Write",
    ("type_tag", None): "TypeTag",
    ("factory", None): "Factory",
    ("construct", None): "Construct",
    ("destroy", None): "Destroy",
}
FUNC_ORDER = ["evaluate", "execute", "reader", "block", "writer", "run", "construct", "factory", "type_tag", "destroy"]


def func_prefix(role, kind):
    return FUNC_PREFIX.get((role, kind)) or FUNC_PREFIX.get((role, None))


# --- inputs -------------------------------------------------------------------------------------------------


def load_owlib(path):
    classes, fields = {}, defaultdict(dict)
    if not path or not Path(path).exists():
        return classes, fields
    cur = None
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        p = line.split()
        if not p or p[0] == "#":
            continue
        if p[0] == "class":
            cur = int(p[1], 16)
            classes[cur] = (p[2], p[3])
        elif p[0] == "field" and cur is not None:
            fields[cur][int(p[1], 16)] = (p[2], p[3], p[4])
    return classes, fields


def class_label(owl_classes, h):
    o = owl_classes.get(h)
    return (o[0], "owlib") if o and o[0] != f"STU_{h:08X}" else (f"STU_{h:08X}", None)


def field_label(owl_fields, ch, fh):
    o = owl_fields.get(ch, {}).get(fh)
    return (o[0], "owlib") if o and o[0] != f"m_{fh:08X}" else (f"m_{fh:08X}", None)


def load_docs(analysis):
    """docs/analysis/*.json functions/globals (VAs at 0x140000000) -> [(rva, name, kind, file)]."""
    out = []
    for path in sorted(Path(analysis).glob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        for key, kind in (("functions", "func"), ("globals", "data")):
            items = data.get(key)
            if isinstance(items, dict):
                for va, name in items.items():
                    out.append((int(va, 16) - IDA_BASE, name, kind, path.name))
    return out


def load_obfuscated(analysis):
    path = Path(analysis) / "obfuscated_functions.json"
    if not path.exists():
        return []
    data = json.loads(path.read_text(encoding="utf-8"))
    return [
        (int(f["addr"], 16) - IDA_BASE, int(f["size"], 16), f.get("confidence"), f.get("area"))
        for f in data.get("functions", [])
    ]


def pdata_rows(img):
    """.pdata RUNTIME_FUNCTIONs -> (begin, end, root begin) with chained unwind info followed to the root."""
    lo, hi = img.pdata
    entries = list(struct.iter_unpack("<III", img.data[lo : lo + (hi - lo) // 12 * 12]))

    def root(b, u):
        for _ in range(32):
            if not (0 < u < img.size - 4):
                break
            head = img.data[u]
            if not (head >> 3) & 4:  # UNW_FLAG_CHAININFO
                break
            count = img.data[u + 2]
            at = u + 4 + 2 * ((count + 1) & ~1)
            b, _, u = struct.unpack_from("<III", img.data, at)
        return b

    return [(b, e, root(b, u)) for b, e, u in entries if b and e > b]


def run_net(img):
    try:
        net = importlib.import_module(f"{__package__}.net")
    except ModuleNotFoundError as e:
        if e.name and e.name.endswith(".net"):
            return None, "missing: tools/owindex/net.py not present"
        return None, f"error importing net.py: {e!r}"
    except Exception as e:  # noqa: BLE001 - recorded in the index metadata
        return None, f"error importing net.py: {e!r}"
    try:
        result = net.build(img.data, img.base)
        return result, "ok"
    except Exception:  # noqa: BLE001
        return None, "error in net.build: " + traceback.format_exc(limit=3).strip().splitlines()[-1]


# --- naming -------------------------------------------------------------------------------------------------


def is_trivial(img, va):
    """A body of at most two instructions ending in ret (`ret`, `ret 0`, `mov al, 1; ret`, `mov eax, 8; ret`)."""
    off = va - img.base
    for n, ins in enumerate(img.cs.disasm(img.data[off : off + 32], va)):
        if ins.mnemonic == "ret":
            return True
        if n >= 1 or ins.mnemonic in ("call", "jmp"):
            return False
    return False


def owner_of(owners, chains):
    """The class all owners derive from, when it is one of them; else None (a shared function)."""
    if len(owners) == 1:
        return next(iter(owners))
    for c in owners:
        if all(c in chains.get(o, ()) for o in owners):
            return c
    return None


# --- main ---------------------------------------------------------------------------------------------------


SCHEMA = """
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE stu(hash INTEGER PRIMARY KEY, name TEXT, name_source TEXT, descriptor_rva INTEGER,
    parent_hash INTEGER, typeinfo_rva INTEGER, node_vtable_rva INTEGER, size INTEGER, align INTEGER,
    field_count INTEGER);
CREATE TABLE field(class_hash INTEGER, idx INTEGER, hash INTEGER, name TEXT, name_source TEXT, offset INTEGER,
    type TEXT, flags INTEGER, flags2 INTEGER, entry_rva INTEGER);
CREATE TABLE registration(idx INTEGER PRIMARY KEY, record_rva INTEGER, node_rva INTEGER, class_hash INTEGER,
    typeinfo_rva INTEGER, kind TEXT, factory_rva INTEGER, construct_rva INTEGER, destroy_rva INTEGER,
    priority INTEGER, winner INTEGER, runtime_vtable_rva INTEGER, vtable_slots INTEGER, slots TEXT);
CREATE TABLE role(rva INTEGER, size INTEGER, role TEXT, kind TEXT, class_hash INTEGER, reg_idx INTEGER,
    winner INTEGER, detail TEXT);
CREATE TABLE symbol(rva INTEGER, name TEXT, kind TEXT, source TEXT, prio INTEGER);
CREATE TABLE func(begin INTEGER PRIMARY KEY, end INTEGER, root INTEGER);
CREATE TABLE obf(begin INTEGER PRIMARY KEY, end INTEGER, confidence TEXT, area TEXT);
CREATE TABLE net(kind TEXT, idx INTEGER, data TEXT);
CREATE TABLE netrole(rva INTEGER, text TEXT);
"""
INDEXES = """
CREATE INDEX field_hash ON field(hash);
CREATE INDEX field_class ON field(class_hash);
CREATE INDEX field_entry ON field(entry_rva);
CREATE INDEX reg_class ON registration(class_hash);
CREATE INDEX role_rva ON role(rva);
CREATE INDEX symbol_rva ON symbol(rva);
CREATE INDEX symbol_name ON symbol(name);
CREATE INDEX stu_name ON stu(name);
CREATE INDEX stu_desc ON stu(descriptor_rva);
CREATE INDEX netrole_rva ON netrole(rva);
"""


def build(dump, out, owlib, analysis=paths.ANALYSIS_DIR, log=print):
    t0 = time.time()
    img = Image(dump)
    base = img.base
    owl_classes, owl_fields = load_owlib(owlib)
    log(f"image {dump} base {base:#x} size {img.size:#x}; OWLib labels: {len(owl_classes)} classes")

    stu = StuTables(img)
    classes = stu.classes()
    log(
        f"STU: {stu.type_vt_objects} TYPE_VT objects -> {len(classes)} classes "
        f"({stu.isolated} isolated), node vtables {len(stu.node_vtables)} ({time.time() - t0:.1f} s)"
    )
    records, kinds_meta, problems, evidence = build_registry(img, stu)
    problems = stu.problems + problems
    log(f"registration list: {len(records)} records ({time.time() - t0:.1f} s)")
    for k in kinds_meta:
        k["base_name"], k["base_name_source"] = class_label(owl_classes, int(k["base_hash"], 16))

    rv = img.rva
    stu_rows, field_rows, roles, unknown_prims = [], [], [], Counter()
    chains = {}  # class hash -> set of ancestor hashes (incl. itself)
    parents = {c["hash"]: c["parent"] for c in classes}
    for c in classes:
        h, chain, p = c["hash"], {c["hash"]}, c["parent"]
        while p is not None and p not in chain:
            chain.add(p)
            p = parents.get(p)
        chains[h] = chain
    for c in classes:
        h = c["hash"]
        name, src = class_label(owl_classes, h)
        nvt = stu.node_vtables.get(c["typeinfo"]) if c["typeinfo"] else None
        stu_rows.append(
            (h, name, src, rv(c["descriptor"]), c["parent"], rv(c["typeinfo"]) if c["typeinfo"] else None,
             rv(nvt) if nvt else None, c["size"], c["align"], len(c["fields"]))
        )  # fmt: skip
        roles.append((rv(c["descriptor"]), 0xB8, "descriptor", None, h, None, None, None))
        if c["typeinfo"]:
            roles.append((rv(c["typeinfo"]), 0x10, "typeinfo", None, h, None, None, None))
        if nvt:
            roles.append((rv(nvt), 0x30, "node_vtable", None, h, None, None, None))
        for i, f in enumerate(c["fields"]):
            fname, fsrc = field_label(owl_fields, h, f["hash"])
            t = f["type"]
            if t["kind"] == "unknown" and t.get("code") in (0, 1):
                unknown_prims[t.get("id")] += 1
            field_rows.append(
                (h, i, f["hash"], fname, fsrc, f["offset"], json.dumps(t), f["flags"], f["flags2"], rv(f["entry"]))
            )
            roles.append((rv(f["entry"]), 56, "field", None, h, None, None, f"{f['hash']:08X}"))
    for pid, n in unknown_prims.items():
        problems.append(f"primitive id {pid} not in PRIMITIVE_NAMES ({n} fields)")

    reg_rows = []
    for r in records:
        h = r["hash"]
        slots = {k: rv(v) for k, v in r["slots"].items() if v}
        reg_rows.append(
            (r["index"], rv(r["record"]), rv(r["node"]), h, rv(r["typeinfo"]), r["kind"], rv(r["factory"]),
             rv(r["construct"]), rv(r["destroy"]), r["priority"], int(r["winner"]),
             rv(r["vtable"]) if r["vtable"] else None, r["vtable_size"], json.dumps(slots))
        )  # fmt: skip
        common = (r["kind"], h, r["index"], int(r["winner"]))
        roles.append((rv(r["record"]), 0x24, "record", *common, None))
        if r["vtable"]:
            roles.append((rv(r["vtable"]), 8 * max(r["vtable_size"], 1), "runtime_vtable", *common, None))
        for role in ("factory", "construct", "destroy"):
            roles.append((rv(r[role]), 0, role, *common, None))
        for role, rva in slots.items():
            roles.append((rva, 0, role, *common, None))

    # names: data objects per class; functions by the owner rule
    symbols = []  # (rva, name, kind, source, prio)
    nrec = Counter(r["hash"] for r in records)
    for c in classes:
        h = c["hash"]
        symbols.append((rv(c["descriptor"]), f"STUDesc_STU_{h:08X}", "data", "index", 1))
        if c["typeinfo"]:
            symbols.append((rv(c["typeinfo"]), f"TypeInfo_STU_{h:08X}", "data", "index", 1))
        nvt = stu.node_vtables.get(c["typeinfo"]) if c["typeinfo"] else None
        if nvt:
            symbols.append((rv(nvt), f"NodeVtbl_STU_{h:08X}", "data", "index", 1))
    vt_named = set()
    for r in sorted(records, key=lambda r: (not r["winner"], r["index"])):
        h = r["hash"]
        suffix = "" if nrec[h] == 1 else f"_{r['index']}"
        symbols.append((rv(r["record"]), f"RegRec_STU_{h:08X}{suffix}", "data", "index", 1))
        if r["vtable"] and r["vtable"] not in vt_named:
            vt_named.add(r["vtable"])
            symbols.append((rv(r["vtable"]), f"Vtbl_STU_{h:08X}" + ("" if r["winner"] else suffix), "data", "index", 1))
    # A function gets an index name only when every role it has (over all kinds) belongs to one class and its
    # subclasses; trivial bodies (`ret`, `mov al, 1; ret`, ...) shared by several classes are identical-code
    # folded and stay unnamed (addr() still lists every role).
    groups = defaultdict(set)  # rva -> {(role, kind)}
    owners_at = defaultdict(set)  # rva -> owner hashes over all roles
    for rva, size, role, kind, h, _idx, _win, _ in roles:
        if size == 0 and h is not None:
            groups[rva].add((role, kind))
            owners_at[rva].add(h)
    func_names = {}  # rva -> name
    for rva, role_kinds in groups.items():
        owners = owners_at[rva]
        owner = owner_of(owners, chains)
        if owner is None or (len(owners) > 1 and is_trivial(img, img.base + rva)):
            continue
        role, kind = min(role_kinds, key=lambda rk: FUNC_ORDER.index(rk[0]))
        func_names[rva] = f"{func_prefix(role, kind)}_STU_{owner:08X}"
    taken = Counter(func_names.values())
    for rva, name in func_names.items():
        symbols.append((rva, name if taken[name] == 1 else f"{name}_{rva:X}", "func", "index", 1))
    for rva, name, kind, src in load_docs(analysis):
        symbols.append((rva, name, kind, f"docs/analysis/{src}", 0))

    funcs = pdata_rows(img)
    obf = load_obfuscated(analysis)
    net_result, net_status = run_net(img)
    log(f"net.py: {net_status}")
    net_roles, net_symbols = net_names(net_result)
    # STU-derived names (prio 1) win over net-derived ones (prio 2) at the same rva; docs/analysis (0) over both
    symbols += [(rva, name, kind, "net", 2) for rva, name, kind in net_symbols]

    counts = {
        "type_vt_objects": stu.type_vt_objects,
        "stu_types": len(classes),
        "stu_with_typeinfo": sum(1 for c in classes if c["typeinfo"]),
        "stu_with_node_vtable": sum(1 for r in stu_rows if r[6] is not None),
        "owlib_labelled_types": sum(1 for r in stu_rows if r[2] == "owlib"),
        "fields": len(field_rows),
        "field_types": dict(Counter(json.loads(f[6])["kind"] for f in field_rows)),
        "records": len(records),
        "records_per_kind": dict(Counter(r["kind"] for r in records)),
        "winners_per_kind": dict(Counter(r["kind"] for r in records if r["winner"])),
        "records_without_vtable": sum(1 for r in records if not r["vtable"]),
        "parent_typeinfo_mismatch": stu.parent_mismatch,
        "node_vtable_class_mismatch": stu.node_vtable_mismatch,
        "pdata_functions": len(funcs),
        "obfuscated_functions": len(obf),
        "symbols": len(symbols),
        "net_messages": len((net_result or {}).get("messages") or []),
        "net_components": len((net_result or {}).get("components") or []),
        "net_protocols": len((net_result or {}).get("protocols") or []),
        "net_roles": len(net_roles),
        "net_names": len(net_symbols),
    }
    meta = {
        "schema": SCHEMA_VERSION,
        "base": base,
        "image_size": img.size,
        "dump": str(Path(dump).resolve()),
        "dump_bytes": len(img.data),
        "dump_mtime": int(os.path.getmtime(dump)),
        "owlib": str(owlib) if owlib and Path(owlib).exists() else None,
        "built": time.strftime("%Y-%m-%d %H:%M:%S"),
        "counts": counts,
        "kinds": kinds_meta,
        "slot_evidence": evidence,
        "problems": problems,
        "net_status": net_status,
        "net_method": (net_result or {}).get("method"),
    }

    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp")
    if tmp.exists():
        tmp.unlink()
    db = sqlite3.connect(tmp)
    db.executescript(SCHEMA)
    db.executemany("INSERT INTO meta VALUES (?, ?)", [(k, json.dumps(v)) for k, v in meta.items()])
    db.executemany("INSERT INTO stu VALUES (?,?,?,?,?,?,?,?,?,?)", stu_rows)
    db.executemany("INSERT INTO field VALUES (?,?,?,?,?,?,?,?,?,?)", field_rows)
    db.executemany("INSERT INTO registration VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)", reg_rows)
    db.executemany("INSERT INTO role VALUES (?,?,?,?,?,?,?,?)", roles)
    db.executemany("INSERT INTO symbol VALUES (?,?,?,?,?)", symbols)
    db.executemany("INSERT OR IGNORE INTO func VALUES (?,?,?)", funcs)
    db.executemany("INSERT OR IGNORE INTO obf VALUES (?,?,?,?)", [(b, b + s, c, a) for b, s, c, a in obf])
    for kind in ("messages", "components", "protocols"):
        rows = (net_result or {}).get(kind) or []
        db.executemany("INSERT INTO net VALUES (?,?,?)", [(kind, i, json.dumps(x)) for i, x in enumerate(rows)])
    db.executemany("INSERT INTO netrole VALUES (?,?)", net_roles)
    db.executescript(INDEXES)
    meta["seconds"] = round(time.time() - t0, 1)
    db.execute("INSERT OR REPLACE INTO meta VALUES ('seconds', ?)", (json.dumps(meta["seconds"]),))
    db.commit()
    db.close()
    replace_index(tmp, out)
    log(f"wrote {out} in {meta['seconds']} s")
    log(json.dumps(counts, indent=1))
    for line in evidence:
        log("slots: " + line)
    for line in problems[:40]:
        log("problem: " + line)
    if len(problems) > 40:
        log(f"... {len(problems) - 40} more problems (meta 'problems')")
    return meta


class IndexInUse(RuntimeError):
    """The old index file could not be replaced: another process has it open."""


def replace_index(tmp, out, tries=5, wait=1.0):
    """Move the new index over the old one. Windows refuses while any process (Index.open, sqlite3) has the
    old file open: retry briefly, then delete the new file and raise IndexInUse."""
    for attempt in range(tries):
        try:
            os.replace(tmp, out)
            return
        except PermissionError:
            if attempt + 1 < tries:
                time.sleep(wait)
    tmp.unlink(missing_ok=True)
    raise IndexInUse(
        f"the index is open in another process; close it and rebuild ({out} is unchanged, "
        f"tried {tries} times over {(tries - 1) * wait:.0f} s; the new build was discarded)"
    )


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--dump", default=str(DEFAULT_DUMP), help="image dump (default %(default)s)")
    ap.add_argument("--out", default=str(DEFAULT_OUT), help="index file (default %(default)s)")
    ap.add_argument("--owlib", default=str(DEFAULT_OWLIB), help="OWLib's STU labels (default %(default)s)")
    args = ap.parse_args(argv)
    try:
        build(args.dump, args.out, args.owlib)
    except IndexInUse as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
