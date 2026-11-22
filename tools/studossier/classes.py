"""Class resolution and per-class facts on top of tools.owindex.Index."""
import json
from pathlib import Path

# Lucy's statescript names (aliases such as "HeroSelect" for STU_0E913A0B), accepted as class keys.
NAMES_JSON = Path(__file__).resolve().parents[2] / "crates" / "fixed" / "data" / "statescript_names.json"

FUNC_ROLES = ("evaluate", "execute", "reader", "block", "writer", "run", "construct", "factory", "destroy", "type_tag")
DATA_ROLES = ("descriptor", "typeinfo", "node_vtable", "record", "runtime_vtable", "field")
# Slot -> argument index (0 = rcx) carrying the STU instance. Measured over every winning registration of the
# index whose class has fields (accesses at field offsets through the argument register, see
# tools/ida_game/stutypes.py): config evaluate 433/633, condition evaluate 4/4, action execute 101/164,
# action run 17/164 (the rest touch no field directly). State reader/writer/block functions work on the
# runtime state object and the bit stream, not on the STU instance.
STU_ARG = {("config", "evaluate"): 3, ("condition", "evaluate"): 3, ("action", "execute"): 3, ("action", "run"): 3}
ARG_REGS = ("rcx", "rdx", "r8", "r9")
# Field flags as the statescript property codec uses them (Lucy research/ida/extract_statescript_codecs.py:
# skipped when flags & 8, flags & 0x10 without an owner frame, flags & 0x20 without a full frame).
NET_FLAGS = ((8, "never sent"), (0x10, "owner frames only"), (0x20, "full frames only"))
PRIM_SIZE = {
    "u8": 1, "s8": 1, "u16": 2, "s16": 2, "u32": 4, "s32": 4, "f32": 4, "u64": 8, "s64": 8, "f64": 8,
    "teVec2": 8, "teVec3": 12, "teVec3A": 16, "teVec4": 16, "teQuat": 16, "teColorRGB": 12, "teColorRGBA": 16,
    "teUUID": 16, "DBID": 16, "teEntityID": 4, "teMtx43A": 64,
}  # fmt: skip
HEX = set("0123456789abcdefABCDEF")


def hx(h):
    return "%08X" % h


def net_meaning(f):
    """What the statescript property codec does with a state class's own field (flags + type)."""
    flags = f["flags"]
    if flags & 8:
        return "never sent"
    if (f["type"] or {}).get("kind") != "primitive" or (f["type"] or {}).get("array"):
        return "not a primitive (codec: not decoded)"
    parts = [text for bit, text in NET_FLAGS[1:] if flags & bit]
    return ", ".join(parts) if parts else "always sent"


class Classes:
    def __init__(self, idx):
        self.idx = idx
        self._sizes = dict(idx.db.execute("SELECT hash, size FROM stu"))
        self._fields = {}
        self._names_json = None

    # ---------------------------------------------------------------- Lucy names
    def _names(self):
        if self._names_json is None:
            try:
                self._names_json = json.loads(NAMES_JSON.read_text(encoding="utf-8")).get("classes", {})
            except (OSError, ValueError):
                self._names_json = {}
        return self._names_json

    def alias(self, h):
        """Lucy alias of class h (statescript_names.json "STU_<H>": {"alias": ...}) or None."""
        v = self._names().get("STU_" + hx(h))
        return v.get("alias") if isinstance(v, dict) else None

    def alias_hash(self, name):
        """Class hash of a Lucy name (an alias, or a name keyed with {"hash": H}), case-insensitive."""
        low = name.lower()
        for k, v in self._names().items():
            if not isinstance(v, dict):
                continue
            if k.startswith("STU_") and str(v.get("alias", "")).lower() == low:
                return int(k[4:], 16)
            if k.lower() == low and v.get("hash"):
                return int(v["hash"], 16)
        return None

    # ---------------------------------------------------------------- naming
    def label(self, h):
        return self.idx.label(h) if h is not None else None

    def h(self, x):
        return x if isinstance(x, int) or x is None else int(x, 16)

    # ---------------------------------------------------------------- resolution
    def resolve(self, key):
        """key: class hash / STU_<H> / OWLib name / address (0x..., VA, RVA) / index name (Eval_STU_x).
        -> (class hash int, {"via": ..., "rva": function or object RVA, "roles": [...]}) or (None, error)."""
        s = str(key).strip()
        is_addr = s.lower().startswith("0x") or (len(s) > 8 and all(c in HEX for c in s.replace("`", "")))
        if not is_addr:
            st = self.idx.stu(s)
            if st:
                return int(st["hash"], 16), {"via": "class"}
            rva = self.idx.lookup(s)
            if rva is None:
                ah = self.alias_hash(s)
                if ah is not None and self.idx.stu(hx(ah)):
                    return ah, {"via": "alias"}
                return None, "%s: no class, Lucy alias, address or index name" % key
        else:
            try:
                rva = self.idx.to_rva(s)
            except ValueError as e:
                return None, str(e)
        return self.class_at(rva)

    def class_at(self, rva):
        """Class owning a function (slot role of its .pdata root) or a data object containing rva."""
        rows = self.idx.db.execute(
            "SELECT rva, role, kind, class_hash, winner FROM role WHERE rva = ? OR (size > 0 AND rva < ? AND rva > ? "
            "AND rva + size > ?)", (rva, rva, rva - 0x1000, rva)).fetchall()  # fmt: skip
        root = None
        if not rows:
            row = self.idx.db.execute(
                "SELECT begin, end, root FROM func WHERE begin <= ? ORDER BY begin DESC LIMIT 1", (rva,)
            ).fetchone()
            if row and row[1] > rva:
                root = row[2]
                rows = self.idx.db.execute(
                    "SELECT rva, role, kind, class_hash, winner FROM role WHERE rva = ?", (root,)
                ).fetchall()
        rows = [r for r in rows if r[3] is not None]
        if not rows:
            return None, "%#x: not a slot function or object of any STU class (idx.addr: %s)" % (
                rva, self.idx.addr(rva).get("what"))
        users = sorted({r[3] for r in rows})
        owner = self.common_ancestor(users)
        roles = sorted({"%s%s" % (r[1], " (%s)" % r[2] if r[2] else "") for r in rows})
        info = {"via": "address", "rva": root if root is not None else rva, "roles": roles,
                "users": [hx(u) for u in users]}
        if len(users) > 1:
            info["shared"] = True
            if owner is None or owner not in users:
                info["note"] = ("shared by %d classes; common ancestor %s" % (len(users), self.label(owner))
                                if owner is not None else "shared by %d unrelated classes" % len(users))
        return (owner if owner is not None else users[0]), info

    def common_ancestor(self, hashes):
        hashes = list(hashes)
        if not hashes:
            return None
        chains = [self.idx.ancestors(h) for h in hashes]
        for a in chains[0]:
            if all(a in c for c in chains[1:]):
                return a
        return None

    # ---------------------------------------------------------------- fields
    def fields(self, h):
        """Fields of h and its ancestors: [field dict + "owner" (hash str), "size"] sorted by offset."""
        if h not in self._fields:
            out = []
            for a in self.idx.ancestors(h):
                st = self.idx.stu(a)
                for f in st["fields"]:
                    out.append(dict(f, owner=st["hash"], size=self.field_size(f)))
            out.sort(key=lambda f: (f["offset"], f["owner"] != hx(h)))
            self._fields[h] = out
        return self._fields[h]

    def field_size(self, f):
        t = f["type"] or {}
        if t.get("array"):
            return 16  # array header (pointer + count); elements live elsewhere
        k = t.get("kind")
        if k == "primitive":
            return PRIM_SIZE.get(t.get("name"))
        if k == "stu" and t.get("mode") == "inline" and t.get("hash"):
            return self._sizes.get(int(t["hash"], 16))
        if k in ("stu", "asset_ref"):
            return 8
        return None

    def type_text(self, f):
        t = f["type"] or {}
        k = t.get("kind", "?")
        desc = t.get("name") or (("STU_" if k in ("stu", "hashmap") else "") + t["hash"] if t.get("hash") else "")
        s = "%s%s %s" % (k + ("/" + t["mode"] if t.get("mode") else ""), "[]" if t.get("array") else "", desc)
        if t.get("label"):
            s += " (%s)" % t["label"]
        return s.strip()

    def field_at(self, h, off):
        """[(class hash, field, offset within the field)] path for an access at h+off (descends inline STUs)."""
        best = None
        for f in self.fields(h):
            size = f["size"] or 1
            if f["offset"] <= off < f["offset"] + size:
                best = f
                if f["offset"] == off:
                    break
        if best is None:
            return []
        rest = off - best["offset"]
        t = best["type"] or {}
        if rest and t.get("kind") == "stu" and t.get("mode") == "inline" and not t.get("array") and t.get("hash"):
            inner = self.field_at(int(t["hash"], 16), rest)
            if inner:
                return [(h, best, 0)] + inner
        return [(h, best, rest)]

    def field_name(self, f):
        return "m_%s" % f["hash"] + (" (%s)" % f["name"] if f.get("name_source") == "owlib" else "")

    def find_field(self, h, key):
        """Field of h (or an ancestor) by hash, m_<H>, OWLib name or +offset."""
        s = str(key).strip()
        if s.startswith("+"):
            off = int(s[1:], 16)  # offsets are hex, with or without 0x
            path = self.field_at(h, off)
            return path[-1][1] if path else None
        t = s[2:] if s.startswith("m_") else s
        for f in self.fields(h):
            if (len(t) == 8 and all(c in HEX for c in t) and f["hash"] == t.upper()) or f.get("name") == s:
                return f
        return None

    def containers(self, h, limit=40):
        """Classes with a non-array STU field of type h: [(class hash, field)] (at most `limit`), total count."""
        rows = self.idx.db.execute(
            "SELECT class_hash, hash, offset, type FROM field WHERE type LIKE ?", ('%"hash": "' + hx(h) + '"%',)
        ).fetchall()
        out = []
        for ch, fh, off, t in rows:
            t = json.loads(t)
            if t.get("kind") == "stu" and t.get("hash") == hx(h) and not t.get("array"):
                out.append((ch, {"hash": hx(fh), "offset": off, "type": t}))
        return out[:limit], len(out)

    # ---------------------------------------------------------------- registrations / slots
    def registrations(self, h):
        """(own registrations, effective registration dict or None, effective is inherited)."""
        st = self.idx.stu(hx(h))
        regs = st["registrations"]
        eff = st["effective_registration"]
        if eff is None:
            return regs, None, False
        rec = next(r for r in self.idx.records() if r["index"] == eff["record_index"])
        return regs, dict(rec, via_hash=eff["via_hash"]), not eff["own"]

    def users(self, rva):
        """Classes using the function at rva in any registration role: {hash: {"role (kind)", ...}}."""
        out = {}
        for role, kind, ch in self.idx.db.execute(
            "SELECT role, kind, class_hash FROM role WHERE rva = ? AND class_hash IS NOT NULL", (rva,)
        ):
            if role in FUNC_ROLES:
                out.setdefault(ch, set()).add("%s (%s)" % (role, kind) if kind else role)
        return out

    def attribution(self, h, rva):
        """How a slot function relates to class h:
        own        only h uses it (or h and its subclasses through inherited registrations);
        shared     other classes use it too; `common` is their closest common ancestor."""
        users = self.users(rva)
        others = [u for u in users if u != h and h not in self.idx.ancestors(u)]
        res = {"users": len(users)}
        if not others:
            res["status"] = "own"
            sub = len(users) - (1 if h in users else 0)
            if sub:
                res["subclasses"] = sub
        else:
            res["status"] = "shared"
            res["common"] = self.common_ancestor(list(users))
        return res
