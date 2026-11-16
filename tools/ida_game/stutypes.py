"""`types` phase: one IDA struct per STU class of the index, and `this` types from the registrations.

Structs: `STU_<HASH>` (always the hash; the OWLib label goes into the struct comment), size = the
descriptor's size dword, members at the descriptor's field offsets (fields of the ancestors
included), named after the OWLib field label or `m_<HASH>`. Member types by field kind:
  primitive  u8..f64 as integers/floats, teVec2/3/3A/4, teQuat, teColorRGB(A), teUUID as small
             helper structs, other primitives (teString, teEntityID, DBID, ...) as bytes;
  stu embed  `STU_<H> *` (embedded STU instances are allocated separately);
  stu inline `STU_<H>` by value when it fits before the next field;
  asset ref  unsigned __int64 (GUID); enum by width; arrays and hash maps as bytes.
Every member is clipped to the next field's offset, so a wrong size guess only shortens a member;
holes are left as gaps. +0 is the vtable when no field sits there.
"""
import json
import re

BASE = 0x7FF788F10000

HELPERS = """
struct teVec2 { float x, y; };
struct teVec3 { float x, y, z; };
struct teVec3A { float x, y, z, w; };
struct teVec4 { float x, y, z, w; };
struct teQuat { float x, y, z, w; };
struct teColorRGB { float r, g, b; };
struct teColorRGBA { float r, g, b, a; };
struct teUUID { unsigned __int8 bytes[16]; };
"""

PRIMS = {  # name: (C type, size)
    "u8": ("unsigned __int8", 1), "s8": ("__int8", 1), "u16": ("unsigned __int16", 2),
    "s16": ("__int16", 2), "u32": ("unsigned int", 4), "s32": ("int", 4),
    "u64": ("unsigned __int64", 8), "s64": ("__int64", 8), "f32": ("float", 4), "f64": ("double", 8),
    "teVec2": ("teVec2", 8), "teVec3": ("teVec3", 12), "teVec3A": ("teVec3A", 16),
    "teVec4": ("teVec4", 16), "teQuat": ("teQuat", 16), "teColorRGB": ("teColorRGB", 12),
    "teColorRGBA": ("teColorRGBA", 16), "teUUID": ("teUUID", 16),
}
UINT_BY_SIZE = {1: "unsigned __int8", 2: "unsigned __int16", 4: "unsigned int", 8: "unsigned __int64"}

_IDENT = re.compile(r"[^0-9A-Za-z_]")


def _ident(name):
    name = _IDENT.sub("_", name)
    return name if not name[0].isdigit() else "_" + name


def struct_name(h):
    return "STU_%s" % (h if isinstance(h, str) else "%08X" % h)


def _member(f, gap, sizes):
    """(C type, array count or None, size) of field f with at most `gap` bytes."""
    t = f["type"] or {}
    kind, array = t.get("kind"), t.get("array")
    if not array:
        if kind == "primitive" and t.get("name") in PRIMS:
            ctype, size = PRIMS[t["name"]]
            if size <= gap:
                return ctype, None, size
        elif kind == "stu" and t.get("hash"):
            target = struct_name(t["hash"])
            if t.get("mode") == "inline":
                size = sizes.get(t["hash"])
                if size and size <= gap:
                    return target, None, size
            elif gap >= 8:
                return target + " *", None, 8
        elif kind == "asset_ref" and gap >= 8:
            return "unsigned __int64", None, 8
        elif kind == "enum":
            for size in (8, 4, 2, 1):
                if size <= gap and (size == gap or size <= 4):
                    return UINT_BY_SIZE[size], None, size
    return "unsigned __int8", gap, gap


def declarations(idx, log=print):
    """C declarations of every STU class (pack 1, explicit padding); returns (text, stats)."""
    classes = {}
    for h in idx.stus():
        classes[h] = idx.stu(h)
    sizes = {c["hash"]: c["size"] for c in classes.values() if c.get("size")}
    out = [HELPERS]
    out += ["struct %s;" % struct_name(h) for h in classes]
    # Inline members need the member struct declared first: emit in dependency order.
    deps, bodies = {}, {}
    stats = {"classes": 0, "members": 0, "typed": 0, "no_size": 0}
    for h, c in classes.items():
        fields = {}
        for a in reversed([h] + idx.ancestors(h)[1:]):
            anc = classes.get(a) if a != h else c
            for f in (anc or {}).get("fields", []):
                if f.get("offset") is not None:
                    fields[f["offset"]] = f
        size = c.get("size") or 0
        if not size:
            stats["no_size"] += 1
            size = max([o + 8 for o in fields] or [8])
        members, used, pos, my_deps = [], set(), 0, set()
        if 0 not in fields:
            members.append("  void *vftable;")
            pos = 8
        offsets = sorted(fields)
        for i, off in enumerate(offsets):
            if off < pos or off >= size:
                continue
            nxt = offsets[i + 1] if i + 1 < len(offsets) else size
            gap = min(nxt, size) - off
            if gap <= 0:
                continue
            if off > pos:
                members.append("  unsigned __int8 _gap%X[%d];" % (pos, off - pos))
            f = fields[off]
            name = _ident(f.get("name") or "m_%s" % f["hash"])
            if name in used:
                name = "%s_%X" % (name, off)
            used.add(name)
            ctype, count, msize = _member(f, gap, sizes)
            if count is None:
                stats["typed"] += 1
                if not ctype.endswith("*") and ctype.startswith("STU_"):
                    my_deps.add(ctype)
            members.append("  %s %s%s; // +0x%X %s" % (ctype, name, "[%d]" % count if count else "",
                                                       off, json.dumps(f["type"], sort_keys=True)))
            stats["members"] += 1
            pos = off + msize
        if pos < size:
            members.append("  unsigned __int8 _gap%X[%d];" % (pos, size - pos))
        label = c["name"] if c.get("name_source") == "owlib" else ""
        bodies[struct_name(h)] = ("// %s size 0x%X\nstruct %s\n{\n%s\n};"
                                  % (label or struct_name(h), size, struct_name(h), "\n".join(members)))
        deps[struct_name(h)] = my_deps
        stats["classes"] += 1
    done = set()

    def emit(n, stack=()):
        if n in done or n not in bodies or n in stack:
            return
        for d in deps[n]:
            emit(d, stack + (n,))
        done.add(n)
        out.append(bodies[n])
    for n in bodies:
        emit(n)
    return "\n".join(out) + "\n", stats


def create_structs(idx, log):
    import ida_typeinf
    text, stats = declarations(idx, log)
    log("types: %d classes, %d members (%d typed), %d without size" % (
        stats["classes"], stats["members"], stats["typed"], stats["no_size"]))
    errors = ida_typeinf.parse_decls(None, text, None, ida_typeinf.HTI_DCL | ida_typeinf.HTI_PAK1)
    stats["parse_errors"] = errors
    log("types: parse_decls errors: %d" % errors)
    return stats, text


# Registration slot -> (argument index of the STU instance, argument name). The runtime objects
# the factories build are not the STU instances (config/action objects are 8-byte vtable-only
# objects; a state's runtime object is a separate, larger allocation), so `this` of an evaluator
# is not the STU: the STU instance arrives as an argument. Verified by hand in this image:
#   config evaluate (runtime vtable slot 4): a4 = the STU (0x7FF789BE27C0 reads STU_CD7F3617
#     +0x10 m_entity / +0x18 m_B350E4FD; 0x7FF789BDFAC0 reads STU_3832D36C +0x10 m_value);
#   action run: a4 = the STU (0x7FF78AA7C690 reads STU_6B7541FC +0x90 m_88F49C7A).
# Not typed: action execute (0x7FF789C3B040: a3 is an entity-like object, no STU argument seen),
# condition slots (4 classes, not checked).
SLOT_ARGS = {
    ("config", "evaluate"): (3, "stu"),
    ("action", "run"): (3, "stu"),
}
# State readers (state vt+0x158) and writers (vt+0x150) run on the runtime state object (`this`),
# whose property block (an STU instance) sits at a class-specific offset. The offsets and block
# types come from ORPHEUS's statescript_codecs.json (classes[H].block {offset, type_hash}).
# Each such class gets `struct StateRt_<H> { void *vftable; ...; STU_<block> block; ... }` sized
# by its factory's allocation, applied as `this` of its reader and writer.
STATE_SLOTS = ("reader", "writer")


def _common_ancestor(idx, hashes):
    chains = [[h] + idx.ancestors(h)[1:] for h in hashes]
    for a in chains[0]:
        if all(a in c for c in chains[1:]):
            return a
    return None


def _factory_alloc_size(ea):
    """Allocation size of a factory: the ecx value at its first call (`mov ecx, imm` or
    `lea ecx, [rdx+imm]` after `xor edx, edx`)."""
    import ida_ua
    size, end = None, ea + 0x40
    while ea < end:
        insn = ida_ua.insn_t()
        n = ida_ua.decode_insn(insn, ea)
        if n <= 0:
            return None
        mnem = insn.get_canon_mnem()
        if mnem == "call":
            return size
        op0, op1 = insn.ops[0], insn.ops[1]
        is_ecx = op0.type == ida_ua.o_reg and op0.reg == 1 and op0.dtype == ida_ua.dt_dword  # R_cx
        if mnem == "mov" and is_ecx and op1.type == ida_ua.o_imm:
            size = op1.value
        elif mnem == "lea" and is_ecx and op1.type == ida_ua.o_displ:
            size = op1.addr
        ea += n
    return None


def state_blocks(idx, codecs_path):
    """{class hash int: (block offset, block type hash str, runtime size or None)}"""
    import os
    if not os.path.exists(codecs_path):
        return {}
    data = json.load(open(codecs_path))
    out = {}
    for h, c in data.get("classes", {}).items():
        b = c.get("block") or {}
        if b.get("offset") is None or not b.get("type_hash"):
            continue
        size = None
        for r in idx.stu(h)["registrations"] if idx.stu(h) else []:
            if r["winner"] and r["kind"] == "state" and r["factory_rva"]:
                size = _factory_alloc_size(BASE + r["factory_rva"])
        out[int(h, 16)] = (b["offset"], b["type_hash"].upper(), size)
    return out


def state_decls(blocks, sizes):
    out = []
    for h, (off, bt, size) in sorted(blocks.items()):
        bsize = sizes.get(bt)
        if not bsize:
            continue
        total = max(size or 0, off + bsize)
        members = ["  void *vftable;"]
        if off > 8:
            members.append("  unsigned __int8 _gap8[%d];" % (off - 8))
        members.append("  %s block; // +0x%X property block" % (struct_name(bt), off))
        if total > off + bsize:
            members.append("  unsigned __int8 _gap%X[%d];" % (off + bsize, total - off - bsize))
        out.append("// runtime state object of STU_%08X (size 0x%X%s)\nstruct StateRt_%08X\n{\n%s\n};"
                   % (h, total, "" if size else ", factory size unknown", h, "\n".join(members)))
    return "\n".join(out) + "\n"


def slot_targets(idx, blocks):
    """{function RVA: (struct name, arg index, arg name, slot, n classes)}. A function shared by
    several classes gets their closest common ancestor (STU args) or is skipped unless all its
    classes have the same block (state readers/writers)."""
    users = {}
    for r in idx.records():
        for slot, rva in r["slots"].items():
            key = (r["kind"], slot)
            if rva and (key in SLOT_ARGS or (r["kind"] == "state" and slot in STATE_SLOTS)):
                users.setdefault((rva, key), set()).add(int(r["class_hash"], 16))
    out = {}
    for (rva, key), hashes in users.items():
        if key in SLOT_ARGS:
            h = next(iter(hashes)) if len(hashes) == 1 else _common_ancestor(idx, hashes)
            if h is not None:
                out[rva] = (struct_name(h), SLOT_ARGS[key][0], SLOT_ARGS[key][1], "%s.%s" % key, len(hashes))
            continue
        kinds = {blocks.get(h, (None, None, None))[:2] for h in hashes}
        if len(kinds) == 1 and None not in next(iter(kinds)):
            h = min(hashes)
            out[rva] = ("StateRt_%08X" % h, 0, "this", "%s.%s" % key, len(hashes))
    return out


def _set_arg(ea, argi, tname, argname):
    """Give argument `argi` of the function at ea the type `tname *`, keeping the rest of the
    prototype Hex-Rays derives. Returns None or an error string."""
    import ida_hexrays
    import ida_typeinf
    try:
        cfunc = ida_hexrays.decompile(ea)
    except ida_hexrays.DecompilationFailure as e:
        return "decompile: %s" % e
    if cfunc is None:
        return "decompile failed"
    tif = ida_typeinf.tinfo_t()
    cfunc.get_func_type(tif)
    fi = ida_typeinf.func_type_data_t()
    if not tif.get_func_details(fi):
        return "no func details"
    while len(fi) <= argi:
        a = ida_typeinf.funcarg_t()
        a.type = ida_typeinf.tinfo_t(ida_typeinf.BT_INT64)
        fi.push_back(a)
    st = ida_typeinf.tinfo_t()
    if not st.get_named_type(None, tname):
        return "no type %s" % tname
    ptr = ida_typeinf.tinfo_t()
    ptr.create_ptr(st)
    fi[argi].type = ptr
    fi[argi].name = argname
    new = ida_typeinf.tinfo_t()
    if not new.create_func(fi):
        return "create_func failed"
    if not ida_typeinf.apply_tinfo(ea, new, ida_typeinf.TINFO_DEFINITE):
        return "apply_tinfo failed"
    ida_hexrays.mark_cfunc_dirty(ea)
    return None


def apply(log, codecs_path):
    from .names import ensure_function
    import ida_hexrays
    import ida_typeinf
    from tools.owindex import Index
    if not ida_hexrays.init_hexrays_plugin():
        raise RuntimeError("Hex-Rays not available")
    idx = Index.open()
    stats, _ = create_structs(idx, log)
    blocks = state_blocks(idx, codecs_path)
    sizes = {"%08X" % h: (idx.stu(h) or {}).get("size") for h in idx.stus()}
    text = state_decls(blocks, sizes)
    stats["state_structs"] = text.count("struct StateRt_")
    stats["state_parse_errors"] = ida_typeinf.parse_decls(None, text, None,
                                                          ida_typeinf.HTI_DCL | ida_typeinf.HTI_PAK1)
    log("types: %d StateRt_ structs (codec data %s), parse errors %d"
        % (stats["state_structs"], codecs_path, stats["state_parse_errors"]))
    targets = slot_targets(idx, blocks)
    log("types: %d slot functions to type" % len(targets))
    typed, failed, by_slot = 0, {}, {}
    for n, (rva, (tname, argi, argname, slot, nclasses)) in enumerate(sorted(targets.items())):
        ea = BASE + rva
        if not ensure_function(ea):
            failed[hex(ea)] = "not a function entry"
            continue
        err = _set_arg(ea, argi, tname, argname)
        if err:
            failed[hex(ea)] = err
        else:
            typed += 1
            by_slot[slot] = by_slot.get(slot, 0) + 1
        if n % 200 == 199:
            log("types: %d/%d typed (%d failed)" % (typed, len(targets), len(failed)))
    stats.update({"slot_functions": len(targets), "functions_typed": typed, "typed_by_slot": by_slot,
                  "failed": len(failed), "failed_examples": dict(list(failed.items())[:20])})
    log("types: %s" % json.dumps(stats)[:800])
    return stats
