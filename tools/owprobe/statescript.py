"""The client's statescript instances, read live through the probe (ClientProbe.md "Statescript").

Component 47 (vtable 0x7FF78B521F68) is an entity's statescript component; its host (Comp47ScriptHost_Vtbl
0x7FF78B53C010) is at +0xD0. The host keeps its instances in an array (+0x8 pointer, +0x10 count; the creation
0x7FF789887F20 appends) and by id ({instance, u16 id} at +0x38, count +0x40). An instance (920 bytes, vtable
0x7FF78B53E740, constructors 0x7FF7898AAE60 -> 0x7FF789C235D0 -> 0x7FF78AAA0800): +0x10 host, +0x18 u16 id,
+0x1E8 graph GUID, +0x98 variable store, +0x318 flags (8: made by the network, 0x400, 0x800).
"""
from .client import Probe

COMP47_VTABLE = 0x7FF78B521F68
HOST_IN_COMP47 = 0xD0
INSTANCE_ID, INSTANCE_GRAPH, INSTANCE_FLAGS, INSTANCE_HOST = 0x18, 0x1E8, 0x318, 0x10


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


def states(p: Probe, instance, image=None):
    """The state objects of an instance: heap objects whose +0x78 is the instance and whose first qword is a
    vtable in the client image (a heap scan of a few seconds). [{address, vtable, node, flags}]."""
    info = p.info(fresh=False)
    low, high = image or (info["base"], info["base"] + info["image_size"])
    out = []
    for at in p.scan(value=instance, max=4096):
        state = at - STATE_INSTANCE
        head = p.read(state, STATE_INSTANCE, partial=True)
        if len(head) < STATE_INSTANCE:
            continue
        vtable = int.from_bytes(head[:8], "little")
        if not low <= vtable < high:
            continue
        out.append({"address": state, "vtable": vtable, "orpheus_vtable": p.orpheus(vtable),
                    "node": int.from_bytes(head[STATE_NODE:STATE_NODE + 4], "little", signed=True),
                    "flags": int.from_bytes(head[STATE_FLAGS:STATE_FLAGS + 4], "little")})
    return sorted(out, key=lambda s: s["node"])