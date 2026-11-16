"""PE files on disk: sections, .pdata function table, unwind info, exports, and an x64 unwinder.

Unwind data always comes from the file: the protector scrambles the game's in-memory PE header,
and the decrypted debug build's .pdata/.xdata on disk match the running code.
"""
import bisect
import mmap
import os
import struct
from array import array

UNW_FLAG_CHAININFO = 4
UWOP_PUSH_NONVOL, UWOP_ALLOC_LARGE, UWOP_ALLOC_SMALL, UWOP_SET_FPREG = 0, 1, 2, 3
UWOP_SAVE_NONVOL, UWOP_SAVE_NONVOL_FAR, UWOP_EPILOG, UWOP_SPARE = 4, 5, 6, 7
UWOP_SAVE_XMM128, UWOP_SAVE_XMM128_FAR, UWOP_PUSH_MACHFRAME = 8, 9, 10
# Unwind register numbers -> names (same order as the CONTEXT integer registers).
UNWIND_REGS = ("rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
               "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15")


class PEFile:
    """Read-only view of a PE32+ file (memory-mapped)."""

    def __init__(self, path):
        self.path = os.path.abspath(path)
        self.name = os.path.basename(path)
        with open(self.path, "rb") as f:
            self._map = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        m = self._map
        pe = struct.unpack_from("<I", m, 0x3C)[0]
        if m[pe:pe + 4] != b"PE\0\0":
            raise ValueError(f"{path}: not a PE file")
        nsec, = struct.unpack_from("<H", m, pe + 6)
        opt_size, = struct.unpack_from("<H", m, pe + 20)
        opt = pe + 24
        if struct.unpack_from("<H", m, opt)[0] != 0x20B:
            raise ValueError(f"{path}: not PE32+")
        self.entry_rva, = struct.unpack_from("<I", m, opt + 16)
        self.image_base, = struct.unpack_from("<Q", m, opt + 24)
        self.size_of_image, = struct.unpack_from("<I", m, opt + 56)
        ndirs, = struct.unpack_from("<I", m, opt + 108)
        self.dirs = [struct.unpack_from("<II", m, opt + 112 + 8 * i) for i in range(min(ndirs, 16))]
        self.sections = []
        sec = opt + opt_size
        for i in range(nsec):
            name = m[sec:sec + 8].rstrip(b"\0").decode("latin-1")
            vsize, va, rsize, raw = struct.unpack_from("<IIII", m, sec + 8)
            chars, = struct.unpack_from("<I", m, sec + 36)
            self.sections.append((name, va, vsize, raw, rsize, chars))
            sec += 40
        self._pdata = None
        self._exports = None

    def section(self, name):
        for s in self.sections:
            if s[0] == name:
                return s
        return None

    def code_ranges(self):
        """(start_rva, end_rva) of executable sections."""
        return [(va, va + vsize) for name, va, vsize, raw, rsize, chars in self.sections if chars & 0x20000000]

    def rva_to_offset(self, rva):
        for name, va, vsize, raw, rsize, chars in self.sections:
            if va <= rva < va + max(vsize, rsize):
                if rva - va < rsize:
                    return raw + rva - va
                return None
        if rva < self.sections[0][1] if self.sections else rva < 0x1000:
            return rva
        return None

    def read(self, rva, n):
        """Bytes at an RVA from the file (zero-filled past the raw data)."""
        off = self.rva_to_offset(rva)
        if off is None:
            return bytes(n)
        data = self._map[off:off + n]
        return data + bytes(n - len(data))

    # ---- .pdata
    def pdata(self):
        """(begins, ends, unwinds) arrays sorted by begin."""
        if self._pdata is None:
            rva, size = self.dirs[3] if len(self.dirs) > 3 else (0, 0)
            arr = array("I")
            if rva and size:
                arr.frombytes(self.read(rva, size - size % 12))
            begins, ends, unwinds = arr[0::3], arr[1::3], arr[2::3]
            if any(begins[i] > begins[i + 1] for i in range(len(begins) - 1)):
                order = sorted(range(len(begins)), key=begins.__getitem__)
                begins = array("I", (begins[i] for i in order))
                ends = array("I", (ends[i] for i in order))
                unwinds = array("I", (unwinds[i] for i in order))
            self._pdata = (begins, ends, unwinds)
        return self._pdata

    def runtime_function(self, rva):
        """(begin, end, unwind_rva) of the .pdata entry containing rva, or None."""
        begins, ends, unwinds = self.pdata()
        i = bisect.bisect_right(begins, rva) - 1
        if i >= 0 and rva < ends[i]:
            return begins[i], ends[i], unwinds[i]
        return None

    def is_chained(self, unwind_rva):
        return bool((self.read(unwind_rva, 1)[0] >> 3) & UNW_FLAG_CHAININFO)

    def _chain_parent(self, unwind_rva):
        hdr = self.read(unwind_rva, 4)
        count = hdr[2]
        return struct.unpack("<III", self.read(unwind_rva + 4 + ((count + 1) & ~1) * 2, 12))

    def function_start(self, rva):
        """Start RVA of the function containing rva (chained fragments resolved to the primary
        entry), or None when rva is not covered by .pdata."""
        rf = self.runtime_function(rva)
        if rf is None:
            return None
        begin, end, unwind = rf
        for _ in range(32):
            if not self.is_chained(unwind):
                return begin
            begin, end, unwind = self._chain_parent(unwind)
        return begin

    def function_starts(self):
        """Begin RVAs of all primary (non-chained) .pdata entries."""
        begins, ends, unwinds = self.pdata()
        out = []
        for b, u in zip(begins, unwinds):
            if not (self.read(u, 1)[0] >> 3) & UNW_FLAG_CHAININFO:
                out.append(b)
        return out

    # ---- exports
    def exports(self):
        """Sorted list of (rva, name) of exported code (forwarders skipped)."""
        if self._exports is None:
            out = []
            rva, size = self.dirs[0] if self.dirs else (0, 0)
            if rva and size:
                hdr = self.read(rva, 40)
                nfuncs, nnames, afuncs, anames, aords = struct.unpack_from("<IIIII", hdr, 20)
                funcs = struct.unpack(f"<{nfuncs}I", self.read(afuncs, 4 * nfuncs))
                names = struct.unpack(f"<{nnames}I", self.read(anames, 4 * nnames))
                ords = struct.unpack(f"<{nnames}H", self.read(aords, 2 * nnames))
                for name_rva, ordinal in zip(names, ords):
                    if ordinal >= nfuncs:
                        continue
                    f = funcs[ordinal]
                    if rva <= f < rva + size or not f:
                        continue
                    raw = self.read(name_rva, 256)
                    out.append((f, raw[:raw.find(b"\0")].decode("latin-1")))
            out.sort()
            self._exports = out
        return self._exports

    def export_rva(self, name):
        for rva, n in self.exports():
            if n == name:
                return rva
        return None

    def export_at(self, rva):
        """Name of the export starting exactly at rva."""
        ex = self.exports()
        i = bisect.bisect_left(ex, (rva, ""))
        if i < len(ex) and ex[i][0] == rva:
            return ex[i][1]
        return None

    def nearest_export(self, rva):
        ex = self.exports()
        i = bisect.bisect_right(ex, (rva, "\uffff")) - 1
        return ex[i] if i >= 0 else None

    # ---- unwinding
    def unwind_frame(self, regs, base, read_u64):
        """Virtually unwind one frame. regs: dict with 'rip', 'rsp' and the integer registers
        (updated in place to the caller's values). base: the module's load address. read_u64:
        reads a qword from the target. Returns False when rip is not covered by this module's
        .pdata (the caller then treats the frame as a leaf)."""
        rva = regs["rip"] - base
        rf = self.runtime_function(rva)
        if rf is None:
            return False
        begin, end, unwind = rf
        mach_frame = False
        first = True
        for _ in range(32):
            hdr = self.read(unwind, 4)
            flags = hdr[0] >> 3
            prolog_size, count, frame = hdr[1], hdr[2], hdr[3]
            frame_reg, frame_off = frame & 15, frame >> 4
            codes = self.read(unwind + 4, count * 2)
            if first and rva - begin < prolog_size:
                prolog_offset = rva - begin
            else:
                prolog_offset = 1 << 30
            first = False
            # Establisher frame: the frame register once SET_FPREG ran, else rsp.
            frame_base = regs["rsp"]
            if frame_reg:
                for i in range(0, len(codes), 2):
                    if codes[i + 1] & 15 == UWOP_SET_FPREG and codes[i] <= prolog_offset:
                        frame_base = regs[UNWIND_REGS[frame_reg]] - 16 * frame_off
                        break
            i = 0
            while i < count:
                off, opinfo = codes[2 * i], codes[2 * i + 1]
                op, info = opinfo & 15, opinfo >> 4
                size = _opcode_slots(op, info)
                if off <= prolog_offset:
                    if op == UWOP_PUSH_NONVOL:
                        regs[UNWIND_REGS[info]] = read_u64(regs["rsp"])
                        regs["rsp"] += 8
                    elif op == UWOP_ALLOC_LARGE:
                        if info == 0:
                            regs["rsp"] += struct.unpack_from("<H", codes, 2 * i + 2)[0] * 8
                        else:
                            regs["rsp"] += struct.unpack_from("<I", codes, 2 * i + 2)[0]
                    elif op == UWOP_ALLOC_SMALL:
                        regs["rsp"] += info * 8 + 8
                    elif op == UWOP_SET_FPREG:
                        regs["rsp"] = regs[UNWIND_REGS[frame_reg]] - 16 * frame_off
                    elif op == UWOP_SAVE_NONVOL:
                        slot = struct.unpack_from("<H", codes, 2 * i + 2)[0] * 8
                        regs[UNWIND_REGS[info]] = read_u64(frame_base + slot)
                    elif op == UWOP_SAVE_NONVOL_FAR:
                        slot = struct.unpack_from("<I", codes, 2 * i + 2)[0]
                        regs[UNWIND_REGS[info]] = read_u64(frame_base + slot)
                    elif op == UWOP_PUSH_MACHFRAME:
                        sp = regs["rsp"] + (8 if info else 0)
                        regs["rip"] = read_u64(sp)
                        regs["rsp"] = read_u64(sp + 24)
                        mach_frame = True
                i += size
            if not flags & UNW_FLAG_CHAININFO:
                break
            begin, end, unwind = struct.unpack("<III", self.read(unwind + 4 + ((count + 1) & ~1) * 2, 12))
        if not mach_frame:
            regs["rip"] = read_u64(regs["rsp"])
            regs["rsp"] += 8
        return True


def _opcode_slots(op, info):
    if op == UWOP_ALLOC_LARGE:
        return 2 if info == 0 else 3
    if op in (UWOP_SAVE_NONVOL, UWOP_SAVE_XMM128, UWOP_EPILOG):
        return 2
    if op in (UWOP_SAVE_NONVOL_FAR, UWOP_SAVE_XMM128_FAR):
        return 3
    return 1


_cache = {}


def open_pe(path):
    """Cached PEFile for a path."""
    key = os.path.normcase(os.path.abspath(path))
    pe = _cache.get(key)
    if pe is None:
        pe = _cache[key] = PEFile(path)
    return pe
