"""Constants in the client's code: 16/32/64-bit immediates and 4/8/16-byte rip-relative loads from
read-only data, by value.

`build()` decodes every .pdata function range (primary functions and their chained fragments) of the
Rose-patched game image the IDB loads (research/out/ida_game/ow_game.exe: the in-game dump with its
re-encrypted code restored and Rose's deobfuscation; the dump itself when that file is missing) with a
linear sweep, and stores per instruction the encoded immediate (the field's bytes as they are, so a
sign-extended `mov r8, 0FFFFFFFFE1C1E402h` is the 32-bit value 0xE1C1E402) and the values of
.rdata loads, plus every direct call/jmp (for callers). Values below 0x100 are not stored.

`search()` looks a value up there and also scans the code outside .pdata functions (Rose-hidden code
such as the `mov eax, 1C6EC712h; ret` stub at 0x7FF7893EC2A0) for the value's bytes, keeping the
places an instruction decodes with its immediate exactly there.
"""

import bisect
import struct

from iced_x86 import (Code, Decoder, FlowControl, Formatter, FormatterSyntax, Instruction, InstructionInfoFactory,
                      MemorySizeExt, Mnemonic, OpAccess)

from tools import paths

from . import connect
from . import guid as G

ROSE_IMAGE = paths.IDA_GAME / "ow_game.exe"  # restored + Rose-patched dump (tools/ida_game/image.py)
MIN_VALUE = 0x100
DATA_SECTIONS = (".rdata", ".rodata", "_RDATA")
_M64 = (1 << 64) - 1
_BRANCHES = (FlowControl.CALL, FlowControl.UNCONDITIONAL_BRANCH, FlowControl.CONDITIONAL_BRANCH)


def code_image_path():
    """The image the sweep decodes: the Rose-patched copy when the game IDB pipeline made one."""
    return ROSE_IMAGE if ROSE_IMAGE.exists() else paths.DUMP


def to_sql(v):
    return v - (1 << 64) if v >= 1 << 63 else v


def from_sql(v):
    return v & _M64


def pdata(img):
    """[(begin, end, root)] of every RUNTIME_FUNCTION, sorted; root: the primary function a chained
    fragment (UNW_FLAG_CHAININFO) belongs to, else begin."""
    lo, hi = img.pdata
    entries, parent = [], {}
    for i in range(lo, hi, 12):
        b, e, u = struct.unpack_from("<III", img.data, i)
        if not b:
            continue
        entries.append((b, e))
        if (img.data[u] >> 3) & 4:
            parent[b] = struct.unpack_from("<I", img.data, u + 4 + 2 * ((img.data[u + 2] + 1) & ~1))[0]
    out = []
    for b, e in sorted(entries):
        r, seen = b, set()
        while r in parent and r not in seen:
            seen.add(r)
            r = parent[r]
        out.append((b, e, r))
    return out


def sweep(img, funcs, watch=()):
    """(consts [(rva, at, value, width, kind, root)], calls [(target, site, root)], refs [(target, site,
    root, access)]); RVAs. kind 'imm' (at = the immediate's bytes) or 'data' (at = the loaded .rdata
    bytes). refs: rip-relative operands into the sorted, disjoint (lo, hi) ranges of watch; access
    'read', 'write', 'read/write' or 'address' (lea)."""
    data, base = img.data, img.base
    ro = [img.sections[n] for n in DATA_SECTIONS if n in img.sections]
    watch_lo = [lo for lo, _ in watch]
    consts, calls, refs = [], [], []
    info = InstructionInfoFactory()
    dec = Decoder(64, data, ip=base)
    ins = Instruction()
    lea = Mnemonic.LEA
    call, jump, jcc = FlowControl.CALL, FlowControl.UNCONDITIONAL_BRANCH, FlowControl.CONDITIONAL_BRANCH
    size_of = MemorySizeExt.size
    for b, e, root in funcs:
        dec.position, dec.ip = b, base + b
        while dec.position < e:
            dec.decode_out(ins)
            rva = ins.ip - base
            fc = ins.flow_control
            if fc == call or fc == jump or fc == jcc:  # rel8/rel32 targets look like immediates to iced
                t = ins.near_branch_target - base
                if fc != jcc and 0 < t < img.size and (fc == call or not b <= t < e):
                    calls.append((t, rva, root))
                continue
            co = dec.get_constant_offsets(ins)
            if co.has_immediate and co.immediate_size >= 2:
                at = rva + co.immediate_offset
                v = int.from_bytes(data[at:at + co.immediate_size], "little")
                if v >= MIN_VALUE:
                    consts.append((rva, at, v, co.immediate_size, "imm", root))
            if ins.is_ip_rel_memory_operand:
                t = ins.ip_rel_memory_address - base
                k = bisect.bisect_right(watch_lo, t) - 1
                if k >= 0 and t < watch[k][1]:
                    refs.append((t, rva, root, _access(ins, info)))
                n = size_of(ins.memory_size)
                if ins.mnemonic != lea and n in (4, 8, 16) and any(lo <= t < hi for lo, hi in ro):
                    for k in range(0, n, 8 if n == 16 else n):
                        w = 8 if n == 16 else n
                        v = int.from_bytes(data[t + k:t + k + w], "little")
                        if v >= MIN_VALUE:
                            consts.append((rva, t + k, v, w, "data", root))
    return consts, calls, refs


def _access(ins, info):
    if ins.mnemonic == Mnemonic.LEA:
        return "address"
    for m in info.info(ins).used_memory():
        a = m.access
        if a in (OpAccess.READ, OpAccess.COND_READ):
            return "read"
        if a in (OpAccess.WRITE, OpAccess.COND_WRITE):
            return "write"
        if a in (OpAccess.READ_WRITE, OpAccess.READ_COND_WRITE):
            return "read/write"
    return "other"


def build(db, image_path=None, log=print, watch=()):
    """Sweep the code image into the const, call, func and ref tables of the open sqlite db (ref: code
    referencing the watch ranges, see sweep()); returns meta."""
    from tools.owindex.image import Image

    image_path = image_path or code_image_path()
    img = Image(image_path)
    funcs = pdata(img)
    consts, calls, refs = sweep(img, funcs, sorted(watch))
    log(f"consts: {len(funcs)} .pdata ranges of {image_path.name}: {len(consts)} constants, {len(calls)} branches,"
        f" {len(refs)} references into {len(watch)} watched objects")  # fmt: skip
    db.executescript("""
        CREATE TABLE const (rva INTEGER, at INTEGER, value INTEGER, width INTEGER, kind TEXT, func INTEGER,
                            gidx INTEGER);
        CREATE TABLE call (target INTEGER, site INTEGER, func INTEGER);
        CREATE TABLE ref (target INTEGER, site INTEGER, func INTEGER, access TEXT);
        CREATE TABLE func (begin INTEGER PRIMARY KEY, end INTEGER, root INTEGER);
    """)
    db.executemany(
        "INSERT INTO const VALUES (?, ?, ?, ?, ?, ?, ?)",
        ((rva, at, to_sql(v), w, k, root, v & G.INDEX_MASK if w == 8 and G.is_guid(v) else None)
         for rva, at, v, w, k, root in consts),
    )
    db.executemany("INSERT INTO call VALUES (?, ?, ?)", calls)
    db.executemany("INSERT INTO ref VALUES (?, ?, ?, ?)", refs)
    db.executemany("INSERT INTO func VALUES (?, ?, ?)", funcs)
    db.executescript("""
        CREATE INDEX const_value ON const(value); CREATE INDEX const_gidx ON const(gidx) WHERE gidx IS NOT NULL;
        CREATE INDEX call_target ON call(target); CREATE INDEX ref_target ON ref(target);
    """)
    return {"code_image": str(image_path), "functions": len(funcs), "constants": len(consts), "branches": len(calls),
            "watched_references": len(refs)}  # fmt: skip


# ------------------------------------------------------------------ queries

def parse(text):
    """A query -> {'kind': 'value', 'value'} or {'kind': 'guid', 'index', 'type' (or None), 'value' (the
    GUID, or None when the type is open)}.
    '0x...' or 7+ hex digits: a value (a GUID-shaped 64-bit value is searched as that GUID);
    '068A.025', '000000000307.025', '097:0001': a GUID; 1-6 hex digits without 0x ('04AA'): a GUID index
    of any type (Lucy's short ids)."""
    s = str(text).strip().replace("`", "").replace("_", "")
    sh = G.parse_short(s)
    if sh:
        return {"kind": "guid", "index": sh[0], "type": sh[1], "value": G.make(*sh)}
    if s.lower().startswith("0x"):
        v = int(s[2:], 16)
    elif s and all(c in "0123456789abcdefABCDEF" for c in s):
        if len(s) <= 6:
            return {"kind": "guid", "index": int(s, 16), "type": None, "value": None}
        v = int(s, 16)
    else:
        raise ValueError(f"{text}: not a hex value, a GUID or a short id (068A.025, 04AA)")
    if G.is_guid(v):
        return {"kind": "guid", "index": v & G.INDEX_MASK, "type": G.guid_type(v), "value": v}
    return {"kind": "value", "value": v}


class Consts:
    def __init__(self, path=None):
        self.db = connect(path)
        self._funcs = None

    def rows(self, where, args):
        return [
            {"rva": rva, "at": at, "value": from_sql(v), "width": w, "kind": k, "func": f}
            for rva, at, v, w, k, f in self.db.execute(
                f"SELECT rva, at, value, width, kind, func FROM const WHERE {where} ORDER BY rva, at", args)
        ]  # fmt: skip

    def value(self, v):
        """Rows with this value; a sign-extended 32-bit value also matches its 32-bit field."""
        vals = {v & _M64}
        if v >> 31 in (0x1FFFFFFFF, -1):
            vals.add(v & 0xFFFFFFFF)
        return self.rows(f"value IN ({','.join('?' * len(vals))})", [to_sql(x) for x in vals])

    def guid(self, index, kind=None):
        """Rows holding a GUID with this index (and type)."""
        rows = self.rows("gidx = ?", (index,))
        return [r for r in rows if kind is None or G.guid_type(r["value"]) == kind]

    def callers(self, target):
        """[(site, root)] of direct calls and jumps to target (RVAs)."""
        return list(self.db.execute("SELECT site, func FROM call WHERE target = ? ORDER BY site", (target,)))

    def refs(self, lo, hi):
        """[(target, site, root, access)] of code referencing [lo, hi) (RVAs; only the ranges the build
        watched: console variable objects)."""
        return list(self.db.execute(
            "SELECT target, site, func, access FROM ref WHERE target >= ? AND target < ? ORDER BY site", (lo, hi)))

    def funcs(self):
        if self._funcs is None:
            self._funcs = list(self.db.execute("SELECT begin, end, root FROM func ORDER BY begin"))
            self._begins = [b for b, _, _ in self._funcs]
        return self._funcs

    def func_at(self, rva):
        """(begin, end, root) of the .pdata range holding rva, or None."""
        funcs = self.funcs()
        k = bisect.bisect_right(self._begins, rva) - 1
        return funcs[k] if k >= 0 and rva < funcs[k][1] else None


def raw_matches(img, value, consts):
    """Code outside .pdata functions with `value` as an immediate: [(ins rva, at, width)]. Takes the
    longest instruction whose immediate field is exactly the matched bytes."""
    if value >= 1 << 32:
        pat = struct.pack("<Q", value)
    elif value >= 0x10000:
        pat = struct.pack("<I", value)
    else:
        return []
    lo, hi = img.sections[".text"]
    data, out = img.data, []
    i = data.find(pat, lo, hi)
    while i != -1:
        if consts.func_at(i) is None:
            for s in range(max(lo, i - 10), i):
                dec = Decoder(64, data[s:i + len(pat) + 4], ip=img.base + s)
                ins = dec.decode()
                co = dec.get_constant_offsets(ins)
                if ins.code != Code.INVALID and ins.flow_control not in _BRANCHES and co.has_immediate \
                        and s + co.immediate_offset == i and co.immediate_size == len(pat):
                    out.append((s, i, len(pat)))
                    break
        i = data.find(pat, i + 1, hi)
    return out


_FMT = None


def ins_text(img, rva):
    global _FMT
    if _FMT is None:
        _FMT = Formatter(FormatterSyntax.INTEL)
        _FMT.hex_prefix, _FMT.hex_suffix = "0x", ""
        _FMT.space_after_operand_separator = True
    return _FMT.format(Decoder(64, img.data[rva:rva + 16], ip=img.base + rva).decode())


def data_refs(img, va, limit=4):
    """RVAs in .rdata/.data that hold the qword va (vtable slots, tables)."""
    pat, out = struct.pack("<Q", va), []
    for name in (".rdata", ".data"):
        lo, hi = img.sections[name]
        i = img.data.find(pat, lo, hi)
        while i != -1 and len(out) < limit:
            if i % 8 == 0:
                out.append(i)
            i = img.data.find(pat, i + 1, hi)
    return out


def in_data(img, value, limit=20):
    """[(rva, next)] of the value's bytes in .rdata/.data at its own alignment (tables keyed by a hash or
    GUID; descriptors). next: the qword after an 8-aligned match when it points into .text (the table
    entry's function), else None. Values below 0x10000 are not searched."""
    if value < 0x10000:
        return []
    pat = struct.pack("<Q" if value >= 1 << 32 else "<I", value)
    out = []
    for name in (".rdata", ".data"):
        lo, hi = img.sections[name]
        i = img.data.find(pat, lo, hi)
        while i != -1 and len(out) < limit:
            if i % len(pat) == 0:
                nxt = img.q(img.base + i + 8) - img.base if i % 8 == 0 else None
                out.append((i, nxt if nxt is not None and img.intext(img.base + nxt) else None))
            i = img.data.find(pat, i + 1, hi)
    return out


def search(text, consts=None, img=None, idx=None, limit=100):
    """Code that uses a constant. text: see parse(). Returns {'query', 'guid', 'total', 'hits': [...],
    'data': [...]}; each hit {'rva', 'at', 'value', 'width', 'kind' ('imm'|'data'|'raw'), 'instruction',
    'function' (root RVA or None), 'function_name', 'what', 'callers' [(rva, name)], 'refs' [(rva, what)]}
    (callers and data references only when the function has no name); each data entry: the value in
    .rdata/.data {'rva', 'what', 'next', 'next_name'} (see in_data())."""
    from tools.owindex.image import Image

    consts = consts or Consts()
    img = img or Image(code_image_path())
    if idx is None:
        try:
            from tools.owindex import Index

            idx = Index.open()
        except FileNotFoundError:
            idx = False
    q = parse(text)
    exact = q["value"]
    if q["kind"] == "guid":
        index, kind = q["index"], q["type"]
        rows = consts.guid(index, kind)
        if exact is not None:
            seen = {(r["rva"], r["at"]) for r in rows}
            rows += [r for r in consts.value(exact) if (r["rva"], r["at"]) not in seen]
        info = {"index": index, "type": f"{kind:03X}" if kind else None, "type_name": G.TYPE_NAMES.get(kind),
                "value": f"0x{exact:016X}" if exact is not None else None}  # fmt: skip
    else:
        rows = consts.value(exact)
        info = None
    if exact is not None:
        rows += [{"rva": s, "at": at, "value": exact, "width": w, "kind": "raw", "func": None}
                 for s, at, w in raw_matches(img, exact, consts)]  # fmt: skip
    rows.sort(key=lambda r: r["rva"])
    name = (lambda r: idx.name(r)) if idx else (lambda r: None)
    hits = []
    for r in rows[:limit]:
        root = r["func"]
        h = {**r, "function": root, "instruction": ins_text(img, r["rva"])}
        h.pop("func")
        h["function_name"] = name(root) if root is not None else None
        h["what"] = idx.addr(r["rva"])["what"] if idx else None
        h["callers"], h["refs"] = [], []
        if h["function_name"] is None:
            target = root if root is not None else r["rva"]
            roots = []
            for _, c in consts.callers(target):
                if c != root and c not in roots:
                    roots.append(c)
            h["callers"] = [(c, name(c)) for c in sorted(roots, key=lambda c: name(c) is None)[:6]]
            h["callers_total"] = len(roots)
            h["refs"] = [(ref, idx.addr(ref)["what"] if idx else None) for ref in data_refs(img, img.base + target)]
        if G.is_guid(r["value"]) and r["width"] == 8:
            h["guid"] = G.short(r["value"])
        hits.append(h)
    data = []
    if exact is not None:
        for rva, nxt in in_data(img, exact):
            data.append({"rva": rva, "what": idx.addr(rva)["what"] if idx else None, "next": nxt,
                         "next_name": name(nxt) if nxt is not None else None})  # fmt: skip
    return {"query": str(text), "guid": info, "total": len(rows), "hits": hits, "data": data}
