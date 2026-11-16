"""Function code for dossiers: STU-pointer register tracking over capstone, annotated listings,
Hex-Rays pseudocode from the game IDB with field annotations, callees and string literals."""
import contextlib
import json
import os
import re
import time

import capstone
from capstone import x86

from .classes import ARG_REGS, hx
from .image import reg64

VOLATILE = ("rax", "rcx", "rdx", "r8", "r9", "r10", "r11")
_MEM, _REG, _IMM = x86.X86_OP_MEM, x86.X86_OP_REG, x86.X86_OP_IMM


def access_kind(insn, o):
    if insn.mnemonic == "lea":
        return "addr"
    r, w = o.access & capstone.CS_AC_READ, o.access & capstone.CS_AC_WRITE
    return "read/write" if r and w else ("write" if w else "read")


# ===================================================================== register tracking
def track(img, cls, root, entry):
    """Walk the function at `root` in address order with `entry` = {reg or 'stk:<entry offset>':
    (class hash, owdbg expression at entry)} and follow STU pointers through register/stack copies and loads
    of embedded STU fields. Linear (no CFG): a value survives until its register is written or a call
    clobbers it, which matches compiler output that parks arguments in callee-saved registers.
    -> {"fields": [(insn, (hash, expr), disp, path, access)], "calls": [(insn, kind, target, {argi: value})]}"""
    state = dict(entry)
    delta = 0
    fields, calls = [], []
    rn = img.cs.reg_name
    for insn in img.decode(root):
        ops = insn.operands
        m = insn.mnemonic
        for o in ops:
            if o.type == _MEM and o.mem.base and o.mem.base != x86.X86_REG_RIP and not o.mem.index:
                b = reg64(rn(o.mem.base))
                v = state.get(b)
                if v is not None:
                    fields.append((insn, v, o.mem.disp, cls.field_at(v[0], o.mem.disp), access_kind(insn, o)))
        br = img.branch_target(insn)
        if br is not None:
            if br[0] in ("call", "jump", "call_ptr", "icall"):
                # register arguments only; stack arguments of callees are not modelled
                args = {i: state[r] for i, r in enumerate(ARG_REGS) if r in state}
                calls.append((insn, br[0], br[1], args))
            if br[0] in ("call", "icall", "call_ptr"):
                for r in VOLATILE:
                    state.pop(r, None)
            continue
        if m in ("mov", "lea") and len(ops) == 2 and ops[0].type == _REG:
            d = reg64(rn(ops[0].reg))
            src, v = ops[1], None
            if src.type == _REG and ops[0].size == 8:
                v = state.get(reg64(rn(src.reg)))
            elif src.type == _MEM and src.mem.base and not src.mem.index:
                b = reg64(rn(src.mem.base))
                if b == "rsp" and m == "mov":
                    v = state.get("stk:%d" % (src.mem.disp - delta))
                elif b in state:
                    h, expr = state[b]
                    path = cls.field_at(h, src.mem.disp)
                    if path and path[-1][2] == 0:
                        t = path[-1][1]["type"] or {}
                        if t.get("kind") == "stu" and not t.get("array") and t.get("hash"):
                            if m == "mov" and t.get("mode") == "embed" and ops[0].size == 8:
                                v = (int(t["hash"], 16), "[%s+%#x]" % (expr, src.mem.disp))
                            elif m == "lea" and t.get("mode") == "inline":
                                v = (int(t["hash"], 16), "%s+%#x" % (expr, src.mem.disp))
            if v is not None:
                state[d] = v
            else:
                state.pop(d, None)
            continue
        if m == "mov" and len(ops) == 2 and ops[0].type == _MEM and ops[0].mem.base == x86.X86_REG_RSP \
                and not ops[0].mem.index and ops[1].type == _REG:  # fmt: skip
            key = "stk:%d" % (ops[0].mem.disp - delta)
            v = state.get(reg64(rn(ops[1].reg))) if ops[1].size == 8 else None
            if v is not None:
                state[key] = v
            else:
                state.pop(key, None)
            continue
        if m == "push":
            delta += 8
        elif m == "pop":
            delta -= 8
        elif m in ("sub", "add") and len(ops) == 2 and ops[0].type == _REG and rn(ops[0].reg) == "rsp" \
                and ops[1].type == _IMM:  # fmt: skip
            delta += ops[1].imm if m == "sub" else -ops[1].imm
        try:
            _, written = insn.regs_access()
        except capstone.CsError:
            written = []
        for r in written:
            name = reg64(rn(r))
            if name not in ("rsp", "rip", "rflags"):
                state.pop(name, None)
    return {"fields": fields, "calls": calls}


def entry_state(argi, h):
    """Entry state for a function receiving an instance of class h in argument argi."""
    if argi < 4:
        return {ARG_REGS[argi]: (h, ARG_REGS[argi])}
    return {"stk:%d" % (8 + 8 * argi): (h, "[rsp+%#x]" % (8 + 8 * argi))}


def field_text(cls, path):
    """'STU_X.m_A.m_B+4' for a field path."""
    if not path:
        return None
    parts = [cls.field_name(f).split(" ")[0] for _, f, _ in path]
    s = "STU_%s.%s" % (hx(path[0][0]), ".".join(parts))
    lab = path[-1][1]
    if lab.get("name_source") == "owlib":
        s += " (%s)" % lab["name"]
    if path[-1][2]:
        s += "+%#x" % path[-1][2]
    return s


# ===================================================================== listings
def callees(img, root):
    """Direct callees from capstone: [{"site", "rva", "kind"}] in address order, one per target
    (calls through [rip+X] give the pointer's current target)."""
    out, seen = [], set()
    for insn in img.decode(root):
        br = img.branch_target(insn)
        if br is None or br[1] is None:
            continue
        kind, t = br
        if kind.endswith("_ptr"):
            ptr = img.q(t)
            if not img.base <= ptr < img.base + img.idx.image_size:
                continue
            t, kind = ptr - img.base, kind[:-4] + " via [%#x]" % (img.base + br[1])
        if kind.startswith("jump"):
            fr = img.func_ranges(root)
            if any(b <= t < e for b, e in fr):
                continue
        if t not in seen:
            seen.add(t)
            out.append({"site": insn.address, "rva": t, "kind": kind})
    return out


def strings(img, root):
    out, seen = [], set()
    for insn in img.decode(root):
        t = img.rip_target(insn)
        if t is not None and t not in seen:
            s = img.string_at(t)
            if s:
                seen.add(t)
                out.append({"site": insn.address, "rva": t, "text": s})
    return out


_PLAIN = []


def _plain():
    if not _PLAIN:
        _PLAIN.append(capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64))
    return _PLAIN[0]


def insn_text(img, insn, plain=None):
    """Instruction text with VA operands (the detail decode runs at RVAs)."""
    p = next((plain or _plain()).disasm(bytes(insn.bytes), img.base + insn.address, 1), None)
    return "%s %s" % (p.mnemonic, p.op_str) if p else "%s %s" % (insn.mnemonic, insn.op_str)


def disasm_listing(img, cls, root, entry, heur_class=None, limit=400):
    """Capstone listing lines ('VA  text ; notes') of the function, with field annotations from tracking
    (`entry`) and, for accesses through untracked registers, `?` notes for offsets of `heur_class`."""
    tr = track(img, cls, root, entry)
    notes = {}
    for insn, v, disp, path, acc in tr["fields"]:
        txt = field_text(cls, path)
        if txt:
            notes.setdefault(insn.address, []).append("%s %s" % (acc, txt))
    lines = []
    plain = _plain()
    for n, insn in enumerate(img.decode(root)):
        if n >= limit:
            lines.append("  ... (%d more instructions)" % (len(img.decode(root)) - limit))
            break
        va = img.base + insn.address
        text = insn_text(img, insn, plain)
        note = list(notes.get(insn.address, []))
        if not note and heur_class is not None:
            for o in insn.operands:
                if o.type == _MEM and o.mem.base and o.mem.base not in (x86.X86_REG_RIP, x86.X86_REG_RSP) \
                        and not o.mem.index and o.mem.disp > 0:  # fmt: skip
                    path = cls.field_at(heur_class, o.mem.disp)
                    if path and path[-1][2] == 0 and len(path) == 1 and path[0][1]["owner"] == hx(heur_class):
                        note.append("%s? (offset match)" % field_text(cls, path))
        br = img.branch_target(insn)
        if br is not None and br[1] is not None:
            t = br[1]
            if br[0].endswith("_ptr"):
                ptr = img.q(t)
                note.append("[%s] -> %s" % (img.fname(t) if img.idx.name(t) else "%#x" % (img.base + t),
                                            img.fname(ptr - img.base) if img.base <= ptr < img.base + img.idx.image_size
                                            else "%#x" % ptr))
            elif not any(b <= t < e for b, e in img.func_ranges(root)):
                note.append(img.fname(t))
        else:
            t = img.rip_target(insn)
            if t is not None:
                s = img.string_at(t)
                nm = img.idx.name(t)
                if s:
                    note.append(s if s.startswith("L\"") else json.dumps(s))
                elif nm:
                    note.append(nm)
        lines.append(("%x  %-48s%s" % (va, text, ("  ; " + "; ".join(note)) if note else "")).rstrip())
    return lines, tr


# ===================================================================== Hex-Rays text
_DECL_TYPES = {"_QWORD": 8, "__int64": 8, "unsigned __int64": 8, "_DWORD": 4, "int": 4, "unsigned int": 4,
               "float": 4, "double": 8, "_WORD": 2, "__int16": 2, "unsigned __int16": 2, "_BYTE": 1, "char": 1,
               "__int8": 1, "unsigned __int8": 1, "_OWORD": 16}  # fmt: skip
_PTR_DECL = re.compile(r"\b(%s)\s*\*\s*(\w+)\b" % "|".join(sorted(map(re.escape, _DECL_TYPES), key=len, reverse=True)))
_STU_DECL = re.compile(r"\bSTU_([0-9A-F]{8})\s*\*\s*(\w+)\b")
_OFF = re.compile(r"\((?P<var>[A-Za-z_]\w*) \+ (?P<off>0x[0-9A-Fa-f]+|\d+)(?:LL|uLL|i64|u)?\)")
_IDX = re.compile(r"\b(?P<var>[A-Za-z_]\w*)\[(?P<off>0x[0-9A-Fa-f]+|\d+)\]")
_ALIAS = re.compile(r"^\s*(\w+) = (\w+);")
_LOAD = re.compile(r"^\s*(\w+) = \*\(_QWORD \*\)\((\w+) \+ (0x[0-9A-Fa-f]+|\d+)\);")


def pseudocode_params(text):
    """Parameter names of the decompiled function, in order, and STU-typed parameters {name: hash}."""
    for line in text.splitlines()[1:6]:  # line 0 is decompile()'s `// name @ ...` header
        m = re.search(r"\b\w+\((.*)\)\s*(?://.*)?$", line)
        if m:
            names, typed = [], {}
            for p in m.group(1).split(","):
                p = p.strip()
                mm = re.search(r"(\w+)\s*$", p)
                names.append(mm.group(1) if mm else None)
                st = _STU_DECL.search(p)
                if st:
                    typed[st.group(2)] = int(st.group(1), 16)
            return names, typed
    return [], {}


def annotate_pseudocode(cls, text, tracked, heur_class=None):
    """Annotate `(var + off)` / `var[i]` accesses with field names. `tracked` = {var: class hash} for
    variables known to hold an instance (STU arguments); aliases (`v5 = a4;`) and loads of embedded STU
    fields (`v7 = *(_QWORD *)(a4 + 16);`) are followed. Untracked variables get `?` notes for exact field
    offsets of heur_class. -> (annotated text, [(line, var, offset, path, tracked bool)])."""
    tracked = dict(tracked)
    elsize = {m.group(2): _DECL_TYPES[m.group(1)] for m in _PTR_DECL.finditer(text)}
    for m in _STU_DECL.finditer(text):
        tracked.setdefault(m.group(2), int(m.group(1), 16))
    hits, out = [], []
    for ln, line in enumerate(text.splitlines()):
        a = _ALIAS.match(line)
        if a and a.group(2) in tracked:
            tracked[a.group(1)] = tracked[a.group(2)]
        ld = _LOAD.match(line)
        if ld and ld.group(2) in tracked:
            path = cls.field_at(tracked[ld.group(2)], int(ld.group(3), 0) * (elsize.get(ld.group(2), 1)))
            if path and path[-1][2] == 0:
                t = path[-1][1]["type"] or {}
                if t.get("kind") == "stu" and t.get("mode") == "embed" and not t.get("array"):
                    tracked[ld.group(1)] = int(t["hash"], 16)

        def note(m, deref):
            var, off = m.group("var"), int(m.group("off"), 0)
            off *= elsize.get(var, 1 if not deref else 0)
            if deref and var not in elsize:
                return m.group(0)
            if var in tracked:
                path = cls.field_at(tracked[var], off)
                if path:
                    hits.append((ln, var, off, path, True))
                    return "%s/* %s */" % (m.group(0), field_text(cls, path))
            elif heur_class is not None and off > 0:
                path = cls.field_at(heur_class, off)
                if path and len(path) == 1 and path[0][2] == 0 and path[0][1]["owner"] == hx(heur_class):
                    hits.append((ln, var, off, path, False))
                    return "%s/* %s? */" % (m.group(0), field_text(cls, path))
            return m.group(0)

        code, sep, tail = line.partition(" // @")
        code = _OFF.sub(lambda m: note(m, False), code)
        code = _IDX.sub(lambda m: note(m, True), code)
        out.append(code + sep + tail)
    return "\n".join(out), hits


# ===================================================================== game IDB
class Idb:
    """Lazily opened game IDB (tools.ida_game). `status` says why it is unavailable."""

    def __init__(self, enabled=True, timeout=0.0):
        self.enabled, self.timeout = enabled, timeout
        self.idb, self.status, self.open_seconds = None, None, None
        self._stack = contextlib.ExitStack()
        self._tried = False

    def get(self):
        if self._tried:
            return self.idb
        self._tried = True
        if not self.enabled:
            self.status = "disabled (--no-idb)"
            return None
        try:
            from tools.ida_game import open_game_idb, paths
        except Exception as e:  # noqa: BLE001
            self.status = "tools.ida_game not importable: %s" % e
            return None
        if not os.path.exists(paths.IDB):
            self.status = "%s missing (python -m tools.ida_game.build)" % paths.IDB
            return None
        try:
            with open(paths.STATE) as f:
                phases = json.load(f).get("phases", {})
        except (OSError, ValueError):
            phases = {}
        done = [p for p, v in phases.items() if v.get("done")]
        if "create" not in done:
            self.status = "game IDB still building (phases done: %s)" % (", ".join(done) or "none")
            return None
        self.phases = done
        t = time.time()
        try:
            self.idb = self._stack.enter_context(open_game_idb(timeout=self.timeout))
        except Exception as e:  # noqa: BLE001
            self.status = "game IDB not opened: %s" % e
            return None
        self.open_seconds = time.time() - t
        self.status = "game IDB open (%.1f s incl. any wait for the lock; phases %s)" % (
            self.open_seconds, ", ".join(done))
        return self.idb

    def close(self):
        self._stack.close()
