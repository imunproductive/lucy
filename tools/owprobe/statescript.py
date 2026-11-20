"""The client's statescript instances, read live through the probe (ClientProbe.md "Statescript").

Component 47 (vtable 0x7FF78B521F68) is an entity's statescript component; its host (Comp47ScriptHost_Vtbl
0x7FF78B53C010) is at +0xD0. The host keeps its instances in an array (+0x8 pointer, +0x10 count; the creation
0x7FF789887F20 appends) and by id ({instance, u16 id} at +0x38, count +0x40). An instance (920 bytes, vtable
0x7FF78B53E740, constructors 0x7FF7898AAE60 -> 0x7FF789C235D0 -> 0x7FF78AAA0800): +0x10 host, +0x18 u16 id,
+0x98 variable store, +0xA0 its states by graph state index (+0xA8 count; a state is made on its first use,
0x7FF78AAA1460), +0x1E8 graph GUID, +0x318 flags (8: made by the network, 0x400, 0x800). A state: +0x40 flags
(bit 0 while it runs), +0x70 its index, +0x78 its instance. The variable store [+0x98]: 16 buckets {entries, u32
count} of 32 bytes from +0x20 by id & 0xF, 16-byte entries {u16 id, variable}, a variable's value at +0x50 (16
bytes, its type at +0xC; crates/probe `layout::script_value`).

The frame sampler records chosen graphs every frame (`trace --script 0D06`); `name_scripts` names what it recorded:
each state by its class (from its vtable, through the index) and each variable by statescript_names.json.
"""
import json
import struct
from functools import lru_cache
from pathlib import Path

from .client import Probe

COMP47_VTABLE = 0x7FF78B521F68
HOST_IN_COMP47 = 0xD0
INSTANCE_ID, INSTANCE_GRAPH, INSTANCE_FLAGS, INSTANCE_HOST = 0x18, 0x1E8, 0x318, 0x10
INSTANCE_STATES = 0xA0
NAMES_JSON = Path(__file__).resolve().parents[2] / "crates" / "fixed" / "data" / "statescript_names.json"


def graph_id(guid):
    """A statescript graph GUID as ORPHEUS writes its id: the low 16 bits in hex (0x...0254 -> '0254')."""
    return f"{guid & 0xFFFF:04X}"


def hosts(p: Probe, entity_id=None):
    """[{component, entity, id, host}] of component 47s (of one entity id)."""
    out = []
    for c in p.components(COMP47_VTABLE):
        if entity_id is None or c["id"] == entity_id:
            out.append(dict(c, host=c["component"] + HOST_IN_COMP47))
    return out


def instances(p: Probe, host):
    """[{address, id, graph, guid, flags}] of a host's instances."""
    array, count = p.read_u64(host + 8), p.read_u32(host + 0x10)
    if not array or not 0 <= count < 4096:
        return []
    out = []
    for address in (p.read_u64(array, count) if count > 1 else [p.read_u64(array)] * count):
        head = p.read(address, INSTANCE_FLAGS + 4, partial=True)
        if len(head) < INSTANCE_FLAGS + 4:
            continue
        ident = int.from_bytes(head[INSTANCE_ID:INSTANCE_ID + 2], "little")
        guid = int.from_bytes(head[INSTANCE_GRAPH:INSTANCE_GRAPH + 8], "little")
        flags = int.from_bytes(head[INSTANCE_FLAGS:INSTANCE_FLAGS + 4], "little")
        out.append({"address": address, "id": ident, "graph": graph_id(guid), "guid": guid, "flags": flags,
                    "host": int.from_bytes(head[INSTANCE_HOST:INSTANCE_HOST + 8], "little")})
    return out


STATE_FLAGS, STATE_NODE, STATE_INSTANCE = 0x40, 0x70, 0x78


def states(p: Probe, instance):
    """The states an instance has made so far, by graph state index: [{index, address, vtable, flags}]."""
    array, count = struct.unpack("<QI", p.read(instance + INSTANCE_STATES, 12))
    if not array or not 0 < count < 4096:
        return []
    out = []
    for index, state in enumerate(p.read_u64(array, count) if count > 1 else [p.read_u64(array)]):
        if not state:
            continue
        head = p.read(state, STATE_FLAGS + 4, partial=True)
        if len(head) < STATE_FLAGS + 4:
            continue
        vtable = int.from_bytes(head[:8], "little")
        out.append({"index": index, "address": state, "vtable": vtable, "orpheus_vtable": p.orpheus(vtable),
                    "flags": int.from_bytes(head[STATE_FLAGS:STATE_FLAGS + 4], "little")})
    return out


VARIABLE_STORE, VARIABLE_BUCKETS, BUCKET_BYTES, VARIABLE_VALUE = 0x98, 0x20, 32, 0x50


def script_value(raw):
    """A variable's 16-byte value as the probe's sampler gives it: bool, int, float, [x, y, z], {"entity": id},
    None (type 0xFF), else {"type", "hex"}."""
    kind = int.from_bytes(raw[0xC:0x10], "little")
    if kind == 1:
        return raw[0] != 0
    if kind == 2:
        return struct.unpack_from("<i", raw)[0]
    if kind == 3:
        return struct.unpack_from("<f", raw)[0]
    if kind == 6:
        return list(struct.unpack_from("<3f", raw))
    if kind == 0xA:
        return {"entity": int.from_bytes(raw[:4], "little")}
    if kind == 0xFF:
        return None
    return {"type": kind, "hex": raw[:12].hex()}


def variables(p: Probe, instance, wanted=None):
    """{id: value} of an instance's variables: all, or those whose ids are in `wanted`."""
    store = p.read_u64(instance + VARIABLE_STORE)
    out = {}
    if not store:
        return out
    buckets = sorted({v & 0xF for v in wanted}) if wanted is not None else range(16)
    for bucket in buckets:
        entries, count = struct.unpack("<QI", p.read(store + VARIABLE_BUCKETS + BUCKET_BYTES * bucket, 12))
        if not entries or not 0 < count <= 4096:
            continue
        table = p.read(entries, 16 * count)
        for at in range(0, len(table), 16):
            ident = int.from_bytes(table[at:at + 2], "little")
            variable = int.from_bytes(table[at + 8:at + 16], "little")
            if variable and (wanted is None or ident in wanted):
                out[ident] = script_value(p.read(variable + VARIABLE_VALUE, 16))
    return out


def find_instances(p: Probe, graph, entity_id=None):
    """[{address, id, graph, guid, flags, host, entity}] of the live instances of a graph ('288B'), on every
    entity or one."""
    return [dict(inst, entity=h["id"]) for h in hosts(p, entity_id) for inst in instances(p, h["host"])
            if inst["graph"] == graph]


@lru_cache(maxsize=None)
def _names():
    try:
        data = json.loads(NAMES_JSON.read_text(encoding="utf-8"))
    except OSError:
        return {}, {}
    classes = {int(v["hash"], 16): k for k, v in data.get("classes", {}).items() if v.get("hash")}
    variables = {int(k): v["name"] for k, v in data.get("vars", {}).items() if isinstance(v, dict) and v.get("name")}
    return classes, variables


def state_class(vtable_orpheus):
    """The STU class of a state's runtime vtable (an ORPHEUS VA), through the owindex: its name when known,
    else STU_<hash>; None without the index."""
    try:
        index = _index()
    except Exception:  # noqa: BLE001 - the index is optional here
        return None
    row = index.db.execute("SELECT class_hash FROM role WHERE role = 'runtime_vtable' AND rva = ?",
                           (vtable_orpheus - 0x7FF788F10000,)).fetchone()
    if not row:
        return None
    classes, _ = _names()
    name = classes.get(row[0])
    if name is None:
        stu = index.db.execute("SELECT name FROM stu WHERE hash = ?", (row[0],)).fetchone()
        name = stu[0] if stu and stu[0] else None
    return f"{name or ''} STU_{row[0]:08X}".strip() if name else f"STU_{row[0]:08X}"


@lru_cache(maxsize=1)
def _index():
    from tools.owindex import Index
    return Index.open()


def variable_name(ident):
    return _names()[1].get(int(ident))


def name_scripts(p: Probe, scripts):
    """{(entity, graph, instance): {"states": {index: class}, "vars": {id: name}}} for the script entries a trace
    recorded (frame records' `scripts`), read once from the live instances."""
    out = {}
    wanted = {(s["entity"], s["graph"], s["instance"]) for s in scripts}
    by_entity = {}
    for entity, graph, instance in wanted:
        if entity not in by_entity:
            by_entity[entity] = [i for h in hosts(p, entity) for i in instances(p, h["host"])]
        for inst in by_entity[entity]:
            if inst["graph"] == graph and inst["id"] == instance:
                names = {s["index"]: state_class(s["orpheus_vtable"]) for s in states(p, inst["address"])}
                variables = {v: variable_name(v) for s in scripts if (s["entity"], s["graph"], s["instance"]) ==
                             (entity, graph, instance) for v in s.get("vars", {})}
                out[(entity, graph, instance)] = {"states": names,
                                                  "vars": {k: v for k, v in variables.items() if v}}
    return out
