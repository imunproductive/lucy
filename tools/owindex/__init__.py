"""Index of the Overwatch 1.74 client's runtime tables (build 104319), built by tools/owindex/build.py.

    from tools.owindex import Index
    idx = Index.open()            # research/out/index/ow_index.sqlite
    idx.stu("CD7F3617"); idx.field("m_B350E4FD"); idx.addr(0x7FF789BE27C0); idx.name(rva); idx.lookup(name)
    idx.component(74); idx.component_of_stu("8FE674F9"); idx.component_accessors(); idx.component_uses(74)

Addresses: RVAs are ints; addr() accepts VAs at 0x140000000 (IDB), 0x7FF788F10000 (ORPHEUS/questions),
0x7FF6D1220000 (kjs notes) or a bare RVA. Hashes in returned dicts are 8-digit upper-case hex strings.
"""

import json
import sqlite3
from pathlib import Path

from tools import paths

DEFAULT_DB = paths.INDEX
BASES = {"ida": 0x140000000, "orpheus": 0x7FF788F10000, "kjs": 0x7FF6D1220000}

_FUNC_WHAT = {
    ("evaluate", "config"): "evaluator of {c}",
    ("evaluate", "condition"): "evaluator of condition {c}",
    ("execute", "action"): "executor of action {c}",
    ("run", "action"): "run (calls execute, follows the output plug) of action {c}",
    ("run", "condition"): "run (calls evaluate, follows the true/false plug) of condition {c}",
    ("run", "entry"): "run of entry {c}",
    ("reader", "state"): "reader of state {c}",
    ("block", "state"): "property block reader of state {c}",
    ("writer", "state"): "writer of state {c}",
    ("type_tag", None): "runtime type tag stub of {k} {c}",
    ("factory", None): "factory of {k} {c}",
    ("construct", None): "construct of {k} {c}",
    ("destroy", None): "destroy/size of {k} {c}",
}
_DATA_WHAT = {
    "descriptor": "STU descriptor of {c}",
    "typeinfo": "node typeinfo of {c}",
    "node_vtable": "node vtable of {c}",
    "record": "registration record of {k} {c}",
    "runtime_vtable": "runtime vtable of {k} {c}",
    "field": "field entry {f} of {c}",
}
_ROLE_ORDER = ["evaluate", "execute", "reader", "block", "writer", "run", "construct", "factory", "type_tag",
               "destroy", "descriptor", "typeinfo", "node_vtable", "record", "runtime_vtable", "field"]  # fmt: skip


def _hx(h):
    return None if h is None else f"{h:08X}"


def _parse_int(x):
    """int, or a hex string (with or without 0x; IDA-style '14249F530', windbg-style '7ff7`89be27c0')."""
    if isinstance(x, int):
        return x
    s = str(x).strip().replace("`", "").replace("_", "")
    return int(s[2:] if s.lower().startswith("0x") else s, 16)


class Index:
    def __init__(self, path=None):
        path = Path(path or DEFAULT_DB)
        if not path.exists():
            raise FileNotFoundError(f"{path}: build it with `python -m tools.owindex.build` (ORPHEUS root)")
        self.path = path
        self.db = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True, check_same_thread=False)
        self.meta = {k: json.loads(v) for k, v in self.db.execute("SELECT key, value FROM meta")}
        self.base = self.meta["base"]
        self.image_size = self.meta["image_size"]
        self._names, self._parent, self._src = {}, {}, {}
        for h, name, src, parent in self.db.execute("SELECT hash, name, name_source, parent_hash FROM stu"):
            self._names[h], self._parent[h], self._src[h] = name, parent, src

    @classmethod
    def open(cls, path=None):
        return cls(path)

    def close(self):
        self.db.close()

    # --- addresses ---
    def to_rva(self, x, base=None):
        """VA in a known base (or this index's base, or `base`, e.g. a live ASLR base) or a bare RVA -> RVA."""
        v = _parse_int(x)
        for b in (*((base,) if base is not None else ()), *BASES.values(), self.base):
            if b <= v < b + self.image_size:
                return v - b
        if 0 <= v < self.image_size:
            return v
        raise ValueError(f"{v:#x} is not an RVA (< {self.image_size:#x}) or a VA at a known base")

    # --- classes ---
    def label(self, h):
        """'STU_<H>' plus the OWLib label when there is one."""
        if h is None:
            return None
        name = self._names.get(h)
        return f"STU_{h:08X}" + (f" ({name})" if self._src.get(h) == "owlib" else "")

    def ancestors(self, h):
        out = []
        while h is not None and h not in out:
            out.append(h)
            h = self._parent.get(h)
        return out

    def _resolve_stu(self, key):
        if isinstance(key, int):
            return key if key in self._names else None
        s = str(key).strip()
        t = s[4:] if s.upper().startswith("STU_") else s
        if t.lower().startswith("0x"):
            t = t[2:]
        if len(t) <= 8 and all(ch in "0123456789abcdefABCDEF" for ch in t) and int(t, 16) in self._names:
            return int(t, 16)
        row = self.db.execute("SELECT hash FROM stu WHERE name = ? ORDER BY hash LIMIT 1", (s,)).fetchone()
        return row[0] if row else None

    def stus(self):
        """All class hashes (ints)."""
        return sorted(self._names)

    def _type(self, text):
        t = json.loads(text)
        if t.get("kind") in ("stu", "asset_ref", "hashmap") and t.get("hash"):
            h = int(t["hash"], 16)
            if self._src.get(h) == "owlib":
                t["label"] = self._names[h]
        return t

    def _registration(self, row):
        (idx, rec, node, h, ti, kind, fac, con, des, prio, win, vt, nslots, slots) = row
        return {
            "index": idx, "kind": kind, "class_hash": _hx(h), "record_rva": rec, "node_rva": node,
            "factory_rva": fac, "construct_rva": con, "destroy_rva": des, "typeinfo_rva": ti, "priority": prio,
            "winner": bool(win), "runtime_vtable_rva": vt, "vtable_slots": nslots, "slots": json.loads(slots),
        }  # fmt: skip

    def records(self, kind=None):
        """Registration list records in list order, optionally of one kind (state, config, action, ...)."""
        sql, args = "SELECT * FROM registration", ()
        if kind:
            sql, args = sql + " WHERE kind = ?", (kind,)
        return [self._registration(r) for r in self.db.execute(sql + " ORDER BY idx", args)]

    def stu(self, key):
        h = self._resolve_stu(key)
        if h is None:
            return None
        row = self.db.execute("SELECT * FROM stu WHERE hash = ?", (h,)).fetchone()
        _, name, src, desc, parent, ti, nvt, size, align, _n = row
        fields = [
            {"hash": _hx(fh), "name": fname, "name_source": fsrc, "offset": off, "type": self._type(t),
             "flags": fl, "flags2": fl2, "entry_rva": e}
            for fh, fname, fsrc, off, t, fl, fl2, e in self.db.execute(
                "SELECT hash, name, name_source, offset, type, flags, flags2, entry_rva FROM field "
                "WHERE class_hash = ? ORDER BY idx", (h,))
        ]  # fmt: skip
        regs = [
            self._registration(r)
            for r in self.db.execute("SELECT * FROM registration WHERE class_hash = ? ORDER BY idx", (h,))
        ]
        effective = None
        for a in self.ancestors(h):
            r = self.db.execute(
                "SELECT idx, kind FROM registration WHERE class_hash = ? AND winner = 1 ORDER BY idx DESC LIMIT 1",
                (a,),
            ).fetchone()
            if r:
                effective = {"via_hash": _hx(a), "record_index": r[0], "kind": r[1], "own": a == h}
                break
        children = [_hx(c) for (c,) in self.db.execute("SELECT hash FROM stu WHERE parent_hash = ? ORDER BY hash", (h,))]
        return {
            "hash": _hx(h), "name": name, "name_source": src, "descriptor_rva": desc, "parent_hash": _hx(parent),
            "parent_name": self._names.get(parent), "ancestors": [_hx(a) for a in self.ancestors(h)[1:]],
            "children": children, "node_vtable_rva": nvt, "node_typeinfo_rva": ti, "size": size, "align": align,
            "fields": fields, "registrations": regs, "effective_registration": effective,
        }  # fmt: skip

    def field(self, key):
        """Every class with this field: key = hash int, 'm_<H>', '<H>' or an OWLib field name."""
        if isinstance(key, int):
            where, arg = "f.hash = ?", key
        else:
            s = str(key).strip()
            t = s[2:] if s.startswith("m_") else s
            if len(t) == 8 and all(ch in "0123456789abcdefABCDEF" for ch in t):
                where, arg = "f.hash = ?", int(t, 16)
            else:
                where, arg = "f.name = ?", s
        rows = self.db.execute(
            "SELECT f.class_hash, s.name, f.hash, f.name, f.name_source, f.offset, f.type, f.flags, f.flags2, "
            f"f.entry_rva FROM field f JOIN stu s ON s.hash = f.class_hash WHERE {where} ORDER BY f.class_hash",
            (arg,),
        )
        return [
            {"class_hash": _hx(ch), "class_name": cname, "hash": _hx(fh), "name": fname, "name_source": fsrc,
             "offset": off, "type": self._type(t), "flags": fl, "flags2": fl2, "entry_rva": e}
            for ch, cname, fh, fname, fsrc, off, t, fl, fl2, e in rows
        ]  # fmt: skip

    # --- names ---
    def name(self, rva):
        """Best name: docs/analysis first, then index-derived (Eval_STU_x, Read_STU_x, STUDesc_STU_x, ...)."""
        row = self.db.execute(
            "SELECT name FROM symbol WHERE rva = ? ORDER BY prio, kind = 'func' DESC LIMIT 1", (rva,)
        ).fetchone()
        return row[0] if row else None

    def lookup(self, name):
        """Exact name -> RVA (index-derived or docs/analysis names), else None."""
        row = self.db.execute("SELECT rva FROM symbol WHERE name = ? ORDER BY prio LIMIT 1", (name,)).fetchone()
        return row[0] if row else None

    def names(self):
        """[(rva, best name, 'func'|'data')] for every named RVA."""
        out, last = [], None
        for rva, name, kind in self.db.execute(
            "SELECT rva, name, kind FROM symbol ORDER BY rva, prio, kind = 'func' DESC"
        ):
            if rva != last:
                out.append((rva, name, kind))
                last = rva
        return out

    # --- addr ---
    def _owner(self, owners):
        if len(owners) == 1:
            return owners[0]
        for c in owners:
            if all(c in self.ancestors(o) for o in owners):
                return c
        return None

    def _describe(self, role, kind, rows, detail=None):
        """rows: [(class_hash, reg_idx, winner)] of one (role, kind) at one address."""
        owners = sorted({h for h, _, _ in rows if h is not None})
        k = kind or ""
        if role in _DATA_WHAT:
            h, idx, win = rows[0]
            text = _DATA_WHAT[role].format(c=self.label(h), k=k, f=detail).replace("  ", " ")
            if role == "field" and detail:
                fname = self.db.execute(
                    "SELECT name FROM field WHERE class_hash = ? AND hash = ?", (h, int(detail, 16))
                ).fetchone()
                text = _DATA_WHAT[role].format(c=self.label(h), f=fname[0] if fname else f"m_{detail}")
            if role in ("record", "runtime_vtable") and idx is not None:
                text += f" (record #{idx}{'' if win else ', not the winner'})"
            return text
        tmpl = _FUNC_WHAT.get((role, kind)) or _FUNC_WHAT.get((role, None)) or f"{role} of {{k}} {{c}}"
        owner = self._owner(owners)
        if owner is not None:
            text = tmpl.format(c=self.label(owner), k=k).replace("  ", " ")
            if len(owners) > 1:
                text += f" (also used by {len(owners) - 1} subclasses)"
        else:
            shown = ", ".join(self.label(h) for h in owners[:3])
            more = f" (+{len(owners) - 3} more)" if len(owners) > 3 else ""
            text = tmpl.format(c=shown + more, k=k).replace("  ", " ") + f" (shared by {len(owners)} classes)"
        if not any(w for _, _, w in rows):
            text += " (record not the winner)"
        return text

    def _whats_at(self, rva):
        groups = {}
        for role, kind, h, idx, win, detail in self.db.execute(
            "SELECT role, kind, class_hash, reg_idx, winner, detail FROM role WHERE rva = ?", (rva,)
        ):
            groups.setdefault((role, kind, detail if role == "field" else None), []).append((h, idx, win))
        keys = sorted(groups, key=lambda g: (_ROLE_ORDER.index(g[0]) if g[0] in _ROLE_ORDER else 99, str(g[1])))
        whats = [self._describe(role, kind, groups[(role, kind, det)], det) for role, kind, det in keys]
        # network tables (tools/owindex/net.py rows, rendered at build time by netnames.py)
        whats += [t for (t,) in self.db.execute("SELECT text FROM netrole WHERE rva = ? ORDER BY rowid", (rva,))]
        return whats

    def addr(self, x, base=None):
        """x: VA at a known base or at `base` (a live run's image base), or an RVA."""
        rva = self.to_rva(x, base)
        out = {"rva": rva, **{k: b + rva for k, b in BASES.items()}, "name": self.name(rva)}
        if self.base not in BASES.values():
            out["index_va"] = self.base + rva
        whats = self._whats_at(rva)
        # inside a data object (descriptor, field entry, record, vtable)
        for start, size, role, kind, h, idx, win, detail in self.db.execute(
            "SELECT * FROM role WHERE size > 0 AND rva < ? AND rva > ? AND rva + size > ?", (rva, rva - 0x1000, rva)
        ):
            text = self._describe(role, kind, [(h, idx, win)], detail)
            off = rva - start
            if role == "runtime_vtable" and off % 8 == 0:
                whats.append(f"slot {off // 8} (+{off:#x}) of {text}")
            else:
                whats.append(f"{text} +{off:#x}")
        # containing function (.pdata; chained fragments resolved to the primary function)
        out["function_rva"] = None
        row = self.db.execute("SELECT begin, end, root FROM func WHERE begin <= ? ORDER BY begin DESC LIMIT 1", (rva,)).fetchone()
        if row and row[1] > rva:
            begin, end, root = row
            out["function_rva"] = root
            out["function"] = {"begin": begin, "end": end, "root": root, "name": self.name(root)}
            if root != rva:
                inner = self._whats_at(root)
                fname = self.name(root)
                if inner:
                    whats += [f"in {w} (+{rva - root:#x})" for w in inner]
                elif fname:
                    whats.append(f"in {fname} (+{rva - root:#x})")
                if begin != root:
                    whats.append(f"in a chained .pdata fragment {begin:#x}-{end:#x} of function {root:#x}")
        # names from docs/analysis
        for name, src in self.db.execute("SELECT name, source FROM symbol WHERE rva = ? AND prio = 0", (rva,)):
            whats.append(f"{name} ({src})")
        row = self.db.execute("SELECT begin, end, confidence, area FROM obf WHERE begin <= ? ORDER BY begin DESC LIMIT 1", (rva,)).fetchone()
        if row and row[1] > rva:
            whats.append(f"obfuscated function ({row[2]}, {row[3]}; {row[0]:#x}-{row[1]:#x}, use Rose)")
        out["whats"] = whats
        out["what"] = "; ".join(whats) if whats else None
        return out

    # --- net tables (tools/owindex/net.py) ---
    def _net(self, kind):
        return [json.loads(d) for (d,) in self.db.execute("SELECT data FROM net WHERE kind = ? ORDER BY idx", (kind,))]

    def messages(self):
        return self._net("messages")

    def components(self):
        return self._net("components")

    def protocols(self):
        return self._net("protocols")

    # --- entity components (tools/owindex/comp.py) ---
    def _comp_access(self, sql, args=()):
        if not self.db.execute("SELECT 1 FROM sqlite_master WHERE name = 'comp_access'").fetchone():
            return []  # index built before the accessor scan: rebuild it
        return self.db.execute(sql, args).fetchall()

    def component_accessors(self, component=None):
        """Getters (`kind` "getter": writes (mask, low mask, shift, word) of `component`), type id stubs ("id stub":
        `mov byte [rcx], component`, used by the factory and the pool of all instances) and the has-component bit
        test ("has", component None): [{"rva", "component", "kind", "name", "calls"}]."""
        sql = ("SELECT a.rva, a.component, a.kind, (SELECT COUNT(*) FROM comp_access c WHERE c.accessor_rva = a.rva "
               "AND c.kind LIKE '% call') FROM comp_access a WHERE a.kind IN ('getter', 'id stub', 'has')")
        args = ()
        if component is not None:
            sql, args = sql + " AND a.component = ?", (component,)
        return [{"rva": r, "component": c, "kind": k, "name": self.name(r), "calls": n}
                for r, c, k, n in self._comp_access(sql + " ORDER BY a.component, a.kind, a.rva", args)]

    def component_uses(self, component):
        """Call sites of component N's getters ("getter call"), its type id stub ("id stub call": factories, pool
        functions) and has-component tests of N ("has call"): [{"site", "func_rva", "kind", "accessor_rva"}]."""
        return [{"site": s, "func_rva": f, "kind": k, "accessor_rva": a} for s, f, k, a in self._comp_access(
            "SELECT rva, func_rva, kind, accessor_rva FROM comp_access WHERE component = ? AND kind LIKE '% call' "
            "ORDER BY rva", (component,))]

    # --- function tables ---
    def _qwords(self):
        if getattr(self, "_q64", None) is None:
            import numpy as np

            self._q64 = np.fromfile(self.meta["dump"], dtype="<u8")
        return self._q64

    def holders(self, x, base=None):
        """Every function table slot in .rdata/.data holding the function at x: [{"table_rva", "slot", "slot_rva",
        "table_name", "table_source", "whats", "slot_roles"}]. The table is the nearest start at or before the slot
        within its run of code pointers: an STU runtime vtable, an address code references ("code") or a data
        pointer points at ("data"), else the run's start ("run"); `whats` is what the index knows of it (STU runtime
        vtable, component vtable, message class, docs/analysis name, ...)."""
        import numpy as np

        secs = self.meta.get("sections")
        if not secs or not self.db.execute("SELECT 1 FROM sqlite_master WHERE name = 'fntable'").fetchone():
            raise RuntimeError("index built before the function table scan: python -m tools.owindex.build")
        rva = self.to_rva(x, base)
        q = self._qwords()
        t0, t1 = (self.base + v for v in secs[".text"])
        lo, hi = secs[".rdata"][0], secs[".data"][1]
        out = []
        for loc in (int(i) * 8 for i in np.nonzero(q == np.uint64(self.base + rva))[0]):
            if not lo <= loc < hi:
                continue
            run = loc
            while run - 8 >= lo and t0 <= int(q[(run - 8) // 8]) < t1 and loc - run < 0x8000:
                run -= 8
            rows = self.db.execute(
                "SELECT rva, source FROM fntable WHERE rva BETWEEN ? AND ? UNION SELECT runtime_vtable_rva, "
                "'stu runtime vtable' FROM registration WHERE runtime_vtable_rva BETWEEN ? AND ? ORDER BY 1 DESC",
                (run, loc, run, loc)).fetchall()
            start, source = rows[0] if rows else (run, "run")
            roles = sorted({role for (slots,) in self.db.execute(
                "SELECT slots FROM registration WHERE runtime_vtable_rva = ?", (start,))
                for role, v in json.loads(slots).items() if v == rva})
            whats = self._whats_at(start) + [f"{n} ({s})" for n, s in self.db.execute(
                "SELECT name, source FROM symbol WHERE rva = ? AND prio = 0", (start,))]
            out.append({"table_rva": start, "slot": (loc - start) // 8, "slot_rva": loc, "table_name": self.name(start),
                        "table_source": source, "whats": whats, "slot_roles": roles})
        return out

    def component(self, n):
        """Component N: the net.py row plus `data_class` (STU entity component class the definitions map to N),
        `mirror_class` / `mirror_offset` (the networked STU instance inside the component) and `fields` (each
        mirror field with its offset in the mirror and in the component), `accessors`, `uses` (call sites)."""
        row = next((c for c in self.components() if c["id"] == n), None)
        acc = self.component_accessors(n)
        if row is None and not acc:
            return None
        out = dict(row or {"id": n})
        hs = out.get("class_hashes") or []
        out["data_class"] = _hx(hs[0]) if len(hs) == 1 else None
        out["data_classes"] = [_hx(h) for h in hs]
        out["data_class_label"] = self.label(hs[0]) if len(hs) == 1 else None
        mh, mo = out.get("mirror_hash"), out.get("mirror_offset")
        out["mirror_class"], out["mirror_label"] = _hx(mh), self.label(mh) if mh in self._names else None
        fields, i = [], 0
        for a in reversed(self.ancestors(mh) if mh in self._names else []):
            for f in self.stu(a)["fields"]:
                fields.append({"index": i, "hash": f["hash"], "name": f["name"], "name_source": f["name_source"],
                               "class_hash": _hx(a), "mirror_offset": f["offset"],
                               "offset": None if mo is None else mo + f["offset"], "type": f["type"],
                               "flags": f["flags"]})
                i += 1
        out["fields"] = sorted(fields, key=lambda f: f["mirror_offset"])
        out["accessors"] = acc
        out["uses"] = self.component_uses(n)
        return out

    def component_of_stu(self, key):
        """Component id whose data class (the definition mapper's STU class) is the class, else whose mirror
        class is; None when neither."""
        h = self._resolve_stu(key)
        if h is None:
            return None
        comps = self.components()
        for test in (lambda c: h in (c.get("class_hashes") or []), lambda c: c.get("mirror_hash") == h):
            hit = next((c["id"] for c in comps if test(c)), None)
            if hit is not None:
                return hit
        return None
