"""Entity component accessors and component mirrors (Overwatch 1.74 client, build 104319).

An entity keeps its components in a bitset at entity +0x88 (136) and a packed pointer array. Code reads a
component through a tiny helper that writes the component's bit:

    getter(u64 *mask, u64 *low_mask, u32 *shift, u32 *word)   ; mask = 1 << shift, low mask = mask - 1
    component id = word * 64 + shift

The caller tests `bitset[word] & mask`, counts the bits below it (popcount of `bitset[word] & low_mask` plus a
per-word base byte) and decrypts the pointer at that index. `has(entity, n)` (`movzx eax, dl; and eax, 0x3F;
... mov rax, [rcx + rax*8 + 0x88]; bt rax, r8; setb al; ret`) tests bit n; its callers pass n in edx.

Type id stub: `mov byte [rcx], N; mov rax, rcx; ret` hands out component id N where code looks a component type
up by id: the factory constructors and the pools (one list of all instances per id; a pool function writes the
list's {first, end} headers to its out parameter).

Mirror: the STU instance a component constructor builds in place (the networked fields). Its offset inside the
component is the `this` offset the constructor chain passes to the STU instance initialiser together with the
mirror's descriptor (`lea rcx, [descriptor]; mov rdx, this + offset`).

All addresses are RVAs; `img` is tools.owindex.net._Image.
"""

from __future__ import annotations

import re
import struct
from collections import Counter

import numpy as np

BITSET_OFFSET = 0x88
_M64 = (1 << 64) - 1
_ARG = {"rcx": "mask", "rdx": "low", "r8": "shift", "r9": "word"}
_R64 = {"eax": "rax", "ecx": "rcx", "edx": "rdx", "ebx": "rbx", "esi": "rsi", "edi": "rdi", "ebp": "rbp"}
_R64.update({f"r{i}d": f"r{i}" for i in range(8, 16)})
_STORE = re.compile(r"(?:qword ptr \[(rcx|rdx)\]|dword ptr \[(r8|r9)\]), (\w+)$")


def _r64(r):
    return _R64.get(r, r)


def _getter_at(img, start, end):
    """(mask, low, shift, word) when [start, end) is exactly a getter body (only constant loads into registers and
    the four stores), else None."""
    regs, out = {}, {}
    pos = start
    for i in img.cs.disasm(img.b[start:end], start):
        m, o = i.mnemonic, i.op_str
        pos = i.address + i.size
        if m == "ret":
            break
        dst, _, src = o.partition(", ")
        if m in ("mov", "movabs") and not dst.endswith("]") and re.fullmatch(r"0x[0-9a-f]+|\d+", src):
            regs[_r64(dst)] = int(src, 0) & _M64
        elif m == "xor" and dst == src and not dst.endswith("]"):
            regs[_r64(dst)] = 0
        elif m == "mov" and (s := _STORE.match(o)):
            v = s.group(3)
            v = int(v, 0) & _M64 if v[0].isdigit() else regs.get(_r64(v))
            if v is None:
                return None
            out[_ARG[s.group(1) or s.group(2)]] = v
        else:
            return None
    if pos != end or len(out) != 4:
        return None
    mask, low, shift, word = out["mask"], out["low"], out["shift"] & 0xFFFFFFFF, out["word"] & 0xFFFFFFFF
    if shift >= 64 or word >= 8 or mask != 1 << shift or low != mask - 1:
        return None
    return mask, low, shift, word


def getters(img):
    """Every component getter in .text: [{"rva", "component", "mask", "low_mask", "shift", "word"}]. A getter
    ends in `mov dword [r9], imm32; ret`; its start is the latest one from which the body decodes as a getter."""
    t0, t1 = img.text
    out = []
    for m in re.finditer(rb"\x41\xC7\x01....\xC3", img.b[t0:t1], re.S):
        end = m.end() + t0
        for k in range(14, 56):
            g = _getter_at(img, end - k, end)
            if g is not None:
                mask, low, shift, word = g
                out.append({"rva": end - k, "component": word * 64 + shift, "mask": mask, "low_mask": low,
                            "shift": shift, "word": word})
                break
    return out


def has_helpers(img):
    """Functions testing one bit of the component bitset: `mov rax, [rcx + rax*8 + 0x88]; bt rax, r..;
    setb al; ret` with the bit number from dl."""
    t0, t1 = img.text
    out = []
    pat = rb"\x48\x8B\x84\xC1" + struct.pack("<I", BITSET_OFFSET) + rb"[\x48\x4C]\x0F\xA3[\xC0-\xFF]\x0F\x92\xC0\xC3"
    for m in re.finditer(pat, img.b[t0:t1], re.S):
        p = m.start() + t0
        back = img.b[p - 32:p]
        start = p - 32 + back.rfind(b"\xCC") + 1 if b"\xCC" in back else None
        if start is None:
            continue
        ins = list(img.cs.disasm(img.b[start:p], start))
        if ins and ins[0].mnemonic == "movzx" and ins[0].op_str.endswith(", dl") and sum(i.size for i in ins) == p - start:
            out.append(start)
    return out


def _edx_before(img, site):
    """The constant in edx/dl at a call site (the last write before it on the decodes that land on the site),
    or None."""
    votes = Counter()
    for k in range(4, 40):
        ins = list(img.cs.disasm(img.b[site - k:site], site - k))
        if not ins or ins[-1].address + ins[-1].size != site:
            continue
        val = None
        for i in ins:
            dst, _, src = i.op_str.partition(", ")
            if dst in ("rdx", "edx", "dx", "dl"):
                val = int(src, 0) if i.mnemonic == "mov" and re.fullmatch(r"0x[0-9a-f]+|\d+", src) else "?"
            elif i.mnemonic == "call":
                val = "?"
        votes[val] += 1
    votes.pop(None, None)
    if not votes:
        return None
    val, _ = votes.most_common(1)[0]
    return val if isinstance(val, int) else None


def id_stubs(img):
    """`mov byte [rcx], N; mov rax, rcx; ret`: [(rva, N)]."""
    t0, t1 = img.text
    return [(m.start() + t0, m.group(1)[0]) for m in re.finditer(rb"\xC6\x01(.)\x48\x8B\xC1\xC3", img.b[t0:t1], re.S)]


def accessors(img):
    """Rows for the index: getters ("getter", rva = helper), has helpers ("has", component None), type id stubs
    ("id stub"), and the call sites of all three ("getter call" / "has call" / "id stub call", rva = site,
    func_rva = containing function, accessor_rva)."""
    rows = []
    for g in getters(img):
        rows.append({"kind": "getter", "rva": g["rva"], "component": g["component"], "func_rva": g["rva"],
                     "accessor_rva": g["rva"], "detail": g})
        for site in img.callers(g["rva"]):
            rows.append({"kind": "getter call", "rva": site, "component": g["component"],
                         "func_rva": img.func_start(site), "accessor_rva": g["rva"]})
    for h in has_helpers(img):
        rows.append({"kind": "has", "rva": h, "component": None, "func_rva": h, "accessor_rva": h})
        for site in img.callers(h):
            rows.append({"kind": "has call", "rva": site, "component": _edx_before(img, site),
                         "func_rva": img.func_start(site), "accessor_rva": h})
    for s, n in id_stubs(img):
        rows.append({"kind": "id stub", "rva": s, "component": n, "func_rva": s, "accessor_rva": s})
        for site in img.callers(s):
            rows.append({"kind": "id stub call", "rva": site, "component": n, "func_rva": img.func_start(site),
                         "accessor_rva": s})
    return rows


def table_starts(img):
    """Starts of function pointer tables (vtables): aligned .rdata/.data addresses whose qword points into .text
    and that code references rip-relative ("code") or a data pointer points at ("data"). -> {rva: source}."""
    t0, t1 = img.text
    lo, hi = np.uint64(img.base + t0), np.uint64(img.base + t1)

    def code_slots(r):
        r = np.asarray(sorted(r), dtype=np.int64)
        r = r[(r % 8 == 0) & (r >= img.rdata[0]) & (r < img.data[1])]
        v = img.q64[r // 8]
        return {int(x) for x in r[(v >= lo) & (v < hi)]}

    out = {r: "data" for r in code_slots(np.unique(img.ptr_val))}
    out.update({r: "code" for r in code_slots(img.ripref.keys())})
    return out


def _rip(i):
    m = re.search(r"\[rip ([+-]) (0x[0-9a-f]+|\d+)\]", i.op_str)
    if not m:
        return None
    d = int(m.group(2), 0)
    return i.address + i.size + (d if m.group(1) == "+" else -d)


def mirror_offset(img, ctor, descriptor, depth=3):
    """Offset of the mirror inside the component: follow `this` (rcx at the constructor's entry) through register
    copies, `lea reg, [this + k]` and calls with rcx = this + k, to the call made with rcx = the mirror's
    descriptor and rdx = this + offset. None when not found."""
    if ctor is None or descriptor is None:
        return None
    work, seen = [(ctor, 0, 0)], set()
    while work:
        fn, base, d = work.pop(0)
        if (fn, base) in seen or d > depth:
            continue
        seen.add((fn, base))
        this = {"rcx": base}
        desc_in_rcx = False
        for i in img.walk(fn, 600)[0]:
            m, o = i.mnemonic, i.op_str
            dst, _, src = o.partition(", ")
            dst = _r64(dst)
            if m == "call":
                if desc_in_rcx and "rdx" in this:
                    return this["rdx"]
                if o.startswith("0x") and "rcx" in this:
                    work.append((int(o, 16), this["rcx"], d + 1))
                for r in ("rax", "rcx", "rdx", "r8", "r9", "r10", "r11"):
                    this.pop(r, None)
                desc_in_rcx = False
                continue
            if m == "lea" and dst == "rcx" and _rip(i) == descriptor:
                desc_in_rcx = True
                continue
            if m == "mov" and src in this and dst in _ARG_OR_SAVED:
                this[dst] = this[src]
            elif m == "lea" and (s := re.fullmatch(r"\[(\w+)(?: \+ (0x[0-9a-f]+|\d+))?\]", src)) and s.group(1) in this:
                this[dst] = this[s.group(1)] + int(s.group(2) or "0", 0)
            elif m in ("add", "sub") and dst in this and re.fullmatch(r"0x[0-9a-f]+|\d+", src):
                this[dst] += int(src, 0) * (1 if m == "add" else -1)
            elif dst in this and m not in ("cmp", "test", "push") and not o.startswith(("qword ptr", "dword ptr", "byte ptr", "word ptr")):
                this.pop(dst, None)
            if dst == "rcx" and m != "lea":
                desc_in_rcx = False
    return None


_ARG_OR_SAVED = {"rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15"}
