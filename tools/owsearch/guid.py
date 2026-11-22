"""Overwatch asset GUIDs the way Lucy shows them (`crates/casc/src/lib.rs` `guid_type`/`guid_name`).

A GUID is 64 bits: the asset type sits bit-reversed, minus one, at bits 48..59; the low 48 bits are the
index. Lucy writes `<index:012X>.<type:03X>` (`000000000307.025`) and, in docs and graphs, the short
form `0307.025` (index with at least 4 digits), or just `0307` when the type is clear from context. The
research notes also write `<type>:<index>` (`097:0001`).
"""

import re

INDEX_MASK = 0xFFFF_FFFF_FFFF

# Type names: the GUID type constants of Lucy's crates (crates/assets, crates/script, crates/casc).
TYPE_NAMES = {
    0x002: "map",
    0x003: "entity definition",
    0x015: "category",
    0x01B: "graph",
    0x01C: "identifier",
    0x025: "game message",
    0x026: "parameter",
    0x053: "aura",
    0x054: "generic settings",
    0x062: "stat",
    0x06C: "target tag",
    0x075: "hero",
    0x07C: "string",
    0x097: "ray profile",
    0x09C: "bundle",
    0x09F: "map header",
    0x0AE: "profile",
    0x0BC: "map chunk",
    0x0C0: "ruleset",
    0x0C5: "game mode",
    0x0C6: "schema",
    0x130: "tunable",
}


def _rev12(x):
    return int(f"{x & 0xFFF:012b}"[::-1], 2)


def guid_type(guid):
    """Asset type (0x001..0x1000) of a GUID."""
    return _rev12(guid >> 48) + 1


def make(index, kind):
    """GUID with this index and asset type."""
    return (_rev12(kind - 1) << 48) | (index & INDEX_MASK)


def is_guid(value):
    """A 64-bit value shaped like a GUID: type bits set, a non-zero index below 2^32, top nibble clear."""
    return 0 <= value < 1 << 60 and (value >> 48) & 0xFFF != 0 and 0 < value & INDEX_MASK < 1 << 32


def name(guid):
    """Lucy's full name, `000000000307.025`."""
    return f"{guid & INDEX_MASK:012X}.{guid_type(guid):03X}"


def short(guid):
    """The short form, `0307.025`."""
    return f"{guid & INDEX_MASK:04X}.{guid_type(guid):03X}"


def describe(guid):
    """{'value', 'type', 'type_name', 'index', 'name', 'short', 'extra_bits'} of a 64-bit value."""
    kind = guid_type(guid)
    return {
        "value": f"0x{guid:016X}", "type": f"{kind:03X}", "type_name": TYPE_NAMES.get(kind),
        "index": guid & INDEX_MASK, "name": name(guid), "short": short(guid),
        "extra_bits": f"0x{guid >> 60:X}" if guid >> 60 else None,
    }  # fmt: skip


_SHORT = re.compile(r"^(?:([0-9A-Fa-f]{1,12})\.([0-9A-Fa-f]{3})|([0-9A-Fa-f]{3}):([0-9A-Fa-f]{1,12}))$")


def parse_short(text):
    """'068A.025', '000000000307.025' or '097:0001' -> (index, type); else None."""
    m = _SHORT.match(text.strip())
    if not m:
        return None
    if m.group(1):
        return int(m.group(1), 16), int(m.group(2), 16)
    return int(m.group(4), 16), int(m.group(3), 16)
