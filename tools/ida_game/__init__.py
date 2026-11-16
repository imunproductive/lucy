"""Headless access to the game IDB research/out/ida_game/ow_game.i64 (built by `python -m tools.ida_game.build`).

    from tools.ida_game import open_game_idb
    with open_game_idb(timeout=1800) as idb:
        print(idb.decompile(0x7FF789BE27C0))      # RVA, ORPHEUS VA (0x7FF788F10000) or IDB VA (0x140000000)
        idb.func(0xCD27C0); idb.xrefs_to(...); idb.callees(...); idb.disasm(..., 20)
        idb.decompile(0x7FF7893EC2A1, define=True)  # no function there yet: define one (in memory)
        idb.define(x)                              # the same without decompiling: start VA or None
        idb.find_imm(0x1C6EC712)                   # VAs of instructions with this immediate
        idb.read(x, 16); idb.qword(x)              # bytes of the database (vtables, globals)

Batch from the shell: `python -m tools.ida_game.decompile <addr> [<addr> ...] [--define] [--out F]`.

Addresses: every method accepts an RVA (< 0x140000000), a VA at the database base 0x7FF788F10000
(= ORPHEUS / IDA-questions.md addresses) or a VA of the user's IDB at 0x140000000. Results give `rva`
and `va` (database base) for each address.

define(): the code holding x becomes a function: from its .pdata entry, else from the nearest
earlier address something references (vtable slot, call) that decodes straight to x, else from x
(funcs.py). find_imm(value, width=None): 4-byte encoding for values below 2**32, 8-byte imm64
otherwise, width=1/2 for short forms; bytes IDA has not made code are decoded in place.

Locking and processes: idalib holds one database per process and runs on the importing thread.
The database is opened in place under an exclusive OS lock (ow_game.i64.lock), so one process at a time:
a running build or another open_game_idb() makes this wait up to `timeout` seconds, then raise.
`snapshot=True` opens a private copy in a temp directory instead (the lock is held only while copying),
so several processes can read concurrently; changes to a snapshot are discarded.
Nothing is saved unless `save=True` (or idb.save()); in-memory changes such as prototypes Hex-Rays guesses
while decompiling are dropped at close.
"""
import contextlib
import os
import re
import shutil
import tempfile

from . import paths
from . import _idalib

BASE = 0x7FF788F10000
IDB_BASE = 0x140000000


class DecompileError(RuntimeError):
    pass


def _line_ea(cfunc, raw):
    """Address of a pseudocode line's statement (cinsn), else of its assignment or call, else of its
    first item (the lowest address would often be the frame-pointer setup inside `[rbp+X]`). Same rule
    as the research repo's tools/ida_dump_pc.py."""
    import ida_hexrays
    import ida_lines
    import idc
    # a ctree item anchor inside the line: COLOR_ON, COLOR_ADDR, 16 hex digits of the item index
    anchor = re.compile(re.escape(ida_lines.SCOLOR_ON + ida_lines.SCOLOR_ADDR) + "([0-9A-Fa-f]{16})")
    items = []
    for m in anchor.finditer(raw):
        index = int(m.group(1), 16)
        if index < len(cfunc.treeitems):
            item = cfunc.treeitems[index]
            if item.ea != idc.BADADDR:
                items.append(item)
    for want in (lambda i: not i.is_expr(),
                 lambda i: ida_hexrays.cot_asg <= i.op <= ida_hexrays.cot_asgumod
                 or i.op == ida_hexrays.cot_call):
        for item in items:
            if want(item):
                return item.ea
    return items[0].ea if items else None


class GameIdb:
    base = BASE

    def __init__(self, path):
        self.path = path

    # -------------------------------------------------------------- addresses
    def ea(self, x):
        """Database VA of an RVA / VA (database base or 0x140000000)."""
        x = int(x)
        if x >= BASE:
            return x
        if IDB_BASE <= x < IDB_BASE + 0x10000000:
            return x - IDB_BASE + BASE
        return BASE + x

    def _addr(self, ea):
        return {"rva": ea - BASE, "va": ea}

    def name(self, x):
        import ida_name
        return ida_name.get_name(self.ea(x)) or None

    # -------------------------------------------------------------- queries
    def func(self, x):
        """{"start", "end" (RVAs), "start_va", "end_va", "name"} of the function containing x, or None."""
        import ida_funcs
        f = ida_funcs.get_func(self.ea(x))
        if f is None:
            return None
        return {"start": f.start_ea - BASE, "end": f.end_ea - BASE, "start_va": f.start_ea,
                "end_va": f.end_ea, "name": ida_funcs.get_func_name(f.start_ea)}

    def define(self, x):
        """Make the code holding x a function when none holds it (in memory; saved only with
        save=True): its .pdata entry, else the nearest referenced start that decodes to x, else x
        (tools/ida_game/funcs.py). Returns the function's start VA or None."""
        from . import funcs
        return funcs.define_at(self.ea(x))

    def decompile(self, x, define=False):
        """Hex-Rays pseudocode of the function containing x, each line suffixed with `// @<VA>`
        (statement address, as the research repo's tools/ida_dump_pc.py). `define`: when no function
        holds x, define one first (see define()). Raises DecompileError."""
        import ida_funcs
        import ida_hexrays
        import ida_lines
        if not ida_hexrays.init_hexrays_plugin():
            raise DecompileError("Hex-Rays decompiler not available")
        ea = self.ea(x)
        f = ida_funcs.get_func(ea)
        if f is None and define and self.define(ea) is not None:
            f = ida_funcs.get_func(ea)
        if f is None:
            raise DecompileError("%#x: no function%s" % (ea, "" if define else " (define=True makes one)"))
        ida_hexrays.mark_cfunc_dirty(f.start_ea)
        failure = ida_hexrays.hexrays_failure_t()
        try:
            cfunc = ida_hexrays.decompile(f.start_ea, failure)
        except ida_hexrays.DecompilationFailure as e:
            raise DecompileError("%#x: %s" % (f.start_ea, e)) from None
        if cfunc is None:
            raise DecompileError("%#x: %#x: %s" % (f.start_ea, failure.errea, failure.desc()))
        lines = ["// %s @ %#x (rva %#x)" % (ida_funcs.get_func_name(f.start_ea), f.start_ea,
                                            f.start_ea - BASE)]
        for sl in cfunc.get_pseudocode():
            text = ida_lines.tag_remove(sl.line)
            line_ea = _line_ea(cfunc, sl.line) if text.strip() else None
            lines.append("%s // @%x" % (text, line_ea) if line_ea is not None else text)
        return "\n".join(lines) + "\n"

    def xrefs_to(self, x):
        """Every code and data reference to x: [{"from", "from_va", "type", "func", "func_rva"}]."""
        import ida_funcs
        import ida_xref
        import idautils
        out = []
        for xr in idautils.XrefsTo(self.ea(x), 0):
            f = ida_funcs.get_func(xr.frm)
            out.append({"from": xr.frm - BASE, "from_va": xr.frm,
                        "type": idautils.XrefTypeName(xr.type), "code": bool(xr.iscode),
                        "func": ida_funcs.get_func_name(f.start_ea) if f else None,
                        "func_rva": f.start_ea - BASE if f else None})
        return out

    def callees(self, x):
        """Direct calls and tail jumps out of the function containing x:
        [{"rva", "va", "name", "site", "kind": "call"|"jump"}] (one entry per call site)."""
        import ida_funcs
        import ida_name
        import ida_xref
        import idautils
        f = ida_funcs.get_func(self.ea(x))
        if f is None:
            return []
        out = []
        for item in idautils.FuncItems(f.start_ea):
            for xr in idautils.XrefsFrom(item, ida_xref.XREF_FAR):
                if not xr.iscode or xr.type in (ida_xref.fl_F,):
                    continue
                if xr.type in (ida_xref.fl_CF, ida_xref.fl_CN):
                    kind = "call"
                else:
                    g = ida_funcs.get_func(xr.to)
                    if g is not None and g.start_ea == f.start_ea:
                        continue
                    kind = "jump"
                out.append({"rva": xr.to - BASE, "va": xr.to, "name": ida_name.get_name(xr.to) or None,
                            "site": item - BASE, "kind": kind})
        return out

    def disasm(self, x, n=20):
        """n instructions/items from x, one line each: `<VA>  <disassembly>`."""
        import ida_bytes
        import ida_lines
        import idc
        ea = self.ea(x)
        lines = []
        for _ in range(n):
            text = ida_lines.tag_remove(idc.generate_disasm_line(ea, 0) or "")
            lines.append("%x  %s" % (ea, text))
            nxt = ida_bytes.next_head(ea, idc.BADADDR)
            if nxt == idc.BADADDR:
                break
            ea = nxt
        return "\n".join(lines)

    def read(self, x, n):
        """n bytes of the database at x (the Rose-patched dump image)."""
        import ida_bytes
        return ida_bytes.get_bytes(self.ea(x), n) or b""

    def qword(self, x):
        import ida_bytes
        return ida_bytes.get_qword(self.ea(x))

    def _code_segments(self):
        """[(start VA, bytes)] of the executable segments, read once."""
        if getattr(self, "_code", None) is None:
            import ida_bytes
            import ida_segment
            self._code = []
            for i in range(ida_segment.get_segm_qty()):
                seg = ida_segment.getnseg(i)
                if seg.perm & ida_segment.SEGPERM_EXEC:
                    self._code.append((seg.start_ea,
                                       ida_bytes.get_bytes(seg.start_ea, seg.end_ea - seg.start_ea) or b""))
        return self._code

    def find_imm(self, value, width=None):
        """VAs of the instructions in code that have `value` as an immediate operand, sorted.
        width (bytes) None: a value below 2**32 (or a sign-extended negative one) as its 4-byte
        encoding, else the 8-byte `mov r64, imm64`; imm8 forms (`cmp eax, 19h`) need width=1
        (slow: every matching byte is decoded). Code IDA has not defined (Rose-hidden, vtable
        stubs) is decoded in place: such a hit has no function until define()."""
        import ida_bytes
        import ida_funcs
        import ida_ua
        v = int(value) & 0xFFFFFFFFFFFFFFFF
        if width is not None:
            pats = [(width, v & ((1 << 8 * width) - 1))]
        elif v <= 0xFFFFFFFF:
            pats = [(4, v)]
        elif v >= 0xFFFFFFFF80000000:
            pats = [(4, v & 0xFFFFFFFF), (8, v)]
        else:
            pats = [(8, v)]
        out = set()
        insn = ida_ua.insn_t()

        def matches(start, p, w, want):
            if ida_ua.decode_insn(insn, start) <= 0 or start + insn.size <= p:
                return False
            for op in insn.ops:
                if op.type == ida_ua.o_void:
                    break
                if (op.type == ida_ua.o_imm and start + op.offb == p
                        and op.value & ((1 << 8 * w) - 1) == want):
                    return True
            return False

        for seg_start, data in self._code_segments():
            for w, want in pats:
                needle = want.to_bytes(w, "little")
                i = data.find(needle)
                while i >= 0:
                    p = seg_start + i
                    head = ida_bytes.get_item_head(p)
                    code = ida_bytes.is_code(ida_bytes.get_flags(head))
                    if code and matches(head, p, w, want):
                        out.add(head)
                    elif not code or ida_funcs.get_func(p) is None:
                        # undefined bytes, or a decode IDA left outside any function (filler
                        # decoded across hidden code): decode the candidate starts in place
                        for start in range(p - 1, p - 15, -1):
                            if matches(start, p, w, want):
                                out.add(start)
                                break
                    i = data.find(needle, i + 1)
        return sorted(out)

    def save(self):
        import ida_loader
        return bool(ida_loader.save_database(self.path, 0))


@contextlib.contextmanager
def open_game_idb(save=False, timeout=0.0, snapshot=False, path=paths.IDB):
    """Open the game IDB (see the module docstring for locking). `save`: save at close."""
    if not os.path.exists(path):
        raise FileNotFoundError("%s missing: run python -m tools.ida_game.build" % path)
    _idalib.ensure()
    tmp = None
    lock = _idalib.DbLock(timeout=timeout)
    if snapshot:
        with lock:
            tmp = tempfile.mkdtemp(prefix="ow_game_")
            target = os.path.join(tmp, os.path.basename(path))
            shutil.copyfile(path, target)
        lock = contextlib.nullcontext()
        save = False
    else:
        target = path
    try:
        with lock:
            _idalib.open_db(target)
            ok = False
            try:
                yield GameIdb(target)
                ok = True
            finally:
                _idalib.close_db(save=bool(save and ok))
    finally:
        if tmp:
            shutil.rmtree(tmp, ignore_errors=True)
