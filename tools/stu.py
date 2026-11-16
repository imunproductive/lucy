"""One-shot dossier of a statescript/STU class of the Overwatch 1.74 client (build 104319).

    python tools/stu.py <class key | address | name> [--depth N] [--json] [--no-code] [--no-idb]
                        [--shared] [--no-where-used] [--max-lines N]

key: CD7F3617, STU_CD7F3617, an OWLib class name; an address (0x7FF789B983D0 ORPHEUS VA, 0x140C883D0 IDB VA,
0xC883D0 RVA) of a slot function or class object (descriptor, typeinfo, vtable, record); or an index name
(Eval_STU_CD7F3617, Read_STU_9935FBC8). An address resolves to the class owning the function.

Prints: class (hash, OWLib label, size, parents, children), fields (offset, type, flags/flags2 with their
statescript network meaning), registrations (kind, winner, priority, runtime vtable, named slots; the
inherited registration the client uses when the class has none), and per slot function its code: Hex-Rays
pseudocode from the game IDB (tools/ida_game) when it can be opened, else capstone disassembly of
research/out/ida_game/ow_game.exe (the dump with restored code and Rose deobfuscation), both annotated with this
class's fields; direct callees to --depth with index names and the classes of callees that are slot
functions; string literals. Functions shared with unrelated classes (base implementations) are listed but
not decompiled unless --shared; tiny bodies are shown as one line. Then where the class is used in the
game data (ORPHEUS `script query class`, temp copy of the ORPHEUS state folder).
Addresses: `rva` plus the ORPHEUS VA (0x7FF788F10000 + rva).
"""
import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.owindex import Index  # noqa: E402
from tools.studossier import code as C  # noqa: E402
from tools.studossier.classes import FUNC_ROLES, STU_ARG, Classes, hx, net_meaning  # noqa: E402
from tools.studossier.image import Image  # noqa: E402

SLOT_ORDER = ["evaluate", "execute", "run", "reader", "block", "writer", "construct", "factory", "type_tag"]
CODE_SLOTS = {"evaluate", "execute", "run", "reader", "block", "writer", "construct"}


class Dossier:
    def __init__(self, idx, img, cls, idb, args):
        self.idx, self.img, self.cls, self.idb, self.args = idx, img, cls, idb, args

    def va(self, rva):
        return None if rva is None else self.idx.base + rva

    def addr(self, rva):
        return "-" if rva is None else "%#x (%#x)" % (rva, self.idx.base + rva)

    # ------------------------------------------------------------------ data
    def build(self, h, info):
        idx, cls = self.idx, self.cls
        st = idx.stu(hx(h))
        d = {"query": self.args.key, "resolved": info, "class": {
            "hash": st["hash"], "label": idx.label(h), "owlib_label": st["name"] if st["name_source"] == "owlib" else None,
            "size": st["size"], "descriptor_rva": st["descriptor_rva"], "node_typeinfo_rva": st["node_typeinfo_rva"],
            "node_vtable_rva": st["node_vtable_rva"],
            "parents": [{"hash": a, "label": idx.label(int(a, 16))} for a in st["ancestors"]],
            "children": [{"hash": c, "label": idx.label(int(c, 16))} for c in st["children"]],
        }}  # fmt: skip
        regs, eff, inherited = cls.registrations(h)
        is_state = (eff or {}).get("kind") == "state"
        d["fields"] = [{
            "offset": f["offset"], "hash": f["hash"], "name": cls.field_name(f), "owner": f["owner"],
            "inherited": f["owner"] != st["hash"], "type": cls.type_text(f), "size": f["size"],
            "flags": f["flags"], "flags2": f["flags2"],
            "net": net_meaning(f) if is_state and f["owner"] == st["hash"] else None,
        } for f in cls.fields(h)]  # fmt: skip
        d["registrations"] = regs
        d["effective_registration"] = (
            {"inherited_from": eff["via_hash"], "label": idx.label(int(eff["via_hash"], 16)), **eff}
            if eff and inherited else None)  # fmt: skip
        d["functions"] = self.functions(h, regs, eff if inherited else None, info)
        from tools.studossier import orpheus
        d["net_codec"] = orpheus.codec(st["hash"]) if is_state else None
        if not self.args.no_where_used:
            d["where_used"] = orpheus.where_used(st["hash"])
        return d

    def functions(self, h, regs, eff, info):
        """Slot functions (own registrations, else the inherited one) with attribution and code."""
        roles = {}  # rva -> [(role, kind, record index, winner)]
        srcs = list(regs) or ([eff] if eff else [])
        for r in sorted(srcs, key=lambda r: not r["winner"]):
            items = list(r["slots"].items()) + [("construct", r["construct_rva"]), ("factory", r["factory_rva"])]
            for slot, rva in items:
                if rva:
                    roles.setdefault(rva, []).append((slot, r["kind"], r["index"], r["winner"]))
        focus = info.get("rva") if info.get("via") == "address" else None
        if focus is not None and focus not in roles and any(r.split(" ")[0] in FUNC_ROLES for r in info.get("roles", [])):
            roles[focus] = [(r.split(" ")[0], None, None, None) for r in info.get("roles", ["?"])]
        order = sorted(roles, key=lambda rva: (rva != focus, min(SLOT_ORDER.index(s) if s in SLOT_ORDER else 99
                                                                 for s, *_ in roles[rva])))
        via = eff["via_hash"] if eff and not regs else None
        return [self.function(h, rva, roles[rva], inherited=via, focus=rva == focus)
                for rva in order]

    def function(self, h, rva, roles, inherited, focus):
        img, cls, args = self.img, self.cls, self.args
        slots = sorted({s for s, *_ in roles}, key=lambda s: SLOT_ORDER.index(s) if s in SLOT_ORDER else 99)
        kind = next((k for _, k, *_ in roles if k), None)
        root = img.func_start(rva)
        size = img.func_size(root) if root is not None else 0
        insns = img.decode(root) if root is not None else []
        tiny = size <= 16 and not any(i.mnemonic == "call" for i in insns)
        att = cls.attribution(h, rva)
        fn = {"rva": rva, "va": self.va(rva), "name": img.fname(rva), "slots": slots, "kind": kind, "size": size,
              "attribution": att, "tiny": tiny, "inherited": inherited, "focus": focus}  # fmt: skip
        if inherited:
            fn["note"] = ("inherited: %s's registration, which the client uses for this class (the parent's code, "
                          "used by %d registering classes)" % (cls.label(int(inherited, 16)), att["users"]))
            fn["attribution"] = dict(att, status="inherited")
        elif att["status"] == "shared":
            fn["note"] = "shared by %d classes (common ancestor %s): base implementation, not this class's code" % (
                att["users"], cls.label(att["common"]))
        if img.obfuscated(rva):
            fn["obfuscated"] = img.obfuscated(rva)
        if tiny:
            fn["code"] = "; ".join(C.insn_text(img, i) for i in insns)
            fn["source"] = "capstone"
            return fn
        show = (fn.get("note") is None or args.shared or focus) and not args.no_code
        show = show and (any(s in CODE_SLOTS for s in slots) or focus)
        if not show:
            return fn
        argi = next((STU_ARG[(k, s)] for s, k, *_ in roles if (k, s) in STU_ARG), None)
        stu_h = h if inherited or att["status"] != "shared" else (att.get("common") or h)
        idb = self.idb.get()
        if idb is not None:
            try:
                text = idb.decompile(rva)
                names, typed = C.pseudocode_params(text)
                tracked = dict(typed)
                if argi is not None and argi < len(names) and names[argi]:
                    tracked.setdefault(names[argi], stu_h)
                text, hits = C.annotate_pseudocode(cls, text, tracked, heur_class=h)
                fn.update(source="hexrays", code=text, stu_param=[k for k in tracked])
            except Exception as e:  # noqa: BLE001  (DecompileError and idalib errors: fall back)
                fn["decompile_error"] = str(e)
        if "code" not in fn:
            entry = C.entry_state(argi, stu_h) if argi is not None else {}
            lines, _ = C.disasm_listing(img, cls, root, entry, heur_class=h, limit=args.max_lines)
            fn.update(source="capstone", code="\n".join(lines),
                      stu_arg=C.ARG_REGS[argi] if argi is not None and argi < 4 else None)
        fn["callees"] = self.callee_tree(root, args.depth, set([root]))
        fn["strings"] = [{"site_va": self.va(s["site"]), "rva": s["rva"], "text": s["text"]}
                         for s in C.strings(img, root)]
        return fn

    def callee_tree(self, root, depth, seen):
        out = []
        if depth <= 0:
            return out
        for c in C.callees(self.img, root)[:60]:
            t = c["rva"]
            node = {"rva": t, "va": self.va(t), "name": self.img.fname(t), "kind": c["kind"],
                    "what": self.what(t)}  # fmt: skip
            if t not in seen and depth > 1:
                seen.add(t)
                sub = self.img.func_start(t)
                if sub is not None:
                    node["callees"] = self.callee_tree(sub, depth - 1, seen)
            out.append(node)
        return out

    def what(self, rva):
        try:
            whats = self.idx.addr(rva)["whats"]
        except ValueError:
            return None
        whats = [w for w in whats if not w.startswith(("in a chained", "in "))]
        return "; ".join(whats) or None


# ---------------------------------------------------------------------- text output
def render(d, out):
    c = d["class"]
    p = lambda *a: print(*a, file=out)  # noqa: E731
    va = lambda rva: "-" if rva is None else "%#x (%#x)" % (rva, BASE + rva)  # noqa: E731
    r = d["resolved"]
    if r.get("via") == "address":
        p("%s -> %s of %s%s" % (d["query"], ", ".join(r["roles"]), c["label"],
                                ("  [%s]" % r["note"]) if r.get("note") else ""))
    p("STU_%s%s  size %#x" % (c["hash"], "  (OWLib label: %s)" % c["owlib_label"] if c["owlib_label"] else "",
                              c["size"] or 0))
    p("  descriptor %s  node typeinfo %s  node vtable %s" % (va(c["descriptor_rva"]), va(c["node_typeinfo_rva"]),
                                                             va(c["node_vtable_rva"])))
    p("  parents: %s" % (" -> ".join(x["label"] for x in c["parents"]) or "none"))
    ch = c["children"]
    p("  children (%d): %s%s" % (len(ch), ", ".join(x["label"] for x in ch[:24]), " ..." if len(ch) > 24 else ""))
    p("")
    own = [f for f in d["fields"] if not f["inherited"]]
    p("fields (%d own%s):" % (len(own), "; net = what the statescript property codec's flag rule does with the field "
                                         "(8 never sent, 0x10 owner frames only, 0x20 full frames only; whether the "
                                         "class sends a property block at all: network codec below)"
                              if any(f["net"] for f in own) else ""))
    if not own:
        p("  none")
    for f in own:
        p("  +%#06x %-30s %-34s flags %#x/%#x%s" % (f["offset"], f["name"], f["type"], f["flags"], f["flags2"],
                                                    "  net: " + f["net"] if f["net"] else ""))
    inh = {}
    for f in d["fields"]:
        if f["inherited"]:
            inh.setdefault(f["owner"], []).append(f)
    for owner, fs in inh.items():
        lab = next((x["label"] for x in c["parents"] if x["hash"] == owner), "STU_" + owner)
        p("  inherited from %s: %s" % (lab, ", ".join("+%#x %s" % (f["offset"], f["name"]) for f in fs)))
    p("")
    p("registrations:")
    if not d["registrations"]:
        p("  none of its own")
    for g in d["registrations"]:
        p("  %s record #%d %s prio %d %s  runtime vtable %s" % (g["kind"], g["index"], va(g["record_rva"]),
                                                                g["priority"], "WINNER" if g["winner"] else "overridden",
                                                                va(g["runtime_vtable_rva"])))
        p("    factory %s  construct %s  destroy %s" % (va(g["factory_rva"]), va(g["construct_rva"]),
                                                       va(g["destroy_rva"])))
        p("    " + "  ".join("%s %s" % (k, va(v)) for k, v in g["slots"].items()))
    e = d["effective_registration"]
    if e:
        p("  effective (inherited, what the client looks up): %s %s record #%d prio %d, runtime vtable %s" % (
            e["label"], e["kind"], e["index"], e["priority"], va(e["runtime_vtable_rva"])))
        p("    " + "  ".join("%s %s" % (k, va(v)) for k, v in e["slots"].items()))
    elif not d["registrations"]:
        p("  no registration on the class chain (not a statescript node class; data only)")
    nc = d.get("net_codec")
    if nc:
        p("  network codec (ORPHEUS statescript_codecs.json): %s, %s; layout %s" % (
            nc.get("codec"), nc.get("status"), " ".join(
                "%s%s" % (op.get("op"), ":" + str(op.get("f")) if op.get("f") else "") for op in nc.get("layout", []))))
        if nc.get("evidence"):
            p("    evidence: %s" % nc["evidence"])
        if nc.get("usage"):
            p("    usage: %s" % ", ".join("%s %s" % (k, v) for k, v in nc["usage"].items()))
    if d.get("code_source"):
        p("")
        p("code source: %s" % d["code_source"])
    for fn in d["functions"]:
        p("")
        att = fn["attribution"]
        tag = []
        if fn["tiny"]:
            tag.append("tiny")
        if att["status"] in ("shared", "inherited"):
            tag.append(att["status"].upper())
        elif att.get("subclasses"):
            tag.append("also used by %d subclasses" % att["subclasses"])
        if fn.get("obfuscated"):
            tag.append("obfuscated (%s)" % fn["obfuscated"])
        p("== %s %s  %s  size %#x%s" % ("/".join(fn["slots"]), va(fn["rva"]), fn["name"], fn["size"],
                                        ("  [" + ", ".join(tag) + "]") if tag else ""))
        if fn.get("note"):
            p("   " + fn["note"])
        if fn["tiny"]:
            p("   " + fn["code"])
            continue
        if fn.get("decompile_error"):
            p("   decompile failed (%s); capstone instead" % fn["decompile_error"])
        if fn.get("code"):
            if fn.get("stu_arg"):
                p("   STU_%s instance in %s at entry (fields annotated; `?` = offset match through an untracked "
                  "register)" % (c["hash"], fn["stu_arg"]))
            for line in fn["code"].splitlines():
                p("   " + line)
        if fn.get("callees"):
            p("   callees:")
            _tree(p, fn["callees"], 2)
        if fn.get("strings"):
            p("   strings:")
            for s in fn["strings"]:
                p("     %#x: %s" % (s["site_va"], s["text"]))
    wu = d.get("where_used")
    if wu is not None:
        p("")
        p("where used in the game data (orpheus script query class %s):" % c["hash"])
        if wu.get("error"):
            p("  " + wu["error"])
        else:
            lines = wu["lines"]
            for ln in lines[:WU_LINES]:
                p("  " + (ln if len(ln) < 300 else ln[:300] + " ..."))
            if len(lines) > WU_LINES:
                p("  ... %d more lines (--json for all)" % (len(lines) - WU_LINES))


def _tree(p, nodes, ind):
    for n in nodes:
        p("%s%s %#x %s%s" % ("  " * ind, n["kind"], n["va"], n["name"], ("  -- " + n["what"]) if n.get("what") else ""))
        if n.get("callees"):
            _tree(p, n["callees"], ind + 1)


BASE = 0x7FF788F10000
WU_LINES = 40


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter,
                                 epilog=__doc__)
    ap.add_argument("key")
    ap.add_argument("--depth", type=int, default=1, help="callee levels to list (default 1)")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--no-code", action="store_true", help="class, fields and registrations only")
    ap.add_argument("--no-idb", action="store_true", help="capstone only, do not open the game IDB")
    ap.add_argument("--idb-timeout", type=float, default=0.0, help="seconds to wait for the IDB lock")
    ap.add_argument("--shared", action="store_true", help="also show code of shared/inherited functions")
    ap.add_argument("--no-where-used", action="store_true", help="skip the ORPHEUS game-data query")
    ap.add_argument("--max-lines", type=int, default=400, help="disassembly lines per function")
    args = ap.parse_args(argv)
    sys.stdout.reconfigure(errors="replace")
    global BASE
    t0 = time.time()
    idx = Index.open()
    BASE = idx.base
    cls = Classes(idx)
    h, info = cls.resolve(args.key)
    if h is None:
        print(info, file=sys.stderr)
        return 1
    img = Image(idx)
    idb = C.Idb(enabled=not args.no_idb and not args.no_code, timeout=args.idb_timeout)
    try:
        d = Dossier(idx, img, cls, idb, args).build(h, info)
        d["code_source"] = ("%s; " % idb.status if idb.status else "") + "disassembly from %s" % img.path.name
        if args.no_code:
            d["code_source"] = None
        d["seconds"] = round(time.time() - t0, 1)
        try:
            if args.json:
                print(json.dumps(d, indent=1, default=str))
            else:
                render(d, sys.stdout)
                print("\n(%.1f s)" % d["seconds"])
        except (BrokenPipeError, OSError):  # output piped into head & co.
            pass
    finally:
        idb.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
