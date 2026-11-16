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

BASE = 0x7FF788F10000
IDB_BASE = 0x140000000
BADADDR = 0xFFFFFFFFFFFFFFFF


def _index_names():
    from tools.owindex import Index
    idx = Index.open()
    if hasattr(idx, "names"):
        return [(int(rva), name, kind) for rva, name, kind in idx.names()]
    # Older index without names(): name every function IDA knows.
    import idautils
    out = []
    for ea in idautils.Functions():
        n = idx.name(ea - BASE)
        if n:
            out.append((ea - BASE, n, "func"))
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


def ensure_function(ea):
    """Make ea a function entry. Stubs reached only through vtables (TypeTag_ `lea rax; ret`,
    small constructors) sit between random filler bytes, and auto-analysis often turns them
    into part of a `dq` array, or decodes the filler as code that runs across the stub (with a
    function built on it): undefine the item covering ea (and such a function), create code and
    the function. A real instruction head inside another function is left alone."""
    import ida_auto
    import ida_bytes
    import ida_funcs
    import idc
    f = ida_funcs.get_func(ea)
    is_head = ida_bytes.is_code(ida_bytes.get_flags(ea)) and ida_bytes.get_item_head(ea) == ea
    if f is not None:
        if f.start_ea == ea:
            return True
        if is_head:
            return False
        # A decode across ea (filler bytes run into the stub): cut it off the function holding it.
        head = ida_bytes.get_item_head(ea)
        chunk = ida_funcs.get_fchunk(ea)
        if chunk.start_ea != f.start_ea:
            ida_funcs.remove_func_tail(f, chunk.start_ea)
        elif head > f.start_ea:
            ida_funcs.set_func_end(f.start_ea, head)
        else:
            ida_funcs.del_func(f.start_ea)
    if not is_head:
        ida_bytes.del_items(ida_bytes.get_item_head(ea), ida_bytes.DELIT_SIMPLE, 1)
        if not idc.create_insn(ea):
            return False
    ok = ida_funcs.add_func(ea)
    ida_auto.auto_wait()
    return ok


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
