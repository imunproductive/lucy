"""Function bounds in the game IDB: make an address a function entry, find the start of the
function that holds an address when IDA has none there (Rose-hidden code, vtable stubs between
filler bytes), read vtables, and the `vslots` build phase.

Start of the code holding an address, in this order:
1. the image's .pdata entry that covers it (chained unwind entries followed to their root);
2. the nearest earlier address that something references (a vtable slot, a call, a jump) from
   which straight decoding reaches the address without passing a `ret`/`jmp`;
3. the address itself.
"""
import struct

BASE = 0x7FF788F10000
MAX_BACK = 0x1000      # how far before an address define_at() looks for a referenced start
MAX_VTABLE = 2048      # slots read from a vtable without a known size
UNW_FLAG_CHAININFO = 4

_pdata = None  # (begins, ends, unwind rvas) of the .pdata segment, loaded once per process


def ensure_function(ea, split=False):
    """Make ea a function entry. Stubs reached only through vtables (TypeTag_ `lea rax; ret`,
    small constructors) sit between random filler bytes, and auto-analysis often turns them
    into part of a `dq` array, or decodes the filler as code that runs across the stub (with a
    function built on it): undefine the item covering ea (and such a function), create code and
    the function. A real instruction head inside another function is left alone, unless `split`
    (ea is known to be an entry, e.g. a vtable slot): then it is cut off that function too."""
    import ida_auto
    import ida_bytes
    import ida_funcs
    import idc
    f = ida_funcs.get_func(ea)
    is_head = ida_bytes.is_code(ida_bytes.get_flags(ea)) and ida_bytes.get_item_head(ea) == ea
    if f is not None:
        if f.start_ea == ea:
            return True
        if is_head and not split:
            return False
        # A decode across ea (filler bytes run into the stub), or a function IDA ran into ea:
        # cut it off the function holding it.
        head = ida_bytes.get_item_head(ea)
        chunk = ida_funcs.get_fchunk(ea)
        if chunk.start_ea != f.start_ea:
            ida_funcs.remove_func_tail(f, chunk.start_ea)
        elif head > f.start_ea:
            ida_funcs.set_func_end(f.start_ea, head)
        else:
            ida_funcs.del_func(f.start_ea)
    if not is_head:
        _undefine_flow(ea)
        if not idc.create_insn(ea):
            return False
    ok = ida_funcs.add_func(ea)
    ida_auto.auto_wait()
    return ok


def _decode(ea):
    import ida_ua
    insn = ida_ua.insn_t()
    return insn if ida_ua.decode_insn(insn, ea) > 0 else None


def _stops(insn):
    import ida_idp
    return bool(ida_idp.has_insn_feature(insn.itype, ida_idp.CF_STOP))


def _undefine_flow(ea, limit=0x100):
    """Undefine the items that straight decoding from ea runs over (data, and code decoded at
    other boundaries), up to the first `ret`/`jmp` or another function's code, so that IDA can
    make code there."""
    import ida_bytes
    import ida_funcs
    starts, end = set(), ea
    while end < ea + limit:
        insn = _decode(end)
        if insn is None:
            break
        starts.add(end)
        end += insn.size
        if _stops(insn):
            break
    x = ea
    while x < end:
        head = ida_bytes.get_item_head(x)
        size = max(1, ida_bytes.get_item_size(head))
        if not ida_bytes.is_code(ida_bytes.get_flags(head)) or head not in starts:
            f = ida_funcs.get_func(head) if head > ea else None
            if f is not None:
                if f.start_ea != head:
                    break
                ida_funcs.del_func(head)  # a function IDA started inside one of our instructions
            ida_bytes.del_items(head, ida_bytes.DELIT_SIMPLE, size)
        x = head + size


def _load_pdata():
    global _pdata
    if _pdata is None:
        import ida_bytes
        import ida_segment
        seg = ida_segment.get_segm_by_name(".pdata")
        begins, ends, unwind = [], [], []
        if seg is not None:
            raw = ida_bytes.get_bytes(seg.start_ea, seg.end_ea - seg.start_ea) or b""
            for b, e, u in struct.iter_unpack("<III", raw[: len(raw) // 12 * 12]):
                if b == 0:
                    break
                begins.append(b)
                ends.append(e)
                unwind.append(u)
        _pdata = (begins, ends, unwind)
    return _pdata


def pdata_root(ea):
    """VA of the function whose .pdata entry covers ea (its root, for chained entries), or None."""
    import bisect
    import ida_bytes
    begins, ends, unwind = _load_pdata()
    rva = ea - BASE
    i = bisect.bisect_right(begins, rva) - 1
    if i < 0 or ends[i] <= rva:
        return None
    for _ in range(32):
        u = unwind[i]
        info = ida_bytes.get_bytes(BASE + u, 4)
        if not info or not (info[0] >> 3) & UNW_FLAG_CHAININFO:
            break
        chained = BASE + u + 4 + 2 * ((info[2] + 1) & ~1)
        b = ida_bytes.get_dword(chained)
        j = bisect.bisect_left(begins, b)
        if j >= len(begins) or begins[j] != b:
            return BASE + b
        i = j
    return BASE + begins[i]


def _reaches(start, ea):
    """Straight decoding from start lands on or covers ea without a `ret`/`jmp` before it."""
    x = start
    while x <= ea:
        insn = _decode(x)
        if insn is None:
            return False
        if x + insn.size > ea:
            return True
        if _stops(insn):
            return False
        x += insn.size
    return False


def code_start(ea):
    """Start of the code that holds ea when no IDA function does (see the module docstring)."""
    import ida_funcs
    import idautils
    root = pdata_root(ea)
    if root is not None:
        return root
    prev = ida_funcs.get_prev_func(ea)
    low = max(ea - MAX_BACK, prev.end_ea if prev is not None else 0)
    for x in range(ea, low - 1, -1):
        if any(True for _ in idautils.XrefsTo(x, 0)) and _reaches(x, ea):
            return x
    return ea


def define_at(ea):
    """Make the code holding ea a function (in memory; the caller saves). Returns the function's
    start VA, or None when IDA cannot make one."""
    import ida_funcs
    f = ida_funcs.get_func(ea)
    if f is not None:
        return f.start_ea
    start = code_start(ea)
    if not ensure_function(start):
        return None
    f = ida_funcs.get_func(ea)
    return f.start_ea if f is not None else None


def is_text(ea):
    import ida_segment
    seg = ida_segment.getseg(ea)
    return seg is not None and bool(seg.perm & ida_segment.SEGPERM_EXEC)


def vtable(ea, n=None):
    """Slot targets of the vtable at ea: n slots, or (n=None) the run of code pointers that starts
    there (at most MAX_VTABLE). A run continues through secondary vtables (multiple inheritance)
    and, where the image has no RTTI pointer between them, into the next class's vtable: every
    pointer of such a run is still a vtable slot, only the class is not the one at ea."""
    import ida_bytes
    out = []
    for k in range(n if n is not None else MAX_VTABLE):
        t = ida_bytes.get_qword(ea + 8 * k)
        if not is_text(t):
            if n is None:
                break
            continue
        out.append(t)
    return out


def _vslot_targets(idx):
    """{VA: reason} of the index's runtime vtable slots, component and message vtables, and their
    factories, constructors and handlers."""
    out = {}

    def add(rva, why):
        if rva:
            out.setdefault(BASE + rva, why)

    def add_vtable(rva, n, why):
        if rva:
            for k, t in enumerate(vtable(BASE + rva, n)):
                out.setdefault(t, "%s slot %d" % (why, k))

    for r in idx.records():
        why = "%s STU_%s" % (r["kind"], r["class_hash"])
        add_vtable(r["runtime_vtable_rva"], r["vtable_slots"], why + " runtime vtable")
        for key in ("factory_rva", "construct_rva", "destroy_rva"):
            add(r[key], why + " " + key[:-4])
    for c in idx.components():
        why = "component %d" % c["id"]
        add_vtable(c.get("vtable_rva"), None, why + " vtable")
        add_vtable(c.get("factory_vtable_rva"), None, why + " factory vtable")
        for key in ("construct_rva", "create_rva", "factory_ctor_rva"):
            add(c.get(key), why + " " + key[:-4])
    for m in idx.messages():
        why = "message %s" % m["id"]
        add_vtable(m.get("class_vtable_rva"), None, why + " vtable")
        for key in ("handler_rva", "reader_rva", "dispatcher_rva"):
            add(m.get(key), why + " " + key[:-4])
    return out


def apply_vslots(log):
    """`vslots` phase: make every target of _vslot_targets() a function entry, cutting it off a
    function IDA ran into it (filler decoded across a stub, or a neighbour that falls through)."""
    import ida_funcs
    from tools.owindex import Index
    idx = Index.open()
    try:
        targets = _vslot_targets(idx)
    finally:
        idx.close()  # the index can be rebuilt (replaced) while this phase runs
    log("vslots: %d slot/handler targets" % len(targets))
    st = {"targets": len(targets), "already": 0, "defined": 0, "split_off": 0, "failed": 0,
          "defined_examples": [], "failed_examples": []}
    for n, (ea, why) in enumerate(sorted(targets.items())):
        f = ida_funcs.get_func(ea)
        if f is not None and f.start_ea == ea:
            st["already"] += 1
            continue
        ensure_function(ea, split=True)
        g = ida_funcs.get_func(ea)
        if g is not None and g.start_ea == ea:
            st["split_off" if f is not None else "defined"] += 1
            if len(st["defined_examples"]) < 20:
                st["defined_examples"].append("%#x %s" % (ea, why))
        else:
            st["failed"] += 1
            if len(st["failed_examples"]) < 20:
                st["failed_examples"].append("%#x %s" % (ea, why))
        if n % 2000 == 1999:
            log("vslots: %d/%d (%d defined, %d split off)" % (n + 1, len(targets), st["defined"], st["split_off"]))
    log("vslots: %s" % {k: v for k, v in st.items() if not k.endswith("examples")})
    return st
