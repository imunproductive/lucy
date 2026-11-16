"""The analysed image: bytes, sections, .pdata functions (from the index), capstone decoding,
string literals and a rip-relative reference scan over .text."""
import mmap
import struct
from pathlib import Path

import capstone
import numpy as np
from capstone import x86

from tools import paths

# Preferred first: the game IDB's input (dump + restored re-encrypted code + Rose deobfuscation), then the
# restored dump, then the raw dump (some functions are re-encrypted in it).
CANDIDATES = [
    paths.IDA_GAME / "ow_game.exe",
    paths.IDA_GAME / "restored.exe",
    paths.DUMP,
]

_REG64 = {}
for _w in ("ax", "bx", "cx", "dx", "si", "di", "bp", "sp"):
    for _n in ("r" + _w, "e" + _w, _w):
        _REG64[_n] = "r" + _w
    _REG64[_w[0] + "l" if _w[1] == "x" else _w + "l"] = "r" + _w
for _i in range(8, 16):
    for _s in ("", "d", "w", "b"):
        _REG64["r%d%s" % (_i, _s)] = "r%d" % _i


def reg64(name):
    """Full 64-bit register of a sub-register name ('ecx' -> 'rcx'); other names unchanged."""
    return _REG64.get(name, name)


class Image:
    def __init__(self, idx, path=None):
        self.idx = idx
        self.base = idx.base
        if path is None:
            path = next((p for p in CANDIDATES if p.exists() and p.stat().st_size == idx.image_size), None)
            if path is None:
                path = next(p for p in CANDIDATES if p.exists())
        self.path = Path(path)
        self._fh = open(self.path, "rb")
        self.mem = mmap.mmap(self._fh.fileno(), 0, access=mmap.ACCESS_READ)
        self.sections = self._sections()
        self.cs = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.cs.detail = True
        self._decoded = {}

    def _sections(self):
        m = self.mem
        pe = struct.unpack_from("<I", m, 0x3C)[0]
        nsec = struct.unpack_from("<H", m, pe + 6)[0]
        opt = struct.unpack_from("<H", m, pe + 20)[0]
        out = []
        for i in range(nsec):
            o = pe + 24 + opt + 40 * i
            name = m[o:o + 8].rstrip(b"\0").decode("latin-1")
            vsize, va = struct.unpack_from("<II", m, o + 8)
            chars = struct.unpack_from("<I", m, o + 36)[0]
            out.append((name, va, va + vsize, chars))
        return out

    def section(self, rva):
        for name, b, e, _ in self.sections:
            if b <= rva < e:
                return name
        return None

    @property
    def text(self):
        return next((b, e) for name, b, e, _ in self.sections if name == ".text")

    def va(self, rva):
        return self.base + rva

    def q(self, rva):
        return struct.unpack_from("<Q", self.mem, rva)[0]

    def d(self, rva):
        return struct.unpack_from("<I", self.mem, rva)[0]

    # ------------------------------------------------------------------ functions
    def func_root(self, rva):
        """Root RVA of the .pdata function containing rva (chained fragments resolved), or None."""
        row = self.idx.db.execute(
            "SELECT begin, end, root FROM func WHERE begin <= ? ORDER BY begin DESC LIMIT 1", (rva,)
        ).fetchone()
        return row[2] if row and row[1] > rva else None

    def func_start(self, rva):
        """Function of a known entry point (call target, slot): its .pdata root, else rva itself (leaf)."""
        root = self.func_root(rva)
        return root if root is not None else rva

    def func_at(self, rva):
        """Start RVA of the function containing an arbitrary address: the .pdata root, or for a leaf function
        without unwind data the closest of: the byte after int3 padding, the end of the previous .pdata
        function, the previous index-named function (adjacent stubs such as construct functions)."""
        root = self.func_root(rva)
        if root is not None:
            return root
        tb, te = self.text
        if not tb <= rva < te:
            return None
        row = self.idx.db.execute("SELECT MAX(end) FROM func WHERE end <= ?", (rva,)).fetchone()
        lo = max(row[0] or tb, rva - 0x1000)
        i = self.mem[lo:rva + 1].rfind(b"\xcc")
        if i >= 0:
            lo = lo + i + 1
        row = self.idx.db.execute(
            "SELECT MAX(rva) FROM symbol WHERE kind = 'func' AND rva <= ? AND rva >= ?", (rva, lo)
        ).fetchone()
        start = row[0] if row and row[0] is not None else lo
        # a named stub before rva that ends before it: the next function starts where it ends
        if start != lo:
            e = self.func_ranges(start)[-1][1]
            if e <= rva and start < e:
                start = e
        return start

    def func_ranges(self, root):
        rows = self.idx.db.execute("SELECT begin, end FROM func WHERE root = ? ORDER BY begin", (root,)).fetchall()
        if rows:
            return rows
        # leaf without .pdata: up to a ret/jmp past every forward branch, an int3, or the next .pdata function
        row = self.idx.db.execute("SELECT MIN(begin) FROM func WHERE begin > ?", (root,)).fetchone()
        end = min(row[0] or root + 0x1000, root + 0x1000)
        pos, reach = root, root
        for insn in self.cs.disasm(self.mem[root:end], root):
            if insn.mnemonic == "int3":
                break
            pos = insn.address + insn.size
            if insn.mnemonic.startswith("j") and insn.operands and insn.operands[0].type == x86.X86_OP_IMM:
                reach = max(reach, insn.operands[0].imm)
            if insn.mnemonic in ("ret", "jmp") and pos > reach:
                break
        return [(root, max(pos, root + 1))]

    def has_pdata(self, root):
        return self.idx.db.execute("SELECT 1 FROM func WHERE root = ? LIMIT 1", (root,)).fetchone() is not None

    def func_size(self, root):
        return sum(e - b for b, e in self.func_ranges(root))

    def insn_at(self, rva):
        """The instruction covering rva, decoded linearly from its function start, or None."""
        root = self.func_at(rva)
        if root is None:
            return None
        for insn in self.decode(root):
            if insn.address <= rva < insn.address + insn.size:
                return insn
        # rva past the decoded ranges (a leaf's padding guess was off): decode up to it directly
        for insn in self.cs.disasm(self.mem[root:rva + 16], root):
            if insn.address <= rva < insn.address + insn.size:
                return insn
            if insn.address > rva:
                break
        return None

    def obfuscated(self, rva):
        row = self.idx.db.execute(
            "SELECT begin, end, confidence FROM obf WHERE begin <= ? ORDER BY begin DESC LIMIT 1", (rva,)
        ).fetchone()
        return row[2] if row and row[1] > rva else None

    def decode(self, root):
        """capstone instructions (detail on) of every fragment of the function, in address order.
        Instruction .address values are RVAs."""
        if root not in self._decoded:
            out = []
            for b, e in self.func_ranges(root):
                out.extend(self.cs.disasm(self.mem[b:e], b))
            self._decoded[root] = out
        return self._decoded[root]

    def fname(self, rva):
        return self.idx.name(rva) or "sub_%X" % self.va(rva)

    # ------------------------------------------------------------------ operands
    @staticmethod
    def rip_target(insn):
        """RVA a rip-relative operand of insn points to, or None (instruction addresses are RVAs)."""
        for o in insn.operands:
            if o.type == x86.X86_OP_MEM and o.mem.base == x86.X86_REG_RIP:
                return insn.address + insn.size + o.mem.disp
        return None

    def branch_target(self, insn):
        """(kind, target RVA) of a direct call/jmp, ('icall', None) for indirect calls through
        registers, ('ptr', slot RVA) for `call [rip+X]`, else None."""
        g = insn.groups
        if capstone.CS_GRP_CALL in g or insn.mnemonic == "jmp":
            o = insn.operands[0] if insn.operands else None
            kind = "call" if capstone.CS_GRP_CALL in g else "jump"
            if o is not None and o.type == x86.X86_OP_IMM:
                return kind, o.imm
            if o is not None and o.type == x86.X86_OP_MEM and o.mem.base == x86.X86_REG_RIP:
                return kind + "_ptr", insn.address + insn.size + o.mem.disp
            return "i" + kind, None
        return None

    def string_at(self, rva, limit=160):
        """ASCII or UTF-16LE string literal at rva (>= 4 printable chars, NUL-terminated), else None."""
        sec = self.section(rva)
        if sec is None or sec == ".text":
            return None
        raw = self.mem[rva:rva + limit * 2]
        end = raw.find(b"\0")
        s = raw[:end] if end >= 0 else b""
        if len(s) >= 4 and all(32 <= c < 127 or c in (9, 10, 13) for c in s) and end < limit:
            return s.decode("ascii")
        try:
            end = 0
            while end + 1 < len(raw) and raw[end:end + 2] != b"\0\0":
                end += 2
            u = raw[:end].decode("utf-16-le")
        except UnicodeDecodeError:
            return None
        if len(u) >= 4 and all(32 <= ord(c) < 127 for c in u):
            return "L" + repr(u)[1:-1].join('""')
        return None

    # ------------------------------------------------------------------ reference scan
    def scan_rip_refs(self, targets):
        """Every instruction in .text whose rip-relative operand points to one of `targets` (RVAs), found by a
        vectorised disp32 scan and confirmed by decoding. -> [(insn RVA, target RVA, insn)]."""
        return self.scan(targets, ())[0]

    def scan(self, ref_targets, call_targets):
        """One pass over .text: (rip-relative references to ref_targets as in scan_rip_refs,
        direct `call`/`jmp rel32` sites to call_targets as [(site RVA, target RVA, kind)])."""
        ref_targets = sorted(set(int(t) for t in ref_targets))
        call_targets = sorted(set(int(t) for t in call_targets))
        tb, te = self.text
        rarr = np.array(ref_targets, dtype=np.int64)
        carr = np.array(call_targets, dtype=np.int64)
        cand, calls = set(), []
        n = te - tb - 8
        for a in range(4):
            cnt = (n - a) // 4
            v = np.frombuffer(self.mem, dtype="<i4", count=cnt, offset=tb + a).astype(np.int64)
            pos = np.arange(cnt, dtype=np.int64) * 4 + (tb + a)
            nxt = pos + 4 + v
            del v
            if len(rarr):
                for extra in (0, 1, 2, 4):  # trailing immediate after the disp32
                    for p in np.nonzero(np.isin(nxt + extra, rarr))[0]:
                        cand.add(int(pos[p]))
            if len(carr):
                for p in np.nonzero(np.isin(nxt, carr))[0]:
                    site = int(pos[p]) - 1
                    op = self.mem[site]
                    if op in (0xE8, 0xE9):
                        calls.append((site, int(nxt[p]), "call" if op == 0xE8 else "jump"))
        calls = [c for c in calls if getattr(self.insn_at(c[0]), "address", None) == c[0]]  # on the linear decode
        out = []
        tset = set(ref_targets)
        for p in sorted(cand):
            insn = self.insn_at(p)
            if insn is not None and self.rip_target(insn) in tset and insn.address + insn.disp_offset == p:
                out.append((insn.address, self.rip_target(insn), insn))
                continue
            # not on the linear decode of its function (junk, data, or a wrong leaf start): best local decode,
            # preferring a REX-prefixed form (`lea rax, [rip+X]` rather than `lea eax, ...`)
            valid = []
            for start in range(p - 2, p - 9, -1):
                insn = next(self.cs.disasm(self.mem[start:start + 16], start, 1), None)
                if insn is not None and self.rip_target(insn) in tset and insn.disp_offset == p - start:
                    valid.append(insn)
            if valid:
                insn = next((i for i in valid if 0x40 <= self.mem[i.address] <= 0x4F), valid[0])
                out.append((insn.address, self.rip_target(insn), insn))
        return out, calls

    def disp_functions(self, offsets, need, target):
        """Prefilter (byte level, no decode) for functions that access `target` and at least `need` distinct
        `offsets` through the same base register: positions whose preceding ModRM (or ModRM+SIB) byte encodes
        a disp8 (offset < 0x80) or disp32 memory operand with that displacement, keyed by .pdata function and
        the base register's low 3 bits (REX.B is not decoded: r8-r15 share keys with rax-rdi; rsp/r12 bases
        are skipped). -> {root RVA: distinct offsets}."""
        tb, te = self.text
        text = np.frombuffer(self.mem, dtype=np.uint8, count=te - tb, offset=tb)
        rows = np.array(self.idx.db.execute("SELECT begin, end, root FROM func ORDER BY begin").fetchall(),
                        dtype=np.int64)
        begins, ends, roots = rows[:, 0], rows[:, 1], rows[:, 2]
        keys = {}
        for off in sorted(set(offsets) | {target}):
            if off < 0x80:
                pos = np.nonzero(text[2:] == off)[0] + 2
                mod = 0x40
            else:
                pos = self._dword_positions(text, off)
                mod = 0x80
            m1, m2 = text[pos - 1], text[pos - 2]
            plain = ((m1 & 0xC0) == mod) & ((m1 & 7) != 4)
            sib = ((m2 & 0xC0) == mod) & ((m2 & 7) == 4) & ((m1 & 7) != 4) & ((m1 >> 3 & 7) == 4)  # no index
            ok = plain | sib
            base = (m1 & 7).astype(np.int64)[ok]
            pos = pos[ok] + tb
            i = np.searchsorted(begins, pos, side="right") - 1
            good = (i >= 0) & (pos < ends[np.maximum(i, 0)])
            keys[off] = np.unique(roots[i[good]] * 8 + base[good])
        allk, cnt = np.unique(np.concatenate(list(keys.values())), return_counts=True)
        sel = allk[cnt >= need]
        sel = sel[np.isin(sel, keys[target])]
        out = {}
        for k, c in zip(sel, cnt[np.isin(allk, sel)]):
            r = int(k) // 8
            out[r] = max(out.get(r, 0), int(c))
        return out

    @staticmethod
    def _dword_positions(text, value):
        b = value.to_bytes(4, "little")
        cand = np.nonzero(text[2:-3] == b[0])[0] + 2
        for k in range(1, 4):
            cand = cand[text[cand + k] == b[k]]
        return cand
