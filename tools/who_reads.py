"""Static list of code that reads (or writes) one field of an STU class or of an entity component (Overwatch 1.74,
build 104319).

    python tools/who_reads.py <class key> <field hash | m_<H> | OWLib name | +offset> [--write] [--depth N]
                              [--json] [--no-idb] [--max-funcs N] [--all-heuristic]
    python tools/who_reads.py --component N <+offset | mirror field hash | m_<H>> [--write] [--size B] [--addr]
                              [--depth N] [--up N] [--json]

--component N: seeds are the functions calling component N's getter (GetComp<N>), the pool functions (callers of
  its type id stub CompId<N>, which fill an out parameter with the list of all N instances) and their callers.
  The component pointer is tracked from the getter's outputs or the pool entries through the pointer array and
  its decryption (jumps followed in trace order; registers restored at each branch target). Functions that return
  the component (or a member of it) are wrappers and their callers are tracked too (--up levels, default 2);
  callees receiving it (or a member address, `rcx = comp+0x1F0`) in an argument register to --depth.
  Hits: tracked `[comp + off]`; "tracked (array element)" when the field is in a mirror array of records (a field
  repeated with one stride) and the access is the same offset in another record or uses an index register;
  "block copy" for 16+ byte moves over the field. A qword read then xor-ed with a 64-bit constant (an encrypted
  pointer field) is not a hit. A mirror field is turned into its component offset (python -m tools.owindex
  comp<N>). Asset reference fields are 16 bytes in memory: an access at +8 (the GUID) counts as the field.

Where it looks:
  slot       the class's own registration slot functions (else the inherited registration's), with the STU
             instance tracked from its argument register (config/condition evaluate, action execute/run: r9);
  container  slot functions of classes holding an instance of the class in a non-array embedded/inline field
             (tracked through the load of that field);
  reference  functions that reference the class's descriptor, node typeinfo, node vtable or runtime vtables
             (rip-relative scan of .text), plus the callers of the tiny ones among them (casts, getters);
  callee     direct callees of all of the above to --depth (default 2); a tracked instance passed in an
             argument register stays tracked in the callee.
Hit kinds, strongest first:
  typed      Hex-Rays shows the member (`->m_<F>`) in the game IDB's pseudocode (needs tools/ida_game types);
  tracked    the access goes through a register that holds the instance (tracked from an argument/field load);
  heuristic  `[reg+offset]` with the field's offset and a compatible size through an untracked register, only
             in the functions above (noisy: any struct with a member at that offset matches).
The dynamic counterpart (hardware watch in a live game) is printed at the end: tools/owdbg/watch.
"""
import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import capstone  # noqa: E402
from capstone import x86  # noqa: E402

from tools.owindex import Index  # noqa: E402
from tools.studossier import code as C  # noqa: E402
from tools.studossier.classes import ARG_REGS, STU_ARG, Classes, hx  # noqa: E402
from tools.studossier.image import Image, reg64  # noqa: E402

KIND_ORDER = {"typed": 0, "tracked": 1, "tracked (base class)": 2, "hash constant": 3, "heuristic": 4, "signature": 5}
KIND_CAVEAT = {  # printed after the heading of the weak kinds
    "heuristic": "  (guesses: only the offset and size match, through a register not known to hold the class)",
    "signature": "  (guesses: the function touches several of the class's offsets through one register; copy "
                 "constructors and look-alike structs match too; check each one)",
}
ASSET_REF_SIZE = 16  # an asset reference field in memory: 16 bytes (next field +0x10 in all 1,691 cases)
SMALL_CLASS = 6  # data-only classes with fewer own fields get no signature scan by default
COMP_HIT_KIND = {"exact": "tracked", "element": "tracked (array element)", "copy": "block copy"}
COMP_HIT_NOTE = {"tracked (array element)": "  [another record of the array, or an unknown index]",
                 "block copy": "  [16+ byte move over the field: copies the component, weak]"}


class Search:
    def __init__(self, idx, img, cls, args, h, field):
        self.idx, self.img, self.cls, self.args, self.h, self.f = idx, img, cls, args, h, field
        self.off, self.size = field["offset"], field["size"] or 1
        t = field["type"] or {}
        if t.get("kind") == "asset_ref" and not t.get("array"):
            self.size = ASSET_REF_SIZE  # code reads the GUID at +8 too
        self.fh = field["hash"]
        self.compatible = set(idx.ancestors(h))  # tracked classes whose instances may be an h
        self.hits, self.funcs, self.seeds, self.notes, self._co = [], {}, [], [], {}
        self.want = {"write", "read/write", "addr"} if args.write else {"read", "read/write", "addr"}

    # ------------------------------------------------------------------ seeds
    def slot_seeds(self, ch, why):
        regs, eff, inherited = self.cls.registrations(ch)
        out = []
        for r in (regs or ([eff] if eff else [])):
            for slot, rva in list(r["slots"].items()) + [("construct", r["construct_rva"])]:
                if not rva or slot == "type_tag":
                    continue
                argi = STU_ARG.get((r["kind"], slot))
                entry = C.entry_state(argi, ch) if argi is not None else {}
                out.append((self.img.func_start(rva), entry, "%s %s of %s%s" % (
                    r["kind"], slot, self.cls.label(ch), " (inherited)" if inherited else "") + why))
        return out

    def collect_seeds(self):
        h, cls, img = self.h, self.cls, self.img
        self.seeds += [(rva, e, w, "slot") for rva, e, w in self.slot_seeds(h, "")]
        # containers of h (and of its ancestors when they are rare)
        for a in [h] + self.idx.ancestors(h)[1:]:
            conts, total = cls.containers(a)
            if a != h and total > 20:
                self.notes.append("containers of base class %s skipped (%d fields)" % (cls.label(a), total))
                continue
            for ch, cf in conts:
                for rva, e, w in self.slot_seeds(ch, " (holds %s in +%#x m_%s)" % (cls.label(a), cf["offset"], cf["hash"])):
                    self.seeds.append((rva, e, w, "container"))
        # references to the class objects
        st = self.idx.stu(hx(h))
        objs = {st["descriptor_rva"]: "descriptor", st["node_typeinfo_rva"]: "node typeinfo",
                st["node_vtable_rva"]: "node vtable"}  # fmt: skip
        for r in st["registrations"]:
            if r["runtime_vtable_rva"]:
                objs[r["runtime_vtable_rva"]] = "runtime vtable (record #%d)" % r["index"]
        objs.pop(None, None)
        t = time.time()
        refs, _ = img.scan(list(objs), ())
        tiny, seen = {}, set()
        for site, target, insn in refs:
            root = img.func_at(site)
            if root is None:
                continue
            if root not in seen:
                seen.add(root)
                self.seeds.append((root, {}, "references the %s at %#x (%s)" % (
                    objs[target], img.va(site), C.insn_text(img, insn)), "reference"))
            if img.func_size(root) < 0x40:
                tiny[root] = objs[target]
        if tiny:
            _, calls = img.scan((), list(tiny))
            for site, target, kind in calls:
                root = img.func_at(site)
                if root is not None and root not in seen:
                    seen.add(root)
                    self.seeds.append((root, {}, "%ss %s (tiny function referencing the %s)" % (
                        kind, img.fname(target), tiny[target]), "reference caller"))
        self.scan_seconds = time.time() - t
        uniq, keys = [], set()
        for s in self.seeds:
            k = (s[0], tuple(sorted((r, v[0]) for r, v in s[1].items())))
            if k not in keys:
                keys.add(k)
                uniq.append(s)
        self.seeds = uniq

    # ------------------------------------------------------------------ walk
    def walk(self):
        work = [(rva, e, w, k, 0) for rva, e, w, k in self.seeds]
        done = set()
        while work and len(self.funcs) < self.args.max_funcs:
            rva, entry, why, kind, depth = work.pop(0)
            key = (rva, tuple(sorted((r, v[0]) for r, v in entry.items())))
            if key in done:
                continue
            done.add(key)
            fn = self.funcs.setdefault(rva, {"rva": rva, "name": self.img.fname(rva), "why": [], "kind": kind})
            fn["why"].append(why)
            tr = C.track(self.img, self.cls, rva, entry)
            tracked_sites = set()
            for insn, v, disp, path, acc in tr["fields"]:
                tracked_sites.add(insn.address)
                hit = any(f["hash"] == self.fh for _, f, _ in path)
                base_only = False
                if not hit and v[0] != self.h and v[0] in self.compatible and self.off <= disp < self.off + self.size:
                    hit, base_only = True, True  # instance tracked as a base class: may be an h
                if not hit and v[0] == self.h and self.off < disp < self.off + self.size:
                    hit = True  # inside the field (the GUID of an asset reference, a member of an inline struct)
                if hit and acc in self.want:
                    self.add("tracked (base class)" if base_only else "tracked", rva, insn, acc,
                             expr="%s+%#x" % (v[1], disp), why=why, depth=depth,
                             note="instance tracked as %s" % self.cls.label(v[0]) if base_only else None)
            if kind != "callee" or self.args.all_heuristic:
                self.heuristic(rva, tracked_sites, why, depth, "heuristic")
            if depth >= self.args.depth:
                continue
            for insn, bkind, target, args in tr["calls"]:
                if target is None or bkind.endswith("_ptr"):
                    continue
                if any(b <= target < e for b, e in self.img.func_ranges(rva)):
                    continue
                sub = self.img.func_start(target)
                ent = {ARG_REGS[i]: (v[0], ARG_REGS[i]) for i, v in args.items()}
                work.append((sub, ent, "called from %s at %#x" % (self.img.fname(rva), self.img.va(insn.address)),
                             "callee", depth + 1))

    def size_ok(self, insn, o, f):
        """Access compatible with the field's type: exact size for scalars (any size inside vectors/structs),
        SSE or 4/8-byte moves for floats."""
        size = f["size"]
        if size is None or size > 8:
            return True
        if o.size != size:
            return False
        name = (f["type"] or {}).get("name")
        if name in ("f32", "f64"):  # floats are used through SSE; plain moves are copies of anything
            return insn.mnemonic.endswith(("ss", "sd")) or "xmm" in insn.op_str
        return not insn.mnemonic.endswith(("ss", "sd")) and "xmm" not in insn.op_str

    def coaccess(self, rva):
        """{base register: score} in a function: own fields of the class accessed through that register with a
        compatible type, minus own-field offsets accessed with a contradicting one (a float load from a u8 field
        says the register holds some other struct)."""
        if rva in self._co:
            return self._co[rva]
        rn = self.img.cs.reg_name
        own = {f["offset"]: f for f in self.cls.fields(self.h) if f["owner"] == hx(self.h)}
        good, bad = {}, {}
        for insn in self.img.decode(rva):
            if insn.mnemonic in ("lea", "call", "jmp"):
                continue
            for o in insn.operands:
                if o.type == x86.X86_OP_MEM and o.mem.base and not o.mem.index:
                    f = own.get(o.mem.disp)
                    if f is not None:
                        (good if self.size_ok(insn, o, f) else bad).setdefault(
                            reg64(rn(o.mem.base)), set()).add(o.mem.disp)
        per = {b: len(s) - len(bad.get(b, set()) - s) for b, s in good.items()}
        self._co[rva] = per
        return per

    def heuristic(self, rva, tracked_sites, why, depth, kind, min_score=0):
        rn = self.img.cs.reg_name
        nown = sum(1 for f in self.cls.fields(self.h) if f["owner"] == hx(self.h))
        for insn in self.img.decode(rva):
            if insn.address in tracked_sites or insn.mnemonic in ("call", "jmp"):
                continue  # calls/jumps through [reg+off] are virtual calls, not field reads
            for o in insn.operands:
                if o.type != x86.X86_OP_MEM or not o.mem.base or o.mem.index:
                    continue
                base = reg64(rn(o.mem.base))
                if base in ("rsp", "rip"):
                    continue
                d = o.mem.disp
                if not self.off <= d < self.off + self.size:
                    continue
                if insn.mnemonic == "lea":
                    if kind == "signature":
                        continue
                elif not self.size_ok(insn, o, self.f):
                    continue
                acc = C.access_kind(insn, o)
                if acc in self.want and (d == self.off or self.size > 8):
                    co = self.coaccess(rva).get(base, 0)
                    if co < min_score:
                        continue
                    self.add(kind, rva, insn, acc, expr="%s+%#x" % (base, d), why=why, depth=depth,
                             arm_here=True, score=co,
                             note="co-access score %d/%d (own fields through %s with matching types, minus "
                                  "contradicting ones)" % (co, nown, base))

    def signature(self):
        """Whole-binary co-access scan: functions accessing the field and many other own fields through the same
        base register (byte-level prefilter, then decoded with size checks); best `--signature` candidates."""
        own = [f["offset"] for f in self.cls.fields(self.h) if f["owner"] == hx(self.h)]
        need = max(3, len(own) // 2)
        t = time.time()
        cand = self.img.disp_functions(own, need, self.off)
        top = sorted(cand.items(), key=lambda kv: -kv[1])[: self.args.signature]
        for rva, _ in top:
            if rva in self.funcs:
                continue
            if self.coaccess(rva) and max(self.coaccess(rva).values()) >= need:
                self.heuristic(rva, set(), "signature scan (>= %d of %d own fields through one register)" % (
                    need, len(own)), None, "signature", min_score=need)
        self.notes.append("signature scan: %d prefiltered functions, %d decoded (%.1f s)" % (
            len(cand), len(top), time.time() - t))

    def hash_constants(self):
        """Instructions with the field hash as an immediate: reflection by field hash (descriptor builders,
        generic STU readers/writers looking a field up by hash). The access kind is unknown."""
        import numpy as np
        tb, te = self.img.text
        text = np.frombuffer(self.img.mem, dtype=np.uint8, count=te - tb, offset=tb)
        value = int(self.fh, 16)
        other = 0
        for p in self.img._dword_positions(text, value):
            insn = self.img.insn_at(int(p) + tb)
            if insn is None or not any(o.type == x86.X86_OP_IMM and o.imm & 0xFFFFFFFF == value
                                       for o in insn.operands):
                continue
            root = self.img.func_at(insn.address)
            descs = set()
            for i in self.img.decode(root):
                t = self.img.rip_target(i)
                if t is not None:
                    descs.update(ch for (ch,) in self.idx.db.execute(
                        "SELECT class_hash FROM role WHERE rva = ? AND role = 'descriptor'", (t,)))
            if descs and self.h not in descs:
                other += 1  # another class's descriptor builder declaring a field with the same hash
                continue
            self.add("hash constant", root, insn, "hash", expr=None, why="field hash 0x%s as an immediate" % self.fh,
                     depth=None, note="the class's descriptor builder (declares the field; not a reader)" if descs
                     else "other code using the hash (e.g. a lookup by field hash; read it)")
        if other:
            self.notes.append("field hash also declared by %d other classes' descriptor builders (skipped)" % other)

    def add(self, kind, rva, insn, acc, expr, why, depth, note=None, arm_here=False, score=None):
        self.hits.append({"kind": kind, "func_rva": rva, "func": self.img.fname(rva), "func_va": self.img.va(rva),
                          "site_va": self.img.va(insn.address), "insn": C.insn_text(self.img, insn), "access": acc,
                          "expr": expr, "via": why, "depth": depth, "note": note, "arm_here": arm_here, "score": score,
                          "size": max((o.size for o in insn.operands if o.type == x86.X86_OP_MEM), default=None)})

    # ------------------------------------------------------------------ IDB
    def typed(self, idb_wrap):
        idb = idb_wrap.get()
        if idb is None:
            return
        import re
        names = {"m_" + self.fh}
        if self.f.get("name_source") == "owlib" and self.f.get("name"):
            names.add(self.f["name"])
        alt = "|".join(re.escape(n) for n in names)
        member = re.compile(r"(?:->|\.)(?:%s)\b" % alt)
        write = re.compile(r"(?:->|\.)(?:%s)\b(?:\.\w+|\[[^\]]*\])*\s*(?:[-+*/|&^]|<<|>>)?=(?!=)" % alt)
        hit_funcs = {x["func_rva"] for x in self.hits if x["kind"].startswith("tracked")}
        core = [r for r, fn in self.funcs.items() if fn["kind"] != "callee" or r in hit_funcs]
        for rva in core[: self.args.max_decompile]:
            try:
                text = idb.decompile(rva)
            except Exception as e:  # noqa: BLE001
                self.funcs[rva]["decompile_error"] = str(e)
                continue
            for line in text.splitlines():
                code, _, at = line.partition(" // @")
                if member.search(code):
                    acc = "write" if write.search(code) else "read"
                    if acc in self.want:
                        self.hits.append({"kind": "typed", "func_rva": rva, "func": self.img.fname(rva),
                                          "func_va": self.img.va(rva), "site_va": int(at, 16) if at else None,
                                          "insn": code.strip(), "access": acc, "expr": None, "score": None,
                                          "via": self.funcs[rva]["why"][0], "depth": None, "note": None,
                                          "arm_here": False})
        self.notes.append("Hex-Rays: %d of %d candidate functions decompiled for typed member accesses" % (
            min(len(core), self.args.max_decompile), len(core)))


# ====================================================================== --component N
class _Decoder:
    """tools.owindex.image.trace over the analysed image (detail decode, RVAs)."""

    def __init__(self, img):
        self.img = img

    def decode(self, ea):
        return next(self.img.cs.disasm(self.img.mem[ea:ea + 16], ea, 1), None)


def comp_track(img, root, entry, src, _cache={}):  # noqa: B006 - per-process decode cache
    """Follow component pointers through one function, in trace order (jumps followed through opaque
    predicates, so the junk after an obfuscated `jmp` is skipped). Values:
      ("A", slot)  address of a stack slot (an out parameter: lea reg, [rsp+X]);
      "I"          derived from the getter's outputs (mask, shift, word, popcount, the bitset words);
      "N"          the component's type id from its id stub; "L" = a pool list header (pool table + 16 * id);
      "S"          pointer into an array of encrypted component pointers (the entity's slot, a pool's entries);
      ("E", k)     a loaded/decrypted component pointer, plus k.
    src: {"getters", "idstubs", "pools", "wrappers" ({rva: k returned})}. A getter call marks its output slots;
    a pool function called with an out parameter fills it with list headers; a wrapper call sets rax.
    -> {"access": [(insn, k + disp, access, size, base reg, disp, indexed)], "calls": [(insn, target, {argi: k})],
        "returns": k of the component pointer returned in rax, or None}"""
    from tools.owindex.image import trace

    if root not in _cache:
        _cache[root] = [(i, k == 0) for b in trace(_Decoder(img), root, maxblocks=3000, maxins=30000)
                        for k, i in enumerate(b)]
    rn = img.cs.reg_name
    st, slots = dict(entry), {}
    access, calls, returns = [], [], None
    consts, pending = set(), {}  # registers holding 64-bit constants; register -> access just loaded into it
    saved = {}  # branch target -> (registers, stack slots) when the branch was seen; restored at its block

    def r64(r):
        return reg64(rn(r))

    def stack_key(m):
        b = r64(m.base) if m.base else None
        return (b, m.disp) if b in ("rsp", "rbp") and not m.index else None

    def is_e(v):
        return isinstance(v, tuple) and v[0] == "E"

    def load(m):
        k = stack_key(m)
        if k is not None:
            v = slots.get(k)
            return "I" if v == "O" else v
        b = st.get(r64(m.base)) if m.base and m.base != x86.X86_REG_RIP else None
        ix = st.get(r64(m.index)) if m.index else None
        if b == "S":
            return ("E", 0)
        if isinstance(b, tuple) and b[0] == "A" and not m.index and m.disp == 0:
            v = slots.get(b[1])  # through the address of a stack slot (an id stub returns its argument)
            return "I" if v == "O" else v
        if (b == "L" and not m.index and m.disp == 0) or "N" in (b, ix):
            return "S"
        if ix == "I" and m.scale == 8 and m.disp == 0:
            return ("E", 0)  # pointer array [base + slot index * 8] (the bitset words are at +0x88)
        if "I" in (b, ix):
            return "I"
        return None

    def merge(vals, m):
        if any(is_e(v) for v in vals):
            return ("E", 0)
        for kind in ("S", "L"):
            if kind in vals:
                return kind
        if "N" in vals:
            return "L" if m == "add" else "N"
        return "I" if "I" in vals else None

    for insn, block_start in _cache[root]:
        if block_start and insn.address in saved:
            st, slots = (dict(x) for x in saved[insn.address])
            consts, pending = set(), {}
        m, ops = insn.mnemonic, insn.operands
        br = img.branch_target(insn)
        if br is not None and br[0] in ("call", "call_ptr", "icall"):
            t = br[1] if br[0] == "call" else None
            rcx = st.get("rcx")
            out = rcx[1] if isinstance(rcx, tuple) and rcx[0] == "A" else None
            if t in src["getters"]:
                for r in ARG_REGS:
                    v = st.get(r)
                    if isinstance(v, tuple) and v[0] == "A":
                        slots[v[1]] = "O"
            elif t in src["idstubs"] and out is not None:
                slots[out] = "N"
            elif t in src["pools"] and out is not None:
                slots[out] = slots[(out[0], out[1] + 8)] = "L"
            elif t is not None:
                args = {i: st[r][1] for i, r in enumerate(ARG_REGS) if is_e(st.get(r))}
                if args:
                    calls.append((insn, t, args))
            for r in C.VOLATILE:
                st.pop(r, None)
            pending.clear()
            if t in src["idstubs"] and out is not None:
                st["rax"] = rcx  # the stub returns its argument
            elif t in src["wrappers"]:
                st["rax"] = ("E", src["wrappers"][t])
            continue
        if br is not None and br[0] in ("jump", "jump_ptr") and br[1] is not None and br[1] in src["wrappers"]:
            returns = src["wrappers"][br[1]]  # tail call of a wrapper
            continue
        if m == "ret":
            if returns is None and is_e(st.get("rax")):
                returns = st["rax"][1]
            continue
        if m.startswith("j"):
            if insn.operands and insn.operands[0].type == x86.X86_OP_IMM:
                saved.setdefault(insn.operands[0].imm, (dict(st), dict(slots)))
            continue
        mem = next((o for o in ops if o.type == x86.X86_OP_MEM), None)
        loaded = None
        if mem is not None and mem.mem.base and mem.mem.base != x86.X86_REG_RIP:
            v = st.get(r64(mem.mem.base))
            if is_e(v):
                access.append((insn, v[1] + mem.mem.disp, C.access_kind(insn, mem), mem.size,
                               r64(mem.mem.base), mem.mem.disp, bool(mem.mem.index)))
                loaded = len(access) - 1
        dst = ops[0] if ops else None
        # stores to the stack: spills of tracked values
        if dst is not None and dst.type == x86.X86_OP_MEM and m == "mov":
            k = stack_key(dst.mem)
            if k is not None:
                v = st.get(r64(ops[1].reg)) if ops[1].type == x86.X86_OP_REG else None
                if v is not None and v != "O":
                    slots[k] = v
                else:
                    slots.pop(k, None)
            continue
        if dst is None or dst.type != x86.X86_OP_REG:
            continue
        d = r64(dst.reg)
        if d in ("rsp", "rip"):
            continue
        src_op = ops[1] if len(ops) > 1 else None
        if d in pending and m != "cmp" and m != "test":
            # a qword read from the component then xor-ed with a 64-bit constant: an encrypted pointer field
            if m == "xor" and src_op is not None and (src_op.type == x86.X86_OP_IMM or (
                    src_op.type == x86.X86_OP_REG and r64(src_op.reg) in consts)):
                access[pending[d]] = None
            del pending[d]
        new = None
        if m == "lea" and src_op is not None:
            mm = src_op.mem
            b = st.get(r64(mm.base)) if mm.base and mm.base != x86.X86_REG_RIP else None
            ix = st.get(r64(mm.index)) if mm.index else None
            if is_e(b):
                new = ("E", b[1] + mm.disp)  # a member, or the first element of an array of records
            elif "I" in (b, ix) or b == "S":
                new = "S"
            elif "N" in (b, ix):
                new = "L"
            elif stack_key(mm) is not None:
                new = ("A", stack_key(mm))
        elif m in ("mov", "movabs", "movzx", "movsxd", "movsx") and src_op is not None:
            if src_op.type == x86.X86_OP_MEM:
                new = load(src_op.mem)
            elif src_op.type == x86.X86_OP_REG:
                new = st.get(r64(src_op.reg))
        elif m in ("xor", "sub") and src_op is not None and src_op.type == x86.X86_OP_REG and src_op.reg == dst.reg:
            new = None
        elif m in ("add", "sub") and src_op is not None and src_op.type == x86.X86_OP_IMM and is_e(st.get(d)):
            new = ("E", st[d][1] + (src_op.imm if m == "add" else -src_op.imm))
        elif m in ("cmp", "test", "bt", "push"):
            continue
        else:
            vals = [st.get(d)] if m not in ("pop",) else []
            for o in ops[1:]:
                if o.type == x86.X86_OP_REG:
                    vals.append(st.get(r64(o.reg)))
                elif o.type == x86.X86_OP_MEM:
                    vals.append(load(o.mem))
            new = merge(vals, m)
        if dst.size < 4:
            new = None  # setcc al, mov al, 1, ...: flags and small values, not pointers
        if new is None:
            st.pop(d, None)
        else:
            st[d] = new
        if m in ("mov", "movabs") and src_op is not None and src_op.type == x86.X86_OP_IMM and \
                src_op.imm & 0xFFFFFFFF00000000:
            consts.add(d)
        else:
            consts.discard(d)
        if loaded is not None and m == "mov" and dst.size == 8:
            pending[d] = loaded
        try:
            written = insn.regs_access()[1]
        except capstone.CsError:
            written = ()
        for r in written:
            name = r64(r)
            if name != d:
                st.pop(name, None)  # implicit writes (mul/div rdx, xchg, cpuid, ...)
                consts.discard(name)
    return {"access": [a for a in access if a is not None], "calls": calls, "returns": returns}


class CompSearch:
    """Readers/writers of component N +off: functions calling N's getter or walking N's pool, and callers of the
    pool functions (a pool function calls N's type id stub and fills an out parameter with the pool's list
    headers); functions among them that return the component (or a member of it) are wrappers, whose callers are
    tracked too (--up levels); callees receiving the pointer in an argument register are tracked to --depth.
    Fixed arrays of records in the mirror (a field repeated with one stride): an access to any element at the same
    offset inside the record counts."""

    def __init__(self, idx, img, args, comp, off, size):
        self.idx, self.img, self.args, self.n, self.off, self.size = idx, img, args, comp["id"], off, size
        acc = idx.component_accessors(self.n)
        uses = idx.component_uses(self.n)
        self.src = {"getters": {a["rva"] for a in acc if a["kind"] == "getter"},
                    "idstubs": {a["rva"] for a in acc if a["kind"] == "id stub"},
                    "pools": {img.func_start(u["func_rva"]) for u in uses if u["kind"] == "id stub call"},
                    "wrappers": {}}  # fmt: skip
        self.run_of = self.record_run(comp["fields"], off)
        self.funcs, self.hits, self.notes, self.done = {}, [], [], set()
        self.want = {"write", "read/write"} if args.write else {"read", "read/write"}
        if args.addr:
            self.want.add("addr")

    @staticmethod
    def record_run(fields, off):
        """(start, stride, count, offset inside the record) when off is inside a run of >= 2 consecutive mirror
        fields with one hash and one stride (a fixed array of records), else None."""
        fs = [f for f in fields if f["offset"] is not None]
        i = 0
        while i < len(fs):
            j = i
            while j + 1 < len(fs) and fs[j + 1]["hash"] == fs[i]["hash"]:
                j += 1
            if j > i:
                stride = fs[i + 1]["offset"] - fs[i]["offset"]
                offs = [f["offset"] for f in fs[i:j + 1]]
                start, n = offs[0], len(offs)
                if stride > 0 and offs == [start + k * stride for k in range(n)] and start <= off < start + n * stride:
                    return start, stride, n, (off - start) % stride
            i = j + 1
        return None

    def match(self, a, size, indexed):
        """'exact', 'element' (same offset in another record of the array) or None."""
        size = max(size, 1)
        if not indexed and a < self.off + self.size and self.off < a + size:
            return "exact"
        if self.run_of:
            start, stride, n, e = self.run_of
            if start <= a < start + n * stride:
                r = (a - start) % stride
                if r < e + self.size and e < r + size:
                    return "exact" if a == self.off and not indexed else "element"
        elif indexed and a < self.off + self.size and self.off < a + size:
            return "element"
        return None

    def run(self):
        """Getter callers, pool functions and their callers first, then (to --up levels) callers of the functions
        among them that return the component, then the callees receiving the component, breadth-first to --depth."""
        seeds = {}
        for u in self.idx.component_uses(self.n):
            if u["kind"] == "getter call":
                seeds.setdefault(self.img.func_start(u["func_rva"]), "calls %s at %#x" % (
                    self.img.fname(u["accessor_rva"]), self.img.va(u["site"])))
        for p in sorted(self.src["pools"]):
            seeds.setdefault(p, "calls %s (component %d's type id)" % (
                self.img.fname(next(iter(self.src["idstubs"]))), self.n))
        if self.src["pools"]:
            _, calls = self.img.scan((), sorted(self.src["pools"]))
            for site, target, kind in calls:
                root = self.img.func_at(site)
                if root is not None:
                    seeds.setdefault(root, "%ss %s (component %d pool) at %#x" % (
                        kind, self.img.fname(target), self.n, self.img.va(site)))
        callees, level = [], seeds
        for up in range(self.args.up + 1):
            new = {}
            for rva, why in level.items():
                k = self.analyse(rva, {}, why, "getter caller" if up == 0 else "wrapper caller", 0, callees)
                if k is not None and rva not in self.src["wrappers"]:
                    new[rva] = k
            self.src["wrappers"].update(new)
            if not new or up == self.args.up:
                break
            _, calls = self.img.scan((), list(new))
            level = {}
            for site, target, kind in calls:
                root = self.img.func_at(site)
                if root is not None:
                    level.setdefault(root, "%ss %s (returns component %d +%#x) at %#x" % (
                        kind, self.img.fname(target), self.n, new[target], self.img.va(site)))
                    self.done.discard((root, ()))  # analysed before the wrapper was known: track it again
        while callees:
            self.analyse(*callees.pop(0), callees)
        self.notes.append("component %d: %d getter(s), %d pool function(s), %d seed functions, %d wrappers "
                          "returning the component" % (self.n, len(self.src["getters"]), len(self.src["pools"]),
                                                       len(seeds), len(self.src["wrappers"])))
        if self.run_of:
            start, stride, n, e = self.run_of
            self.notes.append("+%#x is +%#x in record %d of an array of %d records of %#x bytes at +%#x: accesses to "
                              "+%#x of any record count" % (self.off, e, (self.off - start) // stride, n, stride,
                                                             start, e))
        if len(self.funcs) >= self.args.max_funcs:
            self.notes.append("stopped at --max-funcs %d" % self.args.max_funcs)

    def analyse(self, rva, entry, why, kind, depth, callees):
        """Track one function; queue its callees that receive the component. -> the component offset it returns
        (a wrapper), else None."""
        key = (rva, tuple(sorted(entry.items())))
        if key in self.done or len(self.funcs) >= self.args.max_funcs:
            return None
        self.done.add(key)
        fn = self.funcs.setdefault(rva, {"rva": rva, "name": self.img.fname(rva), "why": why, "kind": kind})
        ent = {ARG_REGS[i]: ("E", k) for i, k in entry.items()}
        tr = comp_track(self.img, rva, ent, self.src)
        for insn, k, acc, size, base, disp, indexed in tr["access"]:
            how = self.match(k, size, indexed) if acc in self.want else None
            if how:
                if size >= 16 and size > self.size:
                    how = "copy"  # an SSE block move over the field: a copy of the component (memcpy, copy ctor)
                self.hits.append({
                    "kind": COMP_HIT_KIND[how], "func_rva": rva,
                    "func": fn["name"], "func_va": self.img.va(rva), "site_va": self.img.va(insn.address),
                    "insn": C.insn_text(self.img, insn), "access": acc,
                    "expr": "comp%d+%#x%s" % (self.n, k, " + index" if indexed else ""), "via": why, "depth": depth,
                    "size": size, "watch": "%s+%#x:%d" % (base, disp, min(max(size, 1), 8))})
        if depth < self.args.depth:
            for insn, target, args in tr["calls"]:
                callees.append((self.img.func_start(target), args, "component %s passed from %s at %#x" % (
                    ", ".join("%s=comp+%#x" % (ARG_REGS[i], k) for i, k in sorted(args.items())), fn["name"],
                    self.img.va(insn.address)), "callee", depth + 1))
        return tr["returns"] if not entry else None


def comp_field(idx, cls, c, key):
    """(component offset, size, label) of `+off` or a mirror field (hash, m_<H>, OWLib name)."""
    s = key.strip()
    if s.startswith("+"):
        off = int(s[1:], 16)
        f = next((f for f in c["fields"] if f["offset"] is not None and
                  f["offset"] <= off < f["offset"] + (cls.field_size(f) or 1)), None)
        return off, None, ("mirror field %d m_%s" % (f["index"], f["hash"]) if f else None)
    t = s[2:] if s.startswith("m_") else s
    for f in c["fields"]:
        if f["hash"] == t.upper() or f["name"] == s:
            if f["offset"] is None:
                return None, None, "the mirror's offset in component %d is unknown" % c["id"]
            size = 16 if f["type"].get("kind") == "asset_ref" and not f["type"].get("array") else cls.field_size(f)
            return f["offset"], size, "mirror field %d m_%s (mirror +%#x)" % (f["index"], f["hash"], f["mirror_offset"])
    return None, None, "component %d has no mirror field %s" % (c["id"], key)


def comp_main(args, idx):
    t0 = time.time()
    c = idx.component(args.component)
    if c is None or not any(a["kind"] in ("getter", "id stub") for a in c["accessors"]):
        print("component %d: no getter or type id stub in the index (python -m tools.owindex comp%d; rebuild the "
              "index with python -m tools.owindex.build if it predates the accessor scan)" % (
                  args.component, args.component),
              file=sys.stderr)
        return 1
    cls = Classes(idx)
    args.max_funcs = args.max_funcs or 3000
    off, size, label = comp_field(idx, cls, c, args.cls)
    if off is None:
        print(label, file=sys.stderr)
        return 1
    size = args.size or size or 1
    img = Image(idx)
    s = CompSearch(idx, img, args, c, off, size)
    s.run()
    seen, hits = set(), []
    order = list(COMP_HIT_KIND.values())
    for x in sorted(s.hits, key=lambda x: (order.index(x["kind"]), x["depth"], x["site_va"])):
        if x["site_va"] not in seen:
            seen.add(x["site_va"])
            hits.append(x)
    res = {"component": args.component, "offset": off, "size": size, "field": label,
           "mode": "write" if args.write else "read", "functions_analysed": len(s.funcs),
           "wrappers": [{"rva": r, "va": img.va(r), "name": img.fname(r), "returns_offset": k}
                        for r, k in s.src["wrappers"].items()],
           "hits": hits, "notes": s.notes, "seconds": round(time.time() - t0, 1)}
    if args.json:
        print(json.dumps(res, indent=1, default=str))
        return 0
    p = print
    p("%s of component %d +%#x (%d bytes)%s" % ("writers" if args.write else "readers", args.component, off, size,
                                               ": " + label if label else ""))
    for n in s.notes:
        p("  note: " + n)
    for r, k in s.src["wrappers"].items():
        p("  returns the component%s: %#x %s" % (" +%#x" % k if k else "", img.va(r), img.fname(r)))
    p("functions analysed: %d (callee depth %d, wrapper levels %d)" % (len(s.funcs), args.depth, args.up))
    p("")
    if not hits:
        p("no %s found" % ("writers" if args.write else "readers"))
    for x in hits:
        p("  %#x %-28s %-10s %-40s %s%s" % (x["site_va"], x["func"], x["access"], x["insn"], x["expr"],
                                            COMP_HIT_NOTE.get(x["kind"], "")))
        p("      via %s" % x["via"])
    if hits:
        x = hits[0]
        p("")
        p("dynamic counterpart (tools/owdbg/watch; 'r' traps reads+writes, reports reads):")
        p("  python -m tools.owdbg.watch --arm-at 0x%X --field %s --access %s" % (
            x["site_va"], x["watch"], "w" if args.write else "r"))
    p("\n(%.1f s)" % res["seconds"])
    return 0


def hint(s, out):
    p = lambda *a: print(*a, file=out)  # noqa: E731
    acc = "w" if s.args.write else "r"
    size = min(s.size, 8)
    p("")
    p("dynamic counterpart (hardware watch in a live game, tools/owdbg/watch; 'r' traps reads+writes, reports reads):")
    shown = 0
    for x in sorted(s.hits, key=lambda x: KIND_ORDER[x["kind"]]):
        if x["kind"].startswith("tracked") and shown < 2:
            base = x["expr"].rsplit("+", 1)[0]
            p("  python -m tools.owdbg.watch --arm-at %s --field %s+%#x:%d --access %s" % (
                x["func"] if not x["func"].startswith("sub_") else "0x%X" % x["func_va"], base, s.off, size, acc))
            shown += 1
    for x in s.hits:
        if x["kind"] in ("heuristic", "signature") and shown < 3:
            base = x["expr"].rsplit("+", 1)[0]
            p("  python -m tools.owdbg.watch --arm-at 0x%X --field %s+%#x:%d --access %s   # at the %s site: %s"
              % (x["site_va"], base, s.off, size, acc, x["func"], x["insn"]))
            shown += 1
    if not shown:
        p("  python -m tools.owdbg.watch --arm-at <function receiving the instance> --field <reg>+%#x:%d --access %s"
          % (s.off, size, acc))
    p("  (the instance register is armed when execution reaches --arm-at; every later access to that instance's "
      "field is reported with its function)")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0], epilog=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cls", help="class key (hash, STU_<H>, OWLib name) or an address of its slot function; with "
                                "--component: the field (+offset or mirror field)")
    ap.add_argument("field", nargs="?", help="field hash, m_<H>, OWLib field name or +offset (hex)")
    ap.add_argument("--component", type=int, metavar="N",
                    help="entity component N instead of a class: `who_reads.py --component 74 +0xF6` or a mirror "
                         "field (`--component 74 m_CB549738`)")
    ap.add_argument("--up", type=int, default=2,
                    help="--component: levels of callers of functions that return the component (default 2)")
    ap.add_argument("--size", type=int, default=0, help="--component: bytes of the field at +offset (default 1)")
    ap.add_argument("--addr", action="store_true", help="--component: also list address-of (lea) uses")
    ap.add_argument("--write", action="store_true", help="writers instead of readers")
    ap.add_argument("--depth", type=int, default=2, help="callee levels (default 2)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--no-idb", action="store_true", help="skip the Hex-Rays typed-member search")
    ap.add_argument("--idb-timeout", type=float, default=0.0)
    ap.add_argument("--max-funcs", type=int, default=0,
                    help="functions to analyse (default 400; 3000 with --component)")
    ap.add_argument("--max-decompile", type=int, default=40, help="functions to decompile for typed hits")
    ap.add_argument("--all-heuristic", action="store_true", help="offset matches in callees too (noisy)")
    ap.add_argument("--from", dest="extra", action="append", default=[], metavar="FUNC",
                    help="extra seed function (index name, RVA or VA), e.g. the slot of a class that produces the "
                         "instance; repeatable")
    ap.add_argument("--signature", type=int, default=-1, metavar="N",
                    help="decode the N best whole-binary co-access candidates (default: 300 for classes without a "
                         "registration on their chain, else 0)")
    args = ap.parse_args(argv)
    sys.stdout.reconfigure(errors="replace")
    t0 = time.time()
    idx = Index.open()
    if args.component is not None:
        if args.field is not None:
            ap.error("--component N takes one field argument")
        return comp_main(args, idx)
    if args.field is None:
        ap.error("the field argument is required")
    args.max_funcs = args.max_funcs or 400
    cls = Classes(idx)
    h, info = cls.resolve(args.cls)
    if h is None:
        print(info, file=sys.stderr)
        return 1
    f = cls.find_field(h, args.field)
    if f is None:
        print("%s has no field %s; fields: %s" % (cls.label(h), args.field, ", ".join(
            "+%#x %s" % (x["offset"], cls.field_name(x)) for x in cls.fields(h))), file=sys.stderr)
        return 1
    img = Image(idx)
    s = Search(idx, img, cls, args, h, f)
    s.collect_seeds()
    for x in args.extra:
        rva = idx.lookup(x)
        if rva is None:
            try:
                rva = idx.to_rva(x)
            except ValueError:
                print("--from %s: not an index name (python -m tools.owindex <name>) or an address" % x,
                      file=sys.stderr)
                return 1
        s.seeds.append((img.func_start(rva), {}, "--from %s" % x, "user"))
    s.walk()
    if args.signature < 0:
        own = sum(1 for x in cls.fields(h) if x["owner"] == hx(h))
        data_only = cls.registrations(h)[1] is None
        args.signature = 300 if data_only and own >= SMALL_CLASS else 0
        if data_only and own < SMALL_CLASS:
            s.notes.append("signature scan skipped: %s has %d own fields, too few to tell it from other structs "
                           "(--signature N forces it; its hits would be guesses)" % (cls.label(h), own))
    if args.signature:
        s.signature()
    s.hash_constants()
    idb = C.Idb(enabled=not args.no_idb, timeout=args.idb_timeout)
    try:
        s.typed(idb)
    finally:
        idb.close()
    # one entry per (site, kind)
    seen, hits = set(), []
    for x in sorted(s.hits, key=lambda x: (KIND_ORDER[x["kind"]], -(x["score"] or 0), x["depth"] or 0)):
        k = (x["site_va"], x["insn"])
        if k not in seen:
            seen.add(k)
            hits.append(x)
    s.hits = hits
    res = {"class": hx(h), "label": cls.label(h), "field": {"hash": f["hash"], "name": cls.field_name(f),
           "offset": f["offset"], "size": f["size"], "type": cls.type_text(f), "owner": f["owner"]},
           "mode": "write" if args.write else "read", "idb": idb.status, "seeds": [
               {"rva": r, "va": img.va(r), "name": img.fname(r), "why": w, "kind": k} for r, _, w, k in s.seeds],
           "functions_analysed": len(s.funcs), "hits": hits, "notes": s.notes,
           "seconds": round(time.time() - t0, 1)}  # fmt: skip
    if args.json:
        print(json.dumps(res, indent=1, default=str))
        return 0
    out = sys.stdout
    p = lambda *a: print(*a, file=out)  # noqa: E731
    p("%s %s %s +%#x (%s, %s bytes)%s" % ("writers of" if args.write else "readers of", cls.label(h),
                                          cls.field_name(f), f["offset"], cls.type_text(f), f["size"],
                                          "  [field of %s]" % cls.label(int(f["owner"], 16)) if f["owner"] != hx(h) else ""))
    p("idb: %s" % (idb.status or "not used"))
    p("seeds (%d):" % len(s.seeds))
    for r, _, w, k in s.seeds[:60]:
        p("  %-16s %#x %s  -- %s" % (k, img.va(r), img.fname(r), w))
    if len(s.seeds) > 60:
        p("  ... %d more" % (len(s.seeds) - 60))
    for n in s.notes:
        p("  note: " + n)
    p("functions analysed: %d (depth %d)" % (len(s.funcs), args.depth))
    p("")
    if not hits:
        p("no %s found" % ("writers" if args.write else "readers"))
    cur = None
    for x in hits:
        if x["kind"] != cur:
            cur = x["kind"]
            p("%s:%s" % (cur, KIND_CAVEAT.get(cur, "")))
        p("  %#x %-28s %-10s %s%s" % (x["site_va"] or 0, x["func"], x["access"], x["insn"],
                                      ("  [" + x["note"] + "]") if x.get("note") else ""))
        p("      via %s" % x["via"])
    hint(s, out)
    p("\n(%.1f s)" % res["seconds"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
