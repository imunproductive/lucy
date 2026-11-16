"""Network message and entity component tables of the Overwatch 1.74 client (build 104319).

`build(image, base)` is a pure function over a memory-layout image dump (file offset == RVA). It
returns the JAM message table (every message the client knows, its field layout, and the client
dispatcher/handler for each incoming message) and the entity component registry (component id ->
factory, runtime class, constructor, STU classes). Every address in the result is an RVA.

How the tables are found (no hard-coded addresses; the constants below are the shapes):

Messages
- `JamMessageInfo` records in .data: `{name*, protocol*, items*, u32 protocol crc, u32 message id}`,
  where both strings are the image's empty string (the most common .data pointer into .rdata, it
  points at a 0 byte). `items` are 0x30-byte records `{name*, u32 type, u32 offset, u32 size,
  i32 count, inner*, resize*, ...}`, type 17 ends a list, type 14 nests `inner`, `resize != 0`
  marks an array. This decodes to exactly ORPHEUS's game_schemas.json / lobby_schemas.json.
- Protocol classes: vtables whose slot 3 is `mov eax, crc; ret`, slots 5/6 `mov eax, first/end id`.
  The game link's protocol table is built by the function that constructs the most protocol
  objects (wire index = construction order; checked against ORPHEUS's PROTOCOLS).
- Message classes: vtable whose slot 2 is the `lea rax, [info]; ret` stub of the message's info.
- Dispatch: the message id getter is `call [vtbl+0x10]; movzx eax, word [rax+0x1C]` (info id).
  Each call site of it is run through a small switch interpreter (add/sub/cmp/test/jcc,
  jump tables) once per known message id; ids that leave the default path are handled there and
  the case block's first direct call (or tail jump) is the handler. Switches inside protocol
  vtable functions (the per-protocol clone switch) are not client dispatchers and are skipped.

Components
- Component factories are heap objects `{vtable, u8 component id at +8, pool at +0x10}` built by
  constructors `lea rax, [factory vtable]; mov byte [rcx+8], id; mov [rcx], rax` (or the id from
  a `mov byte [rcx], id; mov rax, rcx; ret` stub). The factory vtable shape (slots 0 and 3) is
  learned from those constructors; every .rdata vtable of that shape is a factory vtable and the
  constructor that leas it gives the id. Slot 1 is the create function.
- The create function allocates (the allocator = the most common first call of create
  functions), calls the C++ constructor with rcx = the new object, and stores the most derived
  vtable at [object] (in create or in the constructor): `vtable_rva`, `construct_rva`.
- Component data (`mirror_*`): the first STU descriptor the constructor (BFS, depth 3) passes in
  rcx to the STU instance initialiser (the most common callee after `lea rcx, [descriptor]`).
- `class_hash`: the STU entity component class (asset side) the client maps to the id: the
  function with the most `cmp ecx, <STU hash>` (the definition mapper, a compare tree ending in
  `mov byte [out], id`) run for every hash it compares.
"""

from __future__ import annotations

import bisect
import re
import struct
from collections import Counter

import capstone
import numpy as np
import pefile

# Labels from the ORPHEUS server (this repo, read only) and kjs notes. Names, never
# addresses: everything else comes from the image.
GAME_LINK_PROTOCOL_NAMES = {  # crates/wire/src/messages.rs PROTOCOLS
    0xC77F6403: "ClientInInstance",
    0xA7FBBD0C: "ClientOutInstance",
    0x8F89E0DF: "ClientInCombat",
    0x82A2F991: "ClientInVoice",
    0x9F0A9F88: "ClientInStats",
    0xA9385A52: "ClientOutCheats",
    0x716AD5D9: "ClientOutPvP",
    0x145CE2DA: "ClientOutStats",
    0xCF043764: "ClientOutSpectate",
}

MESSAGE_LABELS = {  # ORPHEUS constants (crate/file in the value)
    20300: "pending game (kjs: 20300..20322 dispatcher)",
    20304: "leave game (server/lobby/matchmaker)",
    20306: "pong (server/ingress.rs)",
    20308: "player card (sim/notices.rs)",
    20400: "chat message (lobby/social)", 20401: "channel members (lobby/social)", 20402: "joined (lobby/social)",
    20404: "left (lobby/social)",
    20503: "disconnect (lobby/session.rs)", 20504: "content keys (lobby/celebrations.rs)",
    20600: "handoff message (lobby/handoff.rs)",
    20700: "party state (lobby/party.rs)", 20701: "group suggestion (lobby/social)", 20703: "merge request (lobby/social)",
    20704: "invite (lobby/social)", 20705: "invite cancelled (lobby/social)",
    20802: "game settings block (+0x120 kill feed flags, kjs)", 20803: "party error (lobby/social)",
    20807: "full profile (lobby/mode_rules.rs)", 20809: "friend cards (lobby/social)",
    21604: "hero settings (content/hero_settings.rs)", 21608: "communication action (sim/wheel.rs)",
    21616: "client game message (content/parameters.rs)",
    21700: "chat (lobby/social)", 21701: "who (lobby/social)",
    21800: "login (lobby/login.rs)", 21801: "leaving game (lobby/lineup.rs)",
    22102: "invite player (lobby/social)", 22103: "join group", 22104: "invite group", 22105: "kick",
    22106: "make leader", 22107: "leave", 22108: "accept invite", 22109: "decline invite", 22110: "merge accept",
    22111: "merge decline",
    22204: "save values (lobby/saved_settings.rs)", 22206: "profile request (lobby/mode_rules.rs)",
    22207: "save UX state (lobby/saved_settings.rs)",
    23310: "team colours (lobby/handoff.rs)",
    24000: "create game (lobby/handoff.rs)", 24201: "open box (lobby/lootbox.rs)", 24203: "purchase (lobby/shop.rs)",
    24305: "box opened (lobby/lootbox.rs)", 24307: "credits (lobby/lootbox.rs)",
    24500: "equip (lobby/gallery.rs)", 24902: "equipped (lobby/gallery.rs)",
    25001: "hit notice {entity, u32 damage, u8 flags} u32 frame (wire/combat_messages.rs)",
    25004: "death notice {u8, entity} (wire/combat_messages.rs)",
    25005: "elimination {u64, entity, entity victim, u8 score, bool assist} (wire/combat_messages.rs)",
    26500: "store page (lobby/lookups.rs)",
    27000: "send friend request (lobby/social)", 27001: "answer request", 27002: "remove friend", 27004: "whisper",
    27011: "set status", 27100: "friends list", 27105: "friend added", 27106: "friend removed",
    27107: "request received", 27108: "request removed", 27110: "request result", 27111: "answered", 27112: "removed",
    27113: "presence update", 27116: "whisper sent", 27117: "whisper received",
    27202: "mode rules (lobby/mode_rules.rs)",
    27500: "voice line {entity speaker, entity, entity, u64 stimulus, u64 line, u32, u64} (wire/combat_messages.rs)",
    36200: "season intro seen (lobby/saved_settings.rs)", 36300: "ranked state (lobby/ranked.rs)",
    36302: "card ratings (lobby/ranked.rs)",
    38900: "celebrations (lobby/celebrations.rs)", 38901: "new celebration", 38902: "challenge progress",
    39000: "leaderboard page (lobby/leaderboard.rs)", 39001: "profile status (lobby/mode_rules.rs)",
    39002: "profile summary (lobby/mode_rules.rs)", 39100: "leaderboard page request",
    39802: "arcade cards (lobby/arcade.rs)", 39804: "arcade busy", 39810: "arcade windows",
    42600: "OWL matches (lobby/lookups.rs)", 43200: "OWL live matches (lobby/lookups.rs)",
    44100: "enter queue (lobby/queue.rs)", 44102: "cancel queue", 44103: "set roles", 44104: "accept", 44105: "decline",
    44106: "pass role", 44107: "change roles", 44201: "queue joined", 44202: "queue left",
    52200: "find groups (lobby/social)", 52201: "list group", 52202: "close group", 52203: "group roles",
    52204: "watch groups", 52205: "unwatch groups", 52300: "groups found", 52301: "group changed", 52302: "group closed",
    52902: "leave match lobby (lobby/lineup.rs)", 52903: "lobby answer (lobby/lineup.rs)",
    53000: "lobby state (lobby/lineup.rs)", 53002: "roster (lobby/lineup.rs)", 53003: "lobby left (lobby/lineup.rs)",
    56200: "wait times (lobby/queue.rs)", 58202: "names (lobby/lookups.rs)", 58301: "names found (lobby/lookups.rs)",
}

COMPONENT_LABELS = {  # (name or None, label, source)
    22: (None, "projectile spawn", "kjs notes"),
    26: (None, "team filter (one id field)", "kjs notes, ORPHEUS wire/entity.rs"),
    29: ("MPlayerComponentData", "player: lobby id, battleTag, level, locale, group, party, platform, ranks, moderator",
         "ORPHEUS wire/entity.rs"),
    35: (None, "pending shot records (+0xB0 list, +0xB8 count)", "kjs notes"),
    44: ("MECPossessorData", "possessor: current, view target, primary possessable", "ORPHEUS wire/entity.rs"),
    50: (None, "ProjectileVisual init", "kjs notes"),
    51: (None, "health: 16 health, 16 armour, 16 shield pools (20 bytes) + u32, u32, entity, u8, u8; "
             "+0x4C0 combat event queue (25000)", "ORPHEUS wire/entity.rs, IDA-questions"),
    59: (None, "local command/aim: view angles, 25001 hit list +0x12D0", "ORPHEUS wire/combat_messages.rs, IDA-questions"),
    64: (None, "Tab board stats", "kjs notes, ORPHEUS wire/entity.rs"),
    74: ("MECGameModeParticipant", "game mode participant: portrait frame, slot, roles, can select a hero",
         "ORPHEUS wire/entity.rs"),
    75: (None, "hero (6 ids: hero, skin, ...)", "ORPHEUS wire/entity.rs, kjs notes"),
    84: (None, "observer target (copied from 85 +0x90 to +0x68)", "kjs notes"),
    85: (None, "observer source", "kjs notes"),
    90: (None, "projectile id reservations (leases)", "ORPHEUS wire/entity.rs, kjs notes"),
    91: (None, "predicted projectile marker", "kjs notes"),
    92: (None, "name override (STU_76AF64F0)", "IDA-questions"),
    114: (None, "game mode (the first component-114 entity is the mode entity)", "ORPHEUS wire/entity.rs, combat_messages.rs"),
    123: ("MECCharacterBody", "character body", "kjs notes"),
}

JAM_TYPES = {0: "bool", 1: "u8", 2: "u8", 3: "i8", 4: "u16", 5: "i16", 6: "u32", 7: "i32", 8: "u64", 9: "i64",
             10: "f32", 11: "f64", 12: "string", 13: "string", 14: "struct", 15: "blob"}

_REGS64 = ("rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15")
_R32 = {"eax": "rax", "ebx": "rbx", "ecx": "rcx", "edx": "rdx", "esi": "rsi", "edi": "rdi", "ebp": "rbp", "esp": "rsp",
        **{f"r{n}d": f"r{n}" for n in range(8, 16)}}
_VOLATILE = {"rax", "rcx", "rdx", "r8", "r9", "r10", "r11"}
_MEM = re.compile(r"(byte|word|dword|qword) ptr \[(\w+)(?: \+ (\w+)(?:\*(\d))?)?(?: ([+-]) (0x[0-9a-f]+|\d+))?\]$")
_STORE = re.compile(r"qword ptr \[(\w+)\], (\w+)$")
_M32 = 0xFFFFFFFF
_M64 = 0xFFFFFFFFFFFFFFFF


def _canon(r):
    if r in _REGS64:
        return r, 64
    if r in _R32:
        return _R32[r], 32
    return None, None


def _rip_target(ins):
    """Target RVA of a `[rip +/- disp]` operand, else None."""
    m = re.search(r"\[rip ([+-]) (0x[0-9a-f]+|\d+)\]", ins.op_str)
    if not m:
        return None
    disp = int(m.group(2), 0)
    return ins.address + ins.size + (disp if m.group(1) == "+" else -disp)


class _Image:
    """Image bytes with the indexes the passes need. All addresses are RVAs (capstone runs at 0)."""

    def __init__(self, image: bytes, base: int):
        self.b = image
        self.base = base
        pe = pefile.PE(data=image, fast_load=True)
        secs = {s.Name.rstrip(b"\0").decode(errors="replace"): (s.VirtualAddress, s.VirtualAddress + s.Misc_VirtualSize)
                for s in pe.sections}
        exc = pe.OPTIONAL_HEADER.DATA_DIRECTORY[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXCEPTION"]]
        pe.close()
        self.text, self.rdata, self.data = secs[".text"], secs[".rdata"], secs[".data"]
        self.q64 = np.frombuffer(image, dtype="<u8", count=len(image) // 8)
        self.cs = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self._dc = {}
        # .pdata
        pd = np.frombuffer(image, dtype="<u4", count=exc.Size // 4, offset=exc.VirtualAddress).reshape(-1, 3)
        pd = pd[pd[:, 0] != 0]
        pd = pd[np.argsort(pd[:, 0], kind="stable")]
        self.pd_begin, self.pd_end, self.pd_unwind = (pd[:, 0].astype(np.int64), pd[:, 1].astype(np.int64),
                                                      pd[:, 2].astype(np.int64))
        self.fstarts = set(int(x) for x in self.pd_begin)
        self._build_ptr_index()
        self._build_call_index()
        self._build_rip_index()

    # --- integers ---
    def q(self, r):
        return struct.unpack_from("<Q", self.b, r)[0]

    def d(self, r):
        return struct.unpack_from("<I", self.b, r)[0]

    def ptr(self, r):
        """RVA a qword at r points to, or None when it is not an image pointer."""
        v = self.q(r) - self.base
        return v if 0 <= v < len(self.b) else None

    def in_rdata(self, r):
        return r is not None and self.rdata[0] <= r < self.rdata[1]

    # --- indexes ---
    def _build_ptr_index(self):
        a = self.q64
        m = (a >= self.base) & (a < self.base + len(self.b))
        loc = np.nonzero(m)[0]
        val = (a[loc] - np.uint64(self.base)).astype(np.int64)
        o = np.argsort(val, kind="stable")
        self.ptr_val, self.ptr_loc = val[o], loc[o].astype(np.int64) * 8

    def refs_to(self, r):
        """Aligned qwords in the image that point at RVA r."""
        lo, hi = np.searchsorted(self.ptr_val, r, "left"), np.searchsorted(self.ptr_val, r, "right")
        return [int(x) for x in self.ptr_loc[lo:hi]]

    def _build_call_index(self):
        t0, t1 = self.text
        tb = np.frombuffer(self.b, dtype=np.uint8)
        pos = np.nonzero(tb[t0:t1 - 5] == 0xE8)[0] + t0
        rel = np.frombuffer(self.b, dtype=np.uint8)  # bytes after E8 as signed 32-bit
        r = (rel[pos + 1].astype(np.int64) | (rel[pos + 2].astype(np.int64) << 8) |
             (rel[pos + 3].astype(np.int64) << 16) | (rel[pos + 4].astype(np.int64) << 24))
        r = np.where(r >= 2 ** 31, r - 2 ** 32, r)
        tg = pos + 5 + r
        ok = (tg >= t0) & (tg < t1)
        src, tg = pos[ok], tg[ok]
        o = np.argsort(tg, kind="stable")
        self.call_src, self.call_tgt = src[o], tg[o]

    def callers(self, r):
        lo, hi = np.searchsorted(self.call_tgt, r, "left"), np.searchsorted(self.call_tgt, r, "right")
        return [int(x) for x in self.call_src[lo:hi]]

    def _build_rip_index(self):
        """RIP-relative lea/mov/cmp/... targets: {target: [instruction RVAs]} (REX.W forms)."""
        t0, t1 = self.text
        idx = {}
        for m in re.finditer(rb"[\x48\x4C][\x8D\x8B\x89\x3B\x39\x83\x81\xC7][\x05\x0D\x15\x1D\x25\x2D\x35\x3D]",
                             self.b[t0:t1]):
            r = m.start() + t0
            idx.setdefault(r + 7 + struct.unpack_from("<i", self.b, r + 3)[0], []).append(r)
        self.ripref = idx

    # --- code ---
    def ins(self, r):
        i = self._dc.get(r)
        if i is None:
            for i in self.cs.disasm(self.b[r:r + 16], r, count=1):
                break
            else:
                i = False
            self._dc[r] = i
        return i or None

    def func_start(self, r):
        """Start of the function containing r (follows chained .pdata unwind info)."""
        k = int(np.searchsorted(self.pd_begin, r, "right")) - 1
        if k < 0 or not (self.pd_begin[k] <= r < self.pd_end[k]):
            return None
        b, u = int(self.pd_begin[k]), int(self.pd_unwind[k])
        for _ in range(16):
            if u & 1 or u + 4 > len(self.b):
                break
            flags, cnt = self.b[u] >> 3, self.b[u + 2]
            if not flags & 4:
                break
            b, _e, u = struct.unpack_from("<III", self.b, u + 4 + 2 * ((cnt + 1) & ~1))
        return b

    def walk(self, entry, limit=800):
        """Recursive descent from entry (jcc both ways, direct jmp followed unless it tail-jumps to
        another .pdata function). Returns (instructions sorted by address, [(jmp RVA, tail target)])."""
        seen, work, tails = {}, [entry], []
        while work and len(seen) < limit:
            a = work.pop()
            while a not in seen and len(seen) < limit:
                i = self.ins(a)
                if i is None:
                    break
                seen[a] = i
                m = i.mnemonic
                if m in ("ret", "int3", "ud2"):
                    break
                if m == "jmp":
                    if i.op_str.startswith("0x"):
                        t = int(i.op_str, 16)
                        if t in self.fstarts and t != entry:
                            tails.append((a, t))
                            break
                        a = t
                        continue
                    break
                if m.startswith("j") and i.op_str.startswith("0x"):
                    work.append(int(i.op_str, 16))
                a += i.size
        return [seen[k] for k in sorted(seen)], tails


# ---------------------------------------------------------------- messages

def _empty_string(img):
    """The image's shared empty string: the most common .data pointer into .rdata pointing at 0."""
    d0, d1 = img.data
    seg = img.q64[d0 // 8:d1 // 8]
    lo, hi = img.base + img.rdata[0], img.base + img.rdata[1]
    vals, counts = np.unique(seg[(seg >= lo) & (seg < hi)], return_counts=True)
    for k in np.argsort(-counts)[:16]:
        r = int(vals[k]) - img.base
        if img.b[r] == 0:
            return r
    raise ValueError("no shared empty string in .data")


def _decode_items(img, r, depth=0):
    out = []
    for k in range(512):
        e = r + 0x30 * k
        if e + 0x30 > len(img.b):
            return None
        ty = img.d(e + 8)
        if ty == 17:
            return out
        if ty > 17 or depth > 24:
            return None
        f = {"type": ty, "type_name": JAM_TYPES.get(ty), "off": img.d(e + 0xC), "size": img.d(e + 0x10),
             "count": struct.unpack_from("<i", img.b, e + 0x14)[0], "array": img.q(e + 0x20) != 0}
        if ty == 14:
            inner = img.ptr(e + 0x18)
            sub = _decode_items(img, inner, depth + 1) if inner is not None else None
            if sub is None:
                return None
            f["fields"] = sub
        out.append(f)
    return None


def _message_infos(img, empty):
    ev = np.uint64(img.base + empty)
    d0, d1 = img.data
    infos = {}
    for k in np.nonzero(img.q64[d0 // 8:d1 // 8] == ev)[0]:
        s = d0 + int(k) * 8
        if img.q(s + 8) != img.base + empty:
            continue
        items = img.ptr(s + 16)
        crc, mid = img.d(s + 0x18), img.d(s + 0x1C)
        if items is None or not crc or mid >= 0x10000:
            continue
        fields = _decode_items(img, items)
        if fields is not None:
            infos[s] = {"crc": crc, "id": mid, "items": items, "fields": fields}
    return infos


def _ret_imm(img, t):
    if t is None or t + 6 > len(img.b):
        return None
    b = img.b[t:t + 6]
    return struct.unpack_from("<I", b, 1)[0] if b[0] == 0xB8 and b[5] == 0xC3 else None


def _protocols(img, crcs):
    """Protocol classes: {crc: {vtable, first, end}}."""
    r0, r1 = img.rdata
    a = img.q64[r0 // 8:r1 // 8]
    t0, t1 = img.text
    out = {}
    for k in np.nonzero((a >= img.base + t0) & (a < img.base + t1))[0]:
        loc = r0 + int(k) * 8
        crc = _ret_imm(img, img.ptr(loc))
        if crc not in crcs:
            continue
        vt = loc - 24
        first, end = _ret_imm(img, img.ptr(vt + 40)), _ret_imm(img, img.ptr(vt + 48))
        if first is not None and end is not None and first <= end:
            out[crc] = {"vtable": vt, "first": first, "end": end}
    return out


def _game_link_order(img, protos):
    """Wire order of the game link: the function constructing the most protocol objects."""
    vt2crc = {p["vtable"]: c for c, p in protos.items()}
    ctor_of = {}
    for c, p in protos.items():
        for site in img.ripref.get(p["vtable"], []):
            fs = img.func_start(site)
            if fs is not None:
                ctor_of.setdefault(fs, c)
    builders = Counter()
    for fs in ctor_of:
        for cl in img.callers(fs):
            g = img.func_start(cl)
            if g is not None:
                builders[g] += 1
    if not builders:
        return None, []
    builder, n = builders.most_common(1)[0]
    if n < 4:
        return None, []
    order = []
    insns, _ = img.walk(builder, 4000)
    for i in insns:
        if i.mnemonic == "call" and i.op_str.startswith("0x"):
            c = ctor_of.get(int(i.op_str, 16))
            if c is None:
                for j in img.walk(int(i.op_str, 16), 60)[0]:
                    t = _rip_target(j) if j.mnemonic == "lea" else None
                    if t in vt2crc:
                        c = vt2crc[t]
                        break
            if c is not None and c not in order:
                order.append(c)
    return builder, order


def _evaljcc(m, fl):
    if fl is None:
        return None
    a, b, res, w, kind = fl
    mask, sb = (1 << w) - 1, 1 << (w - 1)
    res &= mask
    zf, sf = res == 0, bool(res & sb)
    if kind == "sub":
        cf, of = (a & mask) < (b & mask), bool(((a ^ b) & (a ^ res)) & sb)
    else:
        cf = of = False
    return {"je": zf, "jz": zf, "jne": not zf, "jnz": not zf, "ja": not cf and not zf, "jae": not cf, "jb": cf,
            "jbe": cf or zf, "jg": not zf and sf == of, "jge": sf == of, "jl": sf != of, "jle": zf or sf != of,
            "js": sf, "jns": not sf}.get(m)


def _run_switch(img, start, value, maxsteps=64):
    """Concrete run of switch code from `start` with eax = value. Returns (landing RVA or None,
    used a jump table)."""
    regs, fl, pc, table = {"rax": value}, None, start, False
    for _ in range(maxsteps):
        i = img.ins(pc)
        if i is None:
            return None, table
        m = i.mnemonic
        ops = [x.strip() for x in i.op_str.split(",")] if i.op_str else []
        nxt = pc + i.size

        def get(o):
            r, w = _canon(o)
            if r is None:
                try:
                    return int(o, 0)
                except ValueError:
                    return None
            v = regs.get(r)
            return None if v is None else (v & _M32 if w == 32 else v)

        def put(o, v):
            r, w = _canon(o)
            if v is None:
                regs.pop(r, None)
            else:
                regs[r] = v & _M32 if w == 32 else v & _M64

        c0 = _canon(ops[0])[0] if ops else None
        if m in ("add", "sub", "cmp") and len(ops) == 2 and c0:
            a, b, w = get(ops[0]), get(ops[1]), _canon(ops[0])[1]
            if a is None or b is None:
                if m == "cmp" or c0 not in regs:
                    return pc, table
                put(ops[0], None)
                fl, pc = None, nxt
                continue
            mask = (1 << w) - 1
            if m == "add":
                res = (a + b) & mask
                fl = (a, (-b) & mask, res, w, "sub")
                put(ops[0], res)
            else:
                res = (a - b) & mask
                fl = (a, b, res, w, "sub")
                if m == "sub":
                    put(ops[0], res)
            pc = nxt
            continue
        if m in ("dec", "inc") and c0 in regs:
            a, w = get(ops[0]), _canon(ops[0])[1]
            mask = (1 << w) - 1
            res = (a - 1 if m == "dec" else a + 1) & mask
            fl = (a, 1 if m == "dec" else mask, res, w, "sub")
            put(ops[0], res)
            pc = nxt
            continue
        if m == "test" and len(ops) == 2 and c0 in regs:
            a, b = get(ops[0]), get(ops[1])
            if a is None or b is None:
                return pc, table
            fl, pc = (a, b, a & b, _canon(ops[0])[1], "and"), nxt
            continue
        if m in ("mov", "movzx", "movsxd") and len(ops) == 2 and c0 and _canon(ops[1])[0]:
            if _canon(ops[1])[0] not in regs:
                return pc, table
            v = get(ops[1])
            if m == "movsxd" and v & 0x80000000:
                v |= 0xFFFFFFFF00000000
            put(ops[0], v)
            pc = nxt
            continue
        if m == "cdqe" and "rax" in regs:
            v = regs["rax"] & _M32
            regs["rax"] = v | 0xFFFFFFFF00000000 if v & 0x80000000 else v
            pc = nxt
            continue
        if m == "lea" and len(ops) == 2 and "[rip" in ops[1]:
            put(ops[0], _rip_target(i))
            pc = nxt
            continue
        if m in ("mov", "movzx", "movsxd") and len(ops) == 2 and c0 and "ptr [" in ops[1]:
            mm = _MEM.match(ops[1])
            if not mm:
                return pc, table
            sz, b, ix, sc, sg, dp = mm.groups()
            bv = regs.get(_canon(b)[0]) if _canon(b)[0] else None
            if bv is None or b in ("rsp", "rbp"):
                return pc, table
            iv = 0
            if ix:
                if _canon(ix)[0] not in regs:
                    return pc, table
                iv = regs[_canon(ix)[0]] * (int(sc) if sc else 1)
            addr = (bv + iv + ((-1 if sg == "-" else 1) * int(dp, 0) if dp else 0)) & _M64
            n = {"byte": 1, "word": 2, "dword": 4, "qword": 8}[sz]
            if addr + n > len(img.b):
                return None, table
            v = int.from_bytes(img.b[addr:addr + n], "little")
            if m == "movsxd" and v & 0x80000000:
                v |= 0xFFFFFFFF00000000
            put(ops[0], v)
            table = True
            pc = nxt
            continue
        if m == "jmp":
            if ops[0].startswith("0x"):
                pc = int(ops[0], 16)
                continue
            r = _canon(ops[0])[0]
            if r in regs:
                pc = regs[r] & _M64
                continue
            return None, table
        if m.startswith("j") and ops and ops[0].startswith("0x"):
            c = _evaljcc(m, fl)
            if c is None:
                return pc, table
            pc = int(ops[0], 16) if c else nxt
            continue
        return pc, table
    return None, table


def _stub_kind(img, t):
    b = img.b[t:t + 6]
    if b[:3] == b"\x33\xC0\xC3" or b[:3] == b"\x32\xC0\xC3":
        return "no-op stub (returns 0)"
    if b[:1] == b"\xC3" or b[:3] == b"\xC2\x00\x00":
        return "no-op stub (ret)"
    if b[:3] == b"\xB0\x01\xC3" or (b[0] == 0xB8 and b[5] == 0xC3):
        return "stub returning a constant"
    return None


def _case_handler(img, land, fs):
    """First direct call of a case block (or its tail jump out of the dispatcher)."""
    pc = land
    for _ in range(32):
        i = img.ins(pc)
        if i is None:
            return None, "undecodable case"
        m, o = i.mnemonic, i.op_str
        if m == "call":
            return (int(o, 16), "call") if o.startswith("0x") else (None, "indirect call " + o)
        if m in ("ret", "int3"):
            return None, "case returns without a call"
        if m == "jmp":
            if not o.startswith("0x"):
                return None, "indirect jump " + o
            t = int(o, 16)
            if t in img.fstarts or img.func_start(t) != fs:
                return t, "tail jump"
            pc = t
            continue
        pc += i.size
    return None, "no call in case block"


def _getid_functions(img, id_off):
    """Functions `sub rsp,..; mov rax,[rcx]; call [rax+0x10]; movzx eax, word [rax+id_off]; ...; ret`."""
    out = []
    t0, t1 = img.text
    pat = re.compile(rb"\x48\x8B\x01\xFF\x50\x10\x0F\xB7\x40" + bytes([id_off]))
    for m in pat.finditer(img.b, t0, t1):
        fs = img.func_start(m.start())
        if fs is not None and m.start() - fs <= 8:
            out.append(fs)
    return out


def _messages(img):
    empty = _empty_string(img)
    infos = _message_infos(img, empty)
    crcs = {v["crc"] for v in infos.values()}
    protos = _protocols(img, crcs)
    builder, order = _game_link_order(img, protos)
    wire = {c: k for k, c in enumerate(order)}

    # message class vtables: slot 2 = `lea rax, [info]; ret`
    stub_of = {}
    t0, t1 = img.text
    for m in re.finditer(rb"\x48\x8D\x05(....)\xC3", img.b[t0:t1], re.S):
        r = m.start() + t0
        stub_of.setdefault(r + 7 + struct.unpack("<i", m.group(1))[0], r)

    # dispatchers
    proto_slots = set()
    for p in protos.values():
        for k in range(16):
            t = img.ptr(p["vtable"] + 8 * k)
            if t is not None:
                proto_slots.add(t)
    getids = _getid_functions(img, 0x1C)
    all_ids = sorted({v["id"] for v in infos.values()})
    handled = {}  # id -> list of dispatch dicts
    dispatchers = {}
    for g in getids:
        for site in img.callers(g):
            fs = img.func_start(site)
            if fs is None or fs in proto_slots:
                continue
            start = site + 5
            default = {_run_switch(img, start, v)[0] for v in (0, 1, 0xFFFF, 19999)}
            hits = 0
            for mid in all_ids:
                land, table = _run_switch(img, start, mid)
                if land is None or land in default:
                    continue
                hits += 1
                h, how = _case_handler(img, land, fs)
                note = how
                if h is not None:
                    sk = _stub_kind(img, h)
                    if sk:
                        note = f"{how} to {sk}: ignored by this dispatcher"
                handled.setdefault(mid, []).append({
                    "dispatcher_rva": fs, "dispatch_site_rva": site, "case_rva": land, "handler_rva": h,
                    "dispatch_kind": "jump table" if table else "compare", "handler_how": note})
            if hits:
                dispatchers.setdefault(fs, 0)
                dispatchers[fs] += hits

    rows = []
    for s, v in sorted(infos.items(), key=lambda kv: (kv[1]["id"], kv[1]["crc"])):
        crc, mid = v["crc"], v["id"]
        p = protos.get(crc)
        stub = stub_of.get(s)
        cls_vt = None
        if stub is not None:
            for loc in img.refs_to(stub):
                if img.in_rdata(loc - 16):
                    cls_vt = loc - 16
                    break
        common = {
            "channel": wire.get(crc),
            "id": mid,
            "link": "game" if crc in wire else "jam",
            "protocol_crc": crc,
            "protocol_name": GAME_LINK_PROTOCOL_NAMES.get(crc),
            "protocol_first_id": p["first"] if p else None,
            "index": mid - p["first"] if p else None,
            "info_rva": s,
            "items_rva": v["items"],
            "class_vtable_rva": cls_vt,
            "reader_rva": None,
            "fields": v["fields"],
        }
        label = MESSAGE_LABELS.get(mid)
        base_note = []
        if label:
            base_note.append(f"label: {label}")
        if crc in wire:
            base_note.append(f"game link protocol index {wire[crc]}"
                             + (f" ({GAME_LINK_PROTOCOL_NAMES[crc]})" if crc in GAME_LINK_PROTOCOL_NAMES else ""))
        base_note.append("fields decoded by the generic JAM codec from items_rva (see fields)")
        for h in handled.get(mid, []) or [None]:
            row = dict(common)
            if h is None:
                row.update({"handler_rva": None, "dispatcher_rva": None})
                row["notes"] = "; ".join(base_note + ["no client dispatcher switch handles this id (outgoing or "
                                                      "handled outside the id getter)"])
            else:
                row.update(h)
                row["notes"] = "; ".join(base_note + [f"{h['dispatch_kind']} dispatcher, {h['handler_how']}"])
            rows.append(row)

    proto_rows = [{"crc": c, "name": GAME_LINK_PROTOCOL_NAMES.get(c), "vtable_rva": p["vtable"],
                   "first_id": p["first"], "end_id": p["end"], "channel": wire.get(c),
                   "messages": sum(1 for v in infos.values() if v["crc"] == c)} for c, p in sorted(protos.items())]
    stats = {
        "message_infos": len(infos), "protocols_with_class": len(protos), "protocol_crcs": len(crcs),
        "game_link_builder_rva": builder, "game_link_order": [f"{c:08X}" for c in order],
        "id_getters": getids, "dispatchers": len(dispatchers),
        "handled_ids": len(handled),
        "handled_ids_per_dispatcher": {f"{k:#x}": n for k, n in sorted(dispatchers.items())},
    }
    return rows, proto_rows, stats


# ---------------------------------------------------------------- components

def _store_vtables(img, insns, this0, after=None):
    """Linear address-order scan tracking registers that hold `this`. Returns (vtable stores to
    [this] as (rva, vtable), direct calls/jumps made with rcx == this as (rva, target))."""
    this = set(this0) if after is None else set()
    lea, stores, calls = {}, [], []
    for i in insns:
        a, m, o = i.address, i.mnemonic, i.op_str
        if after is not None and a == after:
            this = {"rax"}
            continue
        if m == "lea" and "[rip" in o:
            lea[o.split(",")[0]] = _rip_target(i)
        mm = _STORE.match(o) if m == "mov" else None
        if mm and mm.group(1) in this and img.in_rdata(lea.get(mm.group(2))):
            stores.append((a, lea[mm.group(2)]))
        if m in ("call", "jmp") and o.startswith("0x"):
            t = int(o, 16)
            if "rcx" in this and (m == "call" or t in img.fstarts):
                calls.append((a, t))
            if m == "call":
                this -= _VOLATILE
            continue
        if m == "mov" and "," in o:
            dst, src = [x.strip() for x in o.split(",", 1)]
            if dst in _REGS64:
                (this.add if src in this else this.discard)(dst)
                continue
        if o:
            dst = o.split(",")[0].strip()
            if _canon(dst)[0] and m not in ("cmp", "test", "push"):
                this.discard(_canon(dst)[0])
    return stores, calls


def _id_stub(img, t):
    b = img.b[t:t + 7]
    return b[2] if b[:2] == b"\xC6\x01" and b[3:7] == b"\x48\x8B\xC1\xC3" else None


def _factory_id(img, ctor, site):
    insns = list(img.cs.disasm(img.b[ctor:site + 0x30], ctor))
    last_call = None
    for i in insns:
        if i.mnemonic == "call" and i.op_str.startswith("0x"):
            last_call = int(i.op_str, 16)
        if i.mnemonic == "mov" and i.op_str.startswith("byte ptr [") and "+ 8]" in i.op_str.split(",")[0]:
            v = i.op_str.split(",")[1].strip()
            if v.startswith("0x") or v.isdigit():
                return int(v, 0)
            if last_call is not None:
                return _id_stub(img, last_call)
    return None


def _factory_size(img, ctor):
    """Pool element size: `mov r9d, imm` before the pool registration call (else the first `mov edx, imm`)."""
    edx = None
    for i in img.walk(ctor, 200)[0]:
        if i.mnemonic == "mov" and i.op_str.startswith("r9d, 0x"):
            return int(i.op_str.split(",")[1], 0)
        if edx is None and i.mnemonic == "mov" and i.op_str.startswith("edx, 0x"):
            edx = int(i.op_str.split(",")[1], 0)
    return edx


def _definition_mapper(img, hashes):
    """{STU component hash: component id}: among the functions with >= 20 `cmp ecx, <STU hash>`
    (compare trees over the descriptor hash at +0xB0), the one whose `mov byte [out], id` results
    have the most distinct values (others are set-membership tests returning a bool)."""
    t0, t1 = img.text
    per = Counter()
    for m in re.finditer(rb"\x81\xF9(....)", img.b[t0:t1], re.S):
        if struct.unpack("<I", m.group(1))[0] in hashes:
            fs = img.func_start(m.start() + t0)
            if fs is not None:
                per[fs] += 1
    best = (None, {})
    for fn, n in per.items():
        if n < 20:
            continue
        out = _decode_compare_tree(img, fn)
        if len(set(out.values())) > len(set(best[1].values())):
            best = (fn, out)
    return best


def _decode_compare_tree(img, fn):
    insns, _ = img.walk(fn, 4000)
    by = {i.address: i for i in insns}
    addrs = sorted(by)
    nxt = {a: addrs[k + 1] for k, a in enumerate(addrs[:-1])}
    imms = {int(i.op_str.split(",")[1], 0) for i in insns if i.mnemonic == "cmp" and i.op_str.startswith("ecx, 0x")}
    out = {}
    for h in imms:
        pc, fl = fn, None
        for _ in range(400):
            i = by.get(pc)
            if i is None:
                break
            m, o = i.mnemonic, i.op_str
            if m == "cmp" and o.startswith("ecx, "):
                fl = (h, int(o.split(",")[1], 0))
            elif m == "jmp" or (m.startswith("j") and o.startswith("0x")):
                a, b = fl if fl else (0, 0)
                take = {"je": a == b, "jne": a != b, "ja": a > b, "jb": a < b, "jae": a >= b, "jbe": a <= b,
                        "jmp": True}.get(m)
                if take is None:
                    break
                if take:
                    pc = int(o, 16)
                    continue
            elif m == "mov" and re.match(r"byte ptr \[\w+\], ", o):
                v = o.split(",")[1].strip()
                if v.startswith("0x") or v.isdigit():
                    out[h] = int(v, 0)
                break
            elif m in ("ret", "movzx"):
                break
            pc = nxt.get(pc)
    return out


def _components(img, empty):
    t0, t1 = img.text
    r0, r1 = img.rdata
    # 1. learn the factory vtable shape from the immediate-id constructors
    seed = []
    for m in re.finditer(rb"\x48\x8D\x05(....)\xC6\x41\x08.\x48\x89\x01", img.b[t0:t1], re.S):
        r = m.start() + t0
        seed.append(r + 7 + struct.unpack("<i", m.group(1))[0])
    shape = Counter((img.q(v), img.q(v + 24)) for v in seed if img.in_rdata(v)).most_common(1)
    if not shape:
        return [], {"error": "no component factory constructors"}
    s0, s3 = shape[0][0]
    a = img.q64[r0 // 8:r1 // 8]
    fvts = [r0 + int(k) * 8 for k in np.nonzero(a == np.uint64(s0))[0] if img.q(r0 + int(k) * 8 + 24) == s3]
    # 2. factory constructor and id per factory vtable
    factories = {}  # id -> list of (factory vtable, factory ctor)
    for vt in fvts:
        for site in img.ripref.get(vt, []):
            ctor = img.func_start(site)
            if ctor is None:
                continue
            cid = _factory_id(img, ctor, site)
            if cid is not None:
                factories.setdefault(cid, []).append((vt, ctor))

    # 3. allocator(s): the common first callee of the create functions
    def first_call(fn, depth=0):
        insns, tails = img.walk(fn, 200)
        for i in insns:
            if i.mnemonic == "call" and i.op_str.startswith("0x"):
                return int(i.op_str, 16)
        return first_call(tails[0][1], depth + 1) if tails and depth < 2 else None

    fc = Counter(first_call(img.ptr(vt + 8)) for lst in factories.values() for vt, _ in lst if img.ptr(vt + 8))
    alloc = {t for t, n in fc.items() if t is not None and n >= 3}

    def runtime(create, depth=0):
        insns, tails = img.walk(create)
        site = next((i.address for i in insns if i.mnemonic == "call" and i.op_str.startswith("0x")
                     and int(i.op_str, 16) in alloc), None)
        if site is None:
            return runtime(tails[0][1], depth + 1) if tails and depth < 2 else {}
        stores, calls = _store_vtables(img, insns, set(), after=site)
        calls = [c for c in calls if c[1] not in alloc and c[0] > site]
        ctor = calls[0][1] if calls else None
        vt = stores[-1][1] if stores else None
        if vt is None and ctor is not None:
            s2, _ = _store_vtables(img, img.walk(ctor)[0], {"rcx"})
            vt = s2[-1][1] if s2 else None
        return {"ctor": ctor, "vtable": vt, "alloc_fn": create}

    # 4. STU instance initialiser and descriptors
    ev = img.base + empty
    init_calls = Counter()
    for tgt, sites in img.ripref.items():
        if not (img.data[0] <= tgt < img.data[1]) or img.q(tgt) != ev:
            continue
        for site in sites:
            i = img.ins(site)
            if not i or not i.op_str.startswith("rcx"):
                continue
            pc = site + i.size
            for _ in range(4):
                j = img.ins(pc)
                if j is None:
                    break
                if j.mnemonic == "call":
                    if j.op_str.startswith("0x"):
                        init_calls[int(j.op_str, 16)] += 1
                    break
                pc += j.size
    stu_init = init_calls.most_common(1)[0][0] if init_calls else None

    def mirror(fn):
        seen, frontier = set(), [fn]
        for _depth in range(4):
            nxt = []
            for f in frontier:
                if f is None or f in seen or f in alloc:
                    continue
                seen.add(f)
                insns, tails = img.walk(f, 400)
                last_rcx = None
                for i in insns:
                    if i.mnemonic == "lea" and i.op_str.startswith("rcx, [rip"):
                        last_rcx = _rip_target(i)
                    elif i.mnemonic == "call" and i.op_str.startswith("0x"):
                        t = int(i.op_str, 16)
                        if t == stu_init and last_rcx is not None and img.q(last_rcx) == ev:
                            return last_rcx
                        nxt.append(t)
                        last_rcx = None
                nxt += [t for _, t in tails]
            frontier = nxt
        return None

    # 5. STU entity component class -> id
    d0, d1 = img.data
    descs = [d0 + int(k) * 8 for k in np.nonzero(img.q64[d0 // 8:d1 // 8] == np.uint64(ev))[0]]
    hashes = {img.d(r + 0xB0) for r in descs if r + 0xB4 <= len(img.b)}
    mapper, h2id = _definition_mapper(img, hashes)
    id2h = {}
    for h, cid in h2id.items():
        id2h.setdefault(cid, []).append(h)

    rows = []
    for cid in sorted(set(factories) | set(id2h)):
        lst = factories.get(cid, [])
        fvt, fctor = lst[0] if lst else (None, None)
        create = img.ptr(fvt + 8) if fvt is not None else None
        rt = runtime(create) if create is not None else {}
        md = mirror(rt.get("ctor") or rt.get("alloc_fn")) if rt else None
        name, label, src = COMPONENT_LABELS.get(cid, (None, None, None))
        notes = []
        if not lst:
            notes.append("no factory found: id only known from the STU definition mapper")
        if len(lst) > 1:
            notes.append("several factories: " + ", ".join(f"{v:#x}" for v, _ in lst))
        if rt and rt.get("ctor") is None:
            notes.append("constructor inlined in the create function")
        if lst and not rt.get("vtable"):
            notes.append("runtime vtable not resolved")
        shared = [c for c, l in factories.items() if c != cid and l and fvt is not None and l[0][0] == fvt]
        if shared:
            notes.append("factory class shared with ids " + ", ".join(map(str, sorted(shared))))
        if label:
            notes.append(f"label: {label} ({src})")
        hs = sorted(id2h.get(cid, []))
        if len(hs) > 1:
            notes.append("several STU classes map to this id: " + ", ".join(f"{h:08X}" for h in hs))
        rows.append({
            "id": cid,
            "name": name,
            "label": label,
            "class_hash": hs[0] if len(hs) == 1 else None,
            "class_hashes": hs,
            "vtable_rva": rt.get("vtable"),
            "construct_rva": rt.get("ctor") or rt.get("alloc_fn"),
            "create_rva": create,
            "factory_vtable_rva": fvt,
            "factory_ctor_rva": fctor,
            "size": _factory_size(img, fctor) if fctor is not None else None,
            "mirror_descriptor_rva": md,
            "mirror_hash": img.d(md + 0xB0) if md is not None else None,
            "mirror_field_count": img.d(md + 0x90) if md is not None else None,
            "notes": "; ".join(notes) if notes else None,
        })
    stats = {"factory_vtable_shape": [s0 - img.base, s3 - img.base], "factory_vtables": len(fvts),
             "factories": sum(len(v) for v in factories.values()), "allocators": sorted(alloc),
             "stu_instance_init_rva": stu_init, "definition_mapper_rva": mapper, "mapped_stu_classes": len(h2id)}
    return rows, stats


def build(image: bytes, base: int) -> dict:
    img = _Image(image, base)
    empty = _empty_string(img)
    messages, protocols, mstats = _messages(img)
    components, cstats = _components(img, empty)
    method = (
        "messages: JamMessageInfo records in .data {empty-string name*, empty-string protocol*, items*, u32 crc, "
        "u32 id}, items decoded to `fields` (matches ORPHEUS game/lobby schemas); protocol classes = vtables "
        "with slot3 `mov eax, crc; ret` and slots 5/6 first/end id; game-link `channel` = construction order in "
        "the function building the most protocol objects; dispatch = every call of the id getter "
        "(call [vtbl+0x10]; movzx eax, word [rax+0x1C]) outside protocol vtables, run through a switch "
        "interpreter for every known id, handler = first direct call/tail jump of the case block; "
        "reader_rva is None: JAM fields are decoded generically from items. "
        "components: factory objects {vtable, u8 id at +8} found from their constructors (factory vtable shape "
        "learned from `lea rax,[vt]; mov byte [rcx+8], id; mov [rcx], rax`), create = factory slot 1, "
        "construct = callee of create with rcx = the new object, vtable = last vtable stored at [object]; "
        "mirror_* = first STU descriptor passed to the STU instance initialiser from the constructor; "
        "class_hash = STU entity component class mapped to the id by the definition mapper (compare tree). "
        f"stats: {mstats} {cstats}"
    )
    return {"messages": messages, "components": components, "protocols": protocols, "method": method}
