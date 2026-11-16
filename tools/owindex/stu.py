"""STU reflection tables of the image: descriptors (classes), their fields and field types, node classes.

Layout (build 104319; found on the 0x7FF788F10000 dump, see build.py's notes):
- descriptor: +0 TYPE_VT, +8 node typeinfo (0 for classes without a C++ node class), +0x10 first child,
  +0x18 next sibling, +0x20 parent descriptor (a tree; the typeinfo parent at typeinfo+8 agrees),
  +0x28 field array, +0x58.. +0x78 class functions, +0x90 field count, +0x98 size dword / align dword,
  +0xB0 hash.
- field entry (56 bytes): +0 TYPE_VT, +8 offset, +0x10 type object, +0x18 flags, +0x20 flags2, +0x30 hash.
  Field entries also start with TYPE_VT, so "objects starting with TYPE_VT" over-counts descriptors ~6x.
- type object: its vtable+0x30 returns (kind << 32) | id; the getter's code says where id comes from.
"""

import struct
import zlib

TYPE_VT_RVA = 0x25E1640  # 0x7FF78B4F1640 at base 0x7FF788F10000
FIELD_SIZE = 56
DESC_SIZE = 0xB8

# Primitive ids are CRC32s of the lower-case type names (u8 = crc32("u8") = 0x3B9327D2, ...); all 23 ids the
# image's fields use are in this list (checked by the builder: an unknown id is reported, never guessed).
PRIMITIVE_NAMES = [
    "u8", "s8", "u16", "s16", "u32", "s32", "u64", "s64", "f32", "f64", "teString", "teVec2", "teVec3",
    "teVec3A", "teVec4", "teQuat", "teColorRGB", "teColorRGBA", "teUUID", "teEntityID", "teMtx43A", "DBID",
    "teStructuredDataDateAndTime",
]  # fmt: skip
PRIMITIVES = {zlib.crc32(n.lower().encode()): n for n in PRIMITIVE_NAMES}

# Type-object kinds (the getter's high dword). Meaning read off the image's fields against OWLib's field types
# (types.txt): 0 primitive, 1 primitive[], 2 STU embed, 3 STU embed[], 4 STU inline, 5 STU inline[],
# 7 teStructuredDataHashMap<STU>, 8 enum, 9 enum[], 10/12 teStructuredDataAssetRef<..>, 11/13 asset ref [].
KINDS = {
    0: ("primitive", False, None),
    1: ("primitive", True, None),
    2: ("stu", False, "embed"),
    3: ("stu", True, "embed"),
    4: ("stu", False, "inline"),
    5: ("stu", True, "inline"),
    7: ("hashmap", False, None),
    8: ("enum", False, None),
    9: ("enum", True, None),
    10: ("asset_ref", False, None),
    11: ("asset_ref", True, None),
    12: ("asset_ref", False, None),
    13: ("asset_ref", True, None),
}


def _getter_program(img, fn):
    """Decode a type object's vt+0x30 getter into (kind, path) where path reads the id from the type object:
    [("field", off)] dword at t+off, [("ptr", off), ("field", off2)] dword at [t+off]+off2, [("global", va)].
    path None: the getter does more (calls); kind None: not a getter of this shape."""
    kind, path, ptr = 0, [], None
    for ins in img.cs.disasm(img.data[fn - img.base : fn - img.base + 64], fn):
        m, op = ins.mnemonic, ins.op_str
        if m == "ret":
            return kind, path or None
        if m == "movabs" and op.startswith("rcx, "):
            kind = int(op.split(", ")[1], 16) >> 32
        elif m == "mov" and op.startswith("rax, qword ptr [rcx + "):
            ptr = int(op.split("+ ")[1].rstrip("]"), 16)
        elif m == "mov" and op.startswith("eax, dword ptr [rax + ") and ptr is not None:
            path = [("ptr", ptr), ("field", int(op.split("+ ")[1].rstrip("]"), 16))]
        elif m == "mov" and op.startswith("eax, dword ptr [rcx + "):
            path = [("field", int(op.split("+ ")[1].rstrip("]"), 16))]
        elif m == "mov" and op.startswith("eax, dword ptr [rip + "):
            path = [("global", ins.address + ins.size + int(op.split("rip + ")[1].rstrip("]"), 16))]
        elif m == "or" and op == "rax, rcx":
            pass
        else:
            break
    # not a plain getter: find at least the kind constant
    for ins in img.cs.disasm(img.data[fn - img.base : fn - img.base + 96], fn):
        if ins.mnemonic == "movabs" and ins.op_str.startswith("rcx, "):
            return int(ins.op_str.split(", ")[1], 16) >> 32, None
        if ins.mnemonic == "ret":
            break
    return None, None


class StuTables:
    def __init__(self, img):
        self.img = img
        self.type_vt = img.base + TYPE_VT_RVA
        self._getters = {}
        self.problems = []
        self._find_descriptors()
        self._find_node_vtables()

    # --- type objects ---
    def type_of(self, t):
        """Field type object -> dict (kind, name/hash, array, mode, code, type_rva)."""
        img = self.img
        if not img.inimg(t) or not img.inimg(img.q(t) + 0x30):
            return {"kind": "unknown", "code": None, "type_rva": None}
        g = img.q(img.q(t) + 0x30)
        if g not in self._getters:
            self._getters[g] = _getter_program(img, g) if img.intext(g) else (None, None)
        code, path = self._getters[g]
        out = {"kind": "unknown", "code": code, "type_rva": t - img.base}
        if code is None or code not in KINDS:
            return out
        kind, array, mode = KINDS[code]
        ident, desc = None, None
        if path is not None:
            ea = t
            for step, off in path:
                if step == "ptr":
                    ea = img.qs(ea + off)
                    if code in (2, 3, 4, 5):
                        desc = ea
                    if not ea:
                        break
                elif step == "field":
                    ident = img.d(ea + off) if img.inimg(ea + off, 4) else None
                else:
                    ident = img.d(off)
        out.update({"kind": kind, "array": array})
        if mode:
            out["mode"] = mode
        if kind == "primitive":
            out["name"] = PRIMITIVES.get(ident)
            out["id"] = ident
            if out["name"] is None:
                out["kind"] = "unknown"
        elif ident is not None:
            out["hash"] = f"{ident:08X}"
        if kind == "stu" and desc is not None and desc not in self.desc_set:
            out["note"] = "descriptor not in the class table"
        return out

    # --- descriptors ---
    def fields_of(self, x):
        img = self.img
        n, fa = img.d(x + 0x90), img.qs(x + 0x28)
        if not n or not img.inimg(fa) or n > 5000:
            return []
        return [fa + FIELD_SIZE * i for i in range(n)]

    def _find_descriptors(self):
        img, vt = self.img, self.type_vt
        pat = struct.pack("<Q", vt)
        objs, pos = [], img.data.find(pat)
        while pos >= 0:
            if pos % 8 == 0:
                objs.append(img.base + pos)
            pos = img.data.find(pat, pos + 1)
        self.type_vt_objects = len(objs)

        def is_ti(t):
            return img.inimg(t) and img.q(t) == t

        # 1. seeds: objects whose +8 is a node typeinfo (first qword points to itself); 2. close over the tree
        # links and the STU-kind field types
        seen, todo = set(), [x for x in objs if is_ti(img.q(x + 8))]
        self.desc_set = set()
        while todo:
            x = todo.pop()
            if x in seen or not img.inimg(x) or img.q(x) != vt:
                continue
            seen.add(x)
            for o in (0x10, 0x18, 0x20):
                v = img.q(x + o)
                if v:
                    todo.append(v)
            for e in self.fields_of(x):
                t = img.qs(e + 16)
                if img.inimg(t) and img.inimg(img.q(t) + 0x30):
                    g = img.q(img.q(t) + 0x30)
                    if g not in self._getters:
                        self._getters[g] = _getter_program(img, g) if img.intext(g) else (None, None)
                    if self._getters[g][0] in (2, 3, 4, 5):
                        todo.append(img.qs(t + (0x30 if self._getters[g][0] == 5 else 0x28)))
        tree = seen
        field_entries = set()
        for x in tree:
            fs = self.fields_of(x)
            field_entries.update(fs)
            if fs:
                field_entries.add(fs[-1] + FIELD_SIZE)  # terminator entry

        # 3. isolated classes (no typeinfo, no tree links, not used as a field type): structural test
        def ptr0(v):
            return v == 0 or img.inimg(v)

        def structural(x):
            if x in field_entries or x in tree:
                return False
            if not all(ptr0(img.q(x + o)) for o in (8, 0x10, 0x18, 0x20, 0x28)):
                return False
            if img.q(x + 0x30) or not img.intext(img.q(x + 0x58)) or not img.d(x + 0xB0) or img.d(x + 0x90) > 2000:
                return False
            fs = self.fields_of(x)
            if img.d(x + 0x90) and not fs:
                return False
            for e in fs:
                t = img.qs(e + 16)
                if img.q(e) != vt or not img.inimg(t) or not img.inimg(img.q(t) + 0x30):
                    return False
            return True

        isolated = [x for x in objs if structural(x)]
        self.isolated = len(isolated)
        everything = tree | set(isolated)
        # dedupe by hash: keep the initialized descriptor (class functions present); report the others
        by_hash = {}
        for x in sorted(everything):
            h = img.d(x + 0xB0)
            if not h:
                continue
            held = by_hash.get(h)
            if held is None or (not img.intext(img.q(held + 0x58)) and img.intext(img.q(x + 0x58))):
                if held is not None:
                    self.problems.append(f"STU_{h:08X}: duplicate descriptor {held:#x} (uninitialized) dropped")
                by_hash[h] = x
            elif held != x:
                self.problems.append(f"STU_{h:08X}: duplicate descriptor {x:#x} dropped")
        self.by_hash = by_hash
        self.desc_set = everything
        self.desc_to_hash = {x: img.d(x + 0xB0) for x in everything}
        self.ti_to_hash = {}
        for h, x in by_hash.items():
            t = img.q(x + 8)
            if t and is_ti(t):
                self.ti_to_hash.setdefault(t, h)
        # parents: descriptor +0x20; check against the typeinfo parent
        self.parent = {}
        self.parent_mismatch = 0
        for h, x in by_hash.items():
            p = img.q(x + 0x20)
            ph = self.desc_to_hash.get(p) if p else None
            self.parent[h] = ph
            t = img.q(x + 8)
            if t and is_ti(t) and ph is not None:
                pt = img.q(t + 8)
                pd = by_hash.get(ph)
                if pd and img.q(pd + 8) and pt != img.q(pd + 8):
                    self.parent_mismatch += 1

    def classes(self):
        """Per class: dict(hash, descriptor, typeinfo, parent, size, align, fields[])."""
        img = self.img
        out = []
        for h, x in sorted(self.by_hash.items()):
            t = img.q(x + 8)
            fields = []
            for e in self.fields_of(x):
                fields.append(
                    {
                        "hash": img.d(e + 0x30),
                        "offset": img.d(e + 8),
                        "type": self.type_of(img.q(e + 16)),
                        "flags": img.d(e + 0x18),
                        "flags2": img.d(e + 0x20),
                        "entry": e,
                    }
                )
            out.append(
                {
                    "hash": h,
                    "descriptor": x,
                    "typeinfo": t if t and img.inimg(t) and img.q(t) == t else None,
                    "parent": self.parent.get(h),
                    "size": img.d(x + 0x98),
                    "align": img.d(x + 0x9C),
                    "fields": fields,
                }
            )
        return out

    # --- node classes ---
    def _find_node_vtables(self):
        """node typeinfo -> node vtable: vtables whose slot 1 is a stub returning the typeinfo and slot 5 a stub
        returning the class's descriptor (ORPHEUS extract_statescript_codecs.py node_typeinfos)."""
        img = self.img
        stubs = img.text_stubs()
        self.node_vtables = {}
        self.node_vtable_mismatch = 0
        for name in (".rdata", ".data"):
            lo, hi = img.sections[name]
            words = list(struct.iter_unpack("<Q", img.data[lo : (hi - lo) // 8 * 8 + lo]))
            for i in range(len(words) - 6):
                s1 = words[i + 1][0]
                if s1 not in stubs:
                    continue
                s5 = words[i + 5][0]
                if s5 not in stubs:
                    continue
                ti, desc = stubs[s1], stubs[s5]
                if not (img.inimg(desc) and img.q(desc) == self.type_vt and img.inimg(ti) and img.q(ti) == ti):
                    continue
                if ti not in self.node_vtables:
                    self.node_vtables[ti] = img.base + lo + 8 * i
                    if self.desc_to_hash.get(desc) != self.ti_to_hash.get(ti):
                        self.node_vtable_mismatch += 1
