"""Static list of code that reads (or writes) one field of an STU class (Overwatch 1.74, build 104319).

    python tools/who_reads.py <class key> <field hash | m_<H> | OWLib name | +offset> [--write] [--depth N]
                              [--json] [--no-idb] [--max-funcs N] [--all-heuristic]

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

from capstone import x86  # noqa: E402

from tools.owindex import Index  # noqa: E402
from tools.studossier import code as C  # noqa: E402
from tools.studossier.classes import ARG_REGS, STU_ARG, Classes, hx  # noqa: E402
from tools.studossier.image import Image, reg64  # noqa: E402

KIND_ORDER = {"typed": 0, "tracked": 1, "tracked (base class)": 2, "hash constant": 3, "heuristic": 4, "signature": 5}


class Search:
    def __init__(self, idx, img, cls, args, h, field):
        self.idx, self.img, self.cls, self.args, self.h, self.f = idx, img, cls, args, h, field
        self.off, self.size = field["offset"], field["size"] or 1
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
    ap.add_argument("cls", help="class key (hash, STU_<H>, OWLib name) or an address of its slot function")
    ap.add_argument("field", help="field hash, m_<H>, OWLib field name or +offset (hex)")
    ap.add_argument("--write", action="store_true", help="writers instead of readers")
    ap.add_argument("--depth", type=int, default=2, help="callee levels (default 2)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--no-idb", action="store_true", help="skip the Hex-Rays typed-member search")
    ap.add_argument("--idb-timeout", type=float, default=0.0)
    ap.add_argument("--max-funcs", type=int, default=400, help="functions to analyse (default 400)")
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
        args.signature = 300 if cls.registrations(h)[1] is None else 0
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
            p("%s:" % cur)
        p("  %#x %-28s %-10s %s%s" % (x["site_va"] or 0, x["func"], x["access"], x["insn"],
                                      ("  [" + x["note"] + "]") if x.get("note") else ""))
        p("      via %s" % x["via"])
    hint(s, out)
    p("\n(%.1f s)" % res["seconds"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
