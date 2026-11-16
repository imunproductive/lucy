"""The client's console variables (engine settings), read from the in-game image dump.

Registry: a fixed array of 1,601 object pointers in .data (0x7FF78BD3B6B0 in the 0x7FF788F10000 dump; the
registration code pushes each object there, e.g. 0x7FF78A70209D for hash 0x027006C2). Object layout:
  +0x00 vtable (one per value type)   +0x08 the image's shared empty string (the client keeps no names)
  +0x10 hash dword                    +0x14 dword (1 on 7 variables, else 0)
  +0x20 current value, then the default: bool byte +0x20/+0x21, 32-bit +0x20/+0x24, 64-bit and double
        +0x20/+0x28, string char[0x100] +0x20/+0x120; lists and maps keep heap pointers (+0x20, default +0x30
        or +0x48) that the image dump doesn't hold.
The value type comes from the vtable: slot 1 compares the value with the default (`movzx eax, byte
[rcx+21h]` = bool, a reference to +0x120 = string), slot 5 tail-jumps to the text parser, whose
(XOR-encrypted) sscanf formats give the number type ('%d' int32, '%u' uint32, '%lld' int64, '%llu'
uint64, '%lg' double). The values are those of the dump's moment (in a match).
"""

import json
import re
import struct
from collections import Counter

import numpy as np
from iced_x86 import Decoder, Mnemonic

_NUM_FORMATS = {"%d": "int32", "%u": "uint32", "%lld": "int64", "%llu": "uint64", "%lg": "double"}
_PACK = {"int32": "<i", "uint32": "<I", "int64": "<q", "uint64": "<Q", "double": "<d"}


def _qwords(img, section):
    lo, hi = img.sections[section]
    return lo, np.frombuffer(img.data, dtype="<u8", count=(hi - lo) // 8, offset=lo)


def empty_string(img):
    """RVA of the image's shared empty string: the most common .data pointer into .rdata pointing at 0."""
    _, seg = _qwords(img, ".data")
    lo, hi = (img.base + x for x in img.sections[".rdata"])
    vals, counts = np.unique(seg[(seg >= lo) & (seg < hi)], return_counts=True)
    for k in np.argsort(-counts)[:16]:
        r = int(vals[k]) - img.base
        if img.data[r] == 0:
            return r
    raise ValueError("no shared empty string in .data")


def find_registry(img):
    """(RVA, count) of the registry: the longest run of .data pointers to `{vtable in .rdata, empty
    string, hash}` objects."""
    empty = img.base + empty_string(img)
    d0, seg = _qwords(img, ".data")
    d1 = img.sections[".data"][1]
    r0, r1 = (img.base + x for x in img.sections[".rdata"])
    ok = np.zeros(len(seg), dtype=bool)
    for k in np.nonzero((seg >= img.base + d0) & (seg < img.base + d1 - 0x30))[0]:
        vt, s, h = struct.unpack_from("<QQI", img.data, int(seg[k]) - img.base)
        ok[k] = s == empty and r0 <= vt < r1 and h != 0
    best, start, run = (0, 0), 0, 0
    for k in np.nonzero(ok)[0]:
        k = int(k)
        if run and k == start + run:
            run += 1
        else:
            start, run = k, 1
        if run > best[1]:
            best = (start, run)
    if best[1] < 100:
        raise ValueError("no console variable registry in .data")
    return d0 + best[0] * 8, best[1]


def _estring(img, rva):
    """An XOR-encrypted string object {key[8], length:24|flag:8, chars}: its text (decrypted when the
    flag byte says it still is)."""
    key = img.data[rva:rva + 8]
    n = struct.unpack_from("<I", img.data, rva + 8)[0] & 0xFFFFFF
    raw = img.data[rva + 12:rva + 12 + min(n, 64)]
    if img.data[rva + 11]:
        raw = bytes(c ^ key[i & 7] for i, c in enumerate(raw))
    return raw.decode("latin-1")


def _strings(img, va, limit=0x140, depth=0):
    """Strings a function loads with `lea` up to int3 padding (both branches of a parser), and with
    depth > 0 those of its direct callees: encrypted .data strings, plain .rdata ones."""
    d0, d1 = img.sections[".data"]
    r0, r1 = img.sections[".rdata"]
    out, callees = [], []
    off = va - img.base
    for ins in Decoder(64, img.data[off:off + limit], ip=va):
        if ins.mnemonic == Mnemonic.LEA and ins.is_ip_rel_memory_operand:
            t = ins.ip_rel_memory_address - img.base
            if d0 + 12 <= t < d1:
                s = _estring(img, t - 12)
                if s and all(32 <= ord(c) < 127 for c in s):
                    out.append(s)
            elif r0 <= t < r1:
                m = re.match(rb"[\x20-\x7e]{2,}", img.data[t:t + 64])
                if m:
                    out.append(m.group().decode())
        elif ins.mnemonic in (Mnemonic.CALL, Mnemonic.JMP) and img.intext(ins.near_branch_target):
            callees.append(ins.near_branch_target)
        if ins.mnemonic == Mnemonic.INT3:
            break
    if depth > 0:
        for c in callees[:8]:
            out += _strings(img, c, limit, depth - 1)
    return out


def vtable_type(img, vt):
    """('bool'|'int32'|'uint32'|'int64'|'uint64'|'double'|'string'|'list', hint) of a variable vtable.
    hint: for lists, the format strings of its text writer and parser (the element shape)."""
    cmp_va, write_va, parse_va = (img.q(vt + 8 * s) for s in (1, 3, 5))
    head = img.data[cmp_va - img.base:cmp_va - img.base + 24]
    if head.startswith(b"\x0f\xb6\x41\x21"):  # movzx eax, byte [rcx+21h]
        return "bool", None
    if b"\x20\x01\x00\x00" in head:  # lea rdx, [rcx+120h]: inline char buffer and its default
        return "string", None
    target = None
    for ins in Decoder(64, img.data[parse_va - img.base:parse_va - img.base + 16], ip=parse_va):
        if ins.mnemonic == Mnemonic.JMP:
            target = ins.near_branch_target
            break
    fmts = _strings(img, target) if target else []
    for f in fmts:
        if f in _NUM_FORMATS:
            return _NUM_FORMATS[f], None
    hint = sorted(set(_strings(img, write_va, 0x200, depth=1) + fmts))
    return "list", ", ".join(hint) or None


def _cstr(b):
    return b.split(b"\0", 1)[0].decode("utf-8", "replace")


def _value(img, obj, kind):
    o = obj - img.base
    if kind == "bool":
        return bool(img.data[o + 0x20]), bool(img.data[o + 0x21])
    if kind in _PACK:
        fmt = _PACK[kind]
        step = struct.calcsize(fmt)
        return struct.unpack_from(fmt, img.data, o + 0x20)[0], struct.unpack_from(fmt, img.data, o + 0x20 + step)[0]
    if kind == "string":
        return _cstr(img.data[o + 0x20:o + 0x120]), _cstr(img.data[o + 0x120:o + 0x220])
    return None, None


def scan(img):
    """{'registry_rva', 'count', 'vars': [{'index', 'rva', 'hash', 'type', 'hint', 'value', 'default',
    'vtable_rva', 'flag', 'size'}]} in registry order. Values: bool/int/float/str; None for lists.
    size: the usual distance to the next object for the variable's vtable (objects are packed in .data)."""
    reg, count = find_registry(img)
    ptrs = [img.q(img.base + reg + 8 * i) for i in range(count)]
    order = sorted(ptrs)
    gaps = {}
    for a, b in zip(order, order[1:]):
        gaps.setdefault(img.q(a), Counter())[b - a] += 1
    types, out = {}, []
    for i, p in enumerate(ptrs):
        vt = img.q(p)
        if vt not in types:
            types[vt] = vtable_type(img, vt)
        kind, hint = types[vt]
        value, default = _value(img, p, kind)
        out.append({
            "index": i, "rva": p - img.base, "hash": img.d(p + 0x10), "type": kind, "hint": hint,
            "value": value, "default": default, "vtable_rva": vt - img.base, "flag": img.d(p + 0x14),
            "size": gaps[vt].most_common(1)[0][0] if vt in gaps else None,
        })  # fmt: skip
    return {"registry_rva": reg, "count": count, "vars": out}


def show(v):
    """Display text of a value."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (float, str)):
        return repr(v)
    return "-" if v is None else str(v)


# ------------------------------------------------------------------ the built table (tools/owsearch/build.py)

def store(db, result):
    """Write scan()'s result into the cvar table of the open sqlite db."""
    db.execute("CREATE TABLE cvar (idx INTEGER PRIMARY KEY, rva INTEGER, hash INTEGER, type TEXT, hint TEXT,"
               " value TEXT, dflt TEXT, vtable_rva INTEGER, flag INTEGER, size INTEGER)")
    db.executemany(
        "INSERT INTO cvar VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ((v["index"], v["rva"], v["hash"], v["type"], v["hint"], json.dumps(v["value"]), json.dumps(v["default"]),
          v["vtable_rva"], v["flag"], v["size"]) for v in result["vars"]),
    )  # fmt: skip
    db.execute("CREATE INDEX cvar_hash ON cvar(hash)")
    db.execute("CREATE INDEX cvar_rva ON cvar(rva)")


def _row(r):
    idx, rva, h, kind, hint, value, dflt, vt, flag, size = r
    return {"index": idx, "rva": rva, "hash": h, "type": kind, "hint": hint, "value": json.loads(value),
            "default": json.loads(dflt), "vtable_rva": vt, "flag": flag, "size": size}  # fmt: skip


def lookup(db, key):
    """Variables by hash (int, '027006C2', '0x027006C2') or registry index ('#637')."""
    s = str(key).strip()
    if s.startswith("#"):
        where, arg = "idx = ?", int(s[1:], 0)
    else:
        where, arg = "hash = ?", key if isinstance(key, int) else int(s[2:] if s.lower().startswith("0x") else s, 16)
    return [_row(r) for r in db.execute(f"SELECT * FROM cvar WHERE {where}", (arg,))]


def all_vars(db):
    return [_row(r) for r in db.execute("SELECT * FROM cvar ORDER BY idx")]


def containing(db, rva):
    """The variable whose object holds rva, or None."""
    r = db.execute("SELECT * FROM cvar WHERE rva <= ? ORDER BY rva DESC LIMIT 1", (rva,)).fetchone()
    if r and rva < r[1] + (r[9] or 0x28):
        return _row(r)
    return None
