"""`names` phase: names from the index (tools.owindex) and the research repo's docs/analysis/*.json
into the game IDB.

Order: index names first, then docs/analysis names (hand-made, at base 0x140000000, converted to
the database base) overwrite them; docs/analysis comments are added as repeatable comments. Names
already set by a previous run are replaced; IDA's auto names (sub_, off_, ...) are replaced too.
A name used at another address gets IDA's `_N` suffix (SN_FORCE) and is counted as a clash.
"""
import glob
import json
import os

from . import paths
from .funcs import ensure_function

BASE = 0x7FF788F10000
IDB_BASE = 0x140000000
BADADDR = 0xFFFFFFFFFFFFFFFF


def _index_names():
    from tools.owindex import Index
    idx = Index.open()
    try:
        return _names_of(idx)
    finally:
        idx.close()  # the index can be rebuilt (replaced) while the phase runs


def _names_of(idx):
    if hasattr(idx, "names"):
        out = [(int(rva), name, kind) for rva, name, kind in idx.names()]
    else:
        # Older index without names(): name every function IDA knows.
        import idautils
        out = []
        for ea in idautils.Functions():
            n = idx.name(ea - BASE)
            if n:
                out.append((ea - BASE, n, "func"))
    # Component getters the index found (tools.owindex component_accessors) but did not name.
    if hasattr(idx, "component_accessors"):
        named = {rva for rva, _, _ in out}
        for a in idx.component_accessors():
            if a.get("kind") == "getter" and a["rva"] not in named:
                out.append((int(a["rva"]), "GetComp%d" % a["component"], "func"))
                named.add(a["rva"])
    return out


def _docs():
    names, comments = [], []
    for path in sorted(glob.glob(os.path.join(paths.ANALYSIS_DIR, "*.json"))):
        data = json.load(open(path))
        for key in ("functions", "globals"):
            sect = data.get(key)
            if isinstance(sect, dict):
                for ea, name in sect.items():
                    names.append((int(ea, 16) - IDB_BASE, name, "func" if key == "functions" else "data",
                                  os.path.basename(path)))
        sect = data.get("comments")
        if isinstance(sect, dict):
            for ea, text in sect.items():
                comments.append((int(ea, 16) - IDB_BASE, text))
    return names, comments


def _set(ea, name, kind, st):
    import ida_bytes
    import ida_funcs
    import ida_name
    if not ida_bytes.is_mapped(ea):
        st["unmapped"] += 1
        return
    if kind == "func":
        ensure_function(ea)
    if ida_name.get_name(ea) == name:
        st["same"] += 1
        return
    other = ida_name.get_name_ea(BADADDR, name)
    if other not in (BADADDR, ea):
        st["clashes"] += 1
    if ida_name.set_name(ea, name, ida_name.SN_NOWARN | ida_name.SN_FORCE | ida_name.SN_NOCHECK):
        st["named"] += 1
    else:
        st["failed"].append("%#x %s" % (ea, name))


def apply(log):
    import ida_bytes
    st = {"named": 0, "same": 0, "clashes": 0, "unmapped": 0, "failed": []}
    try:
        index = _index_names()
    except (ImportError, FileNotFoundError, OSError) as e:
        raise RuntimeError("tools.owindex index unavailable (%s): build it first "
                           "(python -m tools.owindex.build)" % e)
    log("names: %d index names" % len(index))
    for rva, name, kind in index:
        _set(BASE + rva, name, kind, st)
    docs, comments = _docs()
    log("names: %d docs/analysis names, %d comments" % (len(docs), len(comments)))
    for rva, name, kind, _ in docs:
        _set(BASE + rva, name, kind, st)
    added = 0
    for rva, text in comments:
        ea = BASE + rva
        if not ida_bytes.is_mapped(ea):
            continue
        old = ida_bytes.get_cmt(ea, True) or ""
        if text not in old:
            ida_bytes.set_cmt(ea, (old + "\n" + text).strip(), True)
            added += 1
    st["comments"] = added
    st["failed"] = st["failed"][:50]
    log("names: %s" % json.dumps(st)[:600])
    return st
