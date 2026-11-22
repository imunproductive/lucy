"""Read access to a memory-layout image dump (file offset == RVA): PE sections, integers, x64 decoding."""

import contextlib
import re
import struct
from pathlib import Path

import capstone
import pefile


class Image:
    def __init__(self, path):
        self.path = Path(path)
        self.data = self.path.read_bytes()
        pe = pefile.PE(data=self.data, fast_load=True)
        self.base = pe.OPTIONAL_HEADER.ImageBase
        self.size = pe.OPTIONAL_HEADER.SizeOfImage
        self.sections = {
            s.Name.rstrip(b"\0").decode(): (s.VirtualAddress, s.VirtualAddress + s.Misc_VirtualSize)
            for s in pe.sections
        }
        exc = pe.OPTIONAL_HEADER.DATA_DIRECTORY[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXCEPTION"]]
        self.pdata = (exc.VirtualAddress, exc.VirtualAddress + exc.Size)
        pe.close()
        if len(self.data) < self.size:
            raise ValueError(f"{path}: {len(self.data):#x} bytes < SizeOfImage {self.size:#x} (not a memory-layout dump)")
        self.text = tuple(v + self.base for v in self.sections[".text"])
        self.cs = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)

    # --- integers (VAs) ---
    def inimg(self, va, n=8):
        return self.base <= va <= self.base + self.size - n

    def intext(self, va):
        return self.text[0] <= va < self.text[1]

    def q(self, va):
        return struct.unpack_from("<Q", self.data, va - self.base)[0]

    def d(self, va):
        return struct.unpack_from("<I", self.data, va - self.base)[0]

    def qs(self, va):
        """q(va) or 0 when va is outside the image."""
        return self.q(va) if self.inimg(va) else 0

    def rva(self, va):
        return va - self.base

    # --- code ---
    def decode(self, va):
        off = va - self.base
        for ins in self.cs.disasm(self.data[off : off + 16], va):
            return ins
        return None

    def text_stubs(self):
        """`lea rax, [rip+x]; ret` stubs in .text: {stub VA: x}."""
        lo, hi = self.sections[".text"]
        out = {}
        for m in re.finditer(rb"\x48\x8D\x05(....)\xC3", self.data[lo:hi], re.S):
            ea = self.base + lo + m.start()
            out[ea] = ea + 7 + struct.unpack("<i", m.group(1))[0]
        return out

    def stub_target(self, va):
        """`lea rax, [rip+x]; ret` at va -> x."""
        if not self.inimg(va):
            return None
        b = self.data[va - self.base : va - self.base + 8]
        if b[:3] == b"\x48\x8d\x05" and b[7:8] == b"\xc3":
            return va + 7 + struct.unpack("<i", b[3:7])[0]
        return None


def rip_target(ins):
    return ins.address + ins.size + int(ins.op_str.split("rip + ")[1].split("]")[0], 16)


# --- tracing through this image's opaque predicates (ported from Lucy
# research/ida/extract_statescript_codecs.py, which proved it on the same image) ---

_PAIR = {
    "ja": "jbe", "jbe": "ja", "jb": "jae", "jae": "jb", "je": "jne", "jne": "je", "jo": "jno", "jno": "jo",
    "js": "jns", "jns": "js", "jp": "jnp", "jnp": "jp", "jl": "jge", "jge": "jl", "jg": "jle", "jle": "jg",
}  # fmt: skip
_FLAGS = {
    "jo": ("OF", 1), "jno": ("OF", 0), "jb": ("CF", 1), "jae": ("CF", 0),
    "je": ("ZF", 1), "jne": ("ZF", 0), "js": ("SF", 1), "jns": ("SF", 0),
}  # fmt: skip
_KEEP_FLAGS = {"lea", "nop", "push", "pop", "xchg", "bswap", "not"}
_SHIFTS = {"shl", "shr", "sal", "sar", "rol", "ror", "rcl", "rcr"}
_VOLATILE = {"rax", "rcx", "rdx", "r8", "r9", "r10", "r11"}


def trace(img, start, maxblocks=80, maxins=900):
    """The real instructions from `start` (blocks in visiting order), following jumps through opaque predicates
    (a jcc right after test/and/or/xor or clc/stc decides on known flags; jcc X + opposite jcc X is a jmp).
    Calls are not followed."""
    todo, seen, out, count = [start], set(), [], 0
    while todo and len(out) < maxblocks and count < maxins:
        ea = todo.pop(0)
        block, flags = [], {}
        while count < maxins and ea not in seen:
            seen.add(ea)
            ins = img.decode(ea)
            if ins is None:
                break
            count += 1
            block.append(ins)
            m, nxt = ins.mnemonic, ea + ins.size
            if m in ("ret", "int3", "ud2", "hlt"):
                break
            if m == "jmp":
                try:
                    ea, flags = int(ins.op_str, 16), {}
                    continue
                except ValueError:
                    break
            if m in _PAIR:
                target = int(ins.op_str, 16)
                after = img.decode(nxt)
                if after is not None and after.mnemonic == _PAIR[m] and after.op_str == ins.op_str:
                    ea, flags = target, {}
                    continue
                if m in _FLAGS and _FLAGS[m][0] in flags:
                    ea = target if flags[_FLAGS[m][0]] == _FLAGS[m][1] else nxt
                    continue
                todo.append(target)
                ea, flags = nxt, {}
                continue
            if m.startswith("j"):
                with contextlib.suppress(ValueError):
                    todo.append(int(ins.op_str, 16))
                ea, flags = nxt, {}
                continue
            if m == "clc":
                flags = {"CF": 0}
            elif m == "stc":
                flags = {"CF": 1}
            elif m in ("test", "and", "or", "xor"):
                flags = {"CF": 0, "OF": 0}
            elif m in _SHIFTS and ins.op_str.endswith(", 0"):
                pass
            elif not (m in _KEEP_FLAGS or m.startswith(("mov", "cmov", "set"))):
                flags = {}
            ea = nxt
        out.append(block)
    return out


def this_events(img, fn):
    """A constructor's stores and calls on `this` (rcx at entry, followed through moves): ("vt", 0, value) for
    a `mov [this], reg` of a rip address, ("sub", off, callee) for `lea rcx, [this+off]; call callee`,
    ("base", 0, callee) for a call with rcx = this."""
    alias, regs, events, sub = {"rcx"}, {}, [], None
    for block in trace(img, fn):
        for ins in block:
            m, ops = ins.mnemonic, [o.strip() for o in ins.op_str.split(",")] if ins.op_str else []
            if m == "call":
                try:
                    target = int(ops[0], 16)
                except ValueError:
                    target = None
                if target and sub is not None:
                    events.append(("sub", sub, target))
                elif target and "rcx" in alias:
                    events.append(("base", 0, target))
                alias -= _VOLATILE
                for reg in _VOLATILE:
                    regs.pop(reg, None)
                sub = None
                continue
            if m == "lea" and len(ops) == 2:
                if ops[1].startswith("[rip + "):
                    regs[ops[0]] = rip_target(ins)
                    alias.discard(ops[0])
                else:
                    found = re.match(r"\[(\w+) \+ (0x[0-9a-f]+)\]$", ops[1])
                    if found and found.group(1) in alias and ops[0] == "rcx":
                        sub = int(found.group(2), 16)
                    elif ops[0] == "rcx":
                        sub = None
                    alias.discard(ops[0])
                    regs.pop(ops[0], None)
                continue
            if m == "mov" and len(ops) == 2:
                dst, src = ops
                if dst.startswith("qword ptr [") and dst.endswith("]"):
                    if dst[11:-1] in alias and src in regs:
                        events.append(("vt", 0, regs[src]))
                    continue
                if not dst.startswith(("qword", "dword", "word", "byte", "xmm")):
                    if src in alias:
                        alias.add(dst)
                    else:
                        alias.discard(dst)
                    if src in regs:
                        regs[dst] = regs[src]
                    else:
                        regs.pop(dst, None)
                    if dst == "rcx":
                        sub = None
                    continue
            if ops and m not in ("cmp", "test", "push") and not ops[0].startswith(("qword", "dword", "word", "byte")):
                alias.discard(ops[0])
                regs.pop(ops[0], None)
                if ops[0] in ("rcx", "ecx"):
                    sub = None
    return events


def stored_vtable(img, fn, maxins=24):
    """The rip address a short factory/construct stores to [rax]/[rcx] (`lea reg, [rip+X]; mov [rax|rcx], reg`)."""
    regs, n = {}, 0
    off = fn - img.base
    for ins in img.cs.disasm(img.data[off : off + 256], fn):
        n += 1
        if n > maxins or ins.mnemonic == "ret":
            break
        ops = [o.strip() for o in ins.op_str.split(",")]
        if ins.mnemonic == "lea" and "rip +" in ins.op_str:
            regs[ops[0]] = rip_target(ins)
        elif ins.mnemonic == "mov" and ops[0] in ("qword ptr [rax]", "qword ptr [rcx]") and ops[1] in regs:
            return regs[ops[1]]
    return None
