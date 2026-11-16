"""Disassembly of the debuggee's memory (capstone; our int3 bytes are masked)."""
import capstone

_cs = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
_cs_detail = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
_cs_detail.detail = True


def disasm(dbg, va, count=10, max_bytes=None):
    """[(va, size, text)] from va."""
    data = dbg.read_partial(va, max_bytes or count * 15)
    out = []
    for insn in _cs.disasm(data, va):
        out.append((insn.address, insn.size, f"{insn.mnemonic} {insn.op_str}".strip()))
        if len(out) >= count:
            break
    return out


def _decode_to(dbg, start, target):
    """Linear decode from start; the instruction list if one ends exactly at target."""
    if target - start > 0x4000 or target <= start:
        return None
    data = dbg.read_partial(start, target - start + 15)
    out = []
    for insn in _cs.disasm(data, start):
        if insn.address >= target:
            break
        out.append(insn)
    if out and out[-1].address + out[-1].size == target:
        return out
    return None


def _sync_start(dbg, target):
    """A start address from which linear decoding reaches target: the .pdata function start of
    target's module, else a back-scan."""
    mod = dbg.module_at(target - 1)
    if mod is not None and mod.pe is not None:
        rf = mod.pe.runtime_function(target - 1 - mod.base)
        if rf is not None and target - (mod.base + rf[0]) <= 0x4000:
            return mod.base + rf[0]
    return None


def instructions_before(dbg, va, count):
    """Up to `count` instructions ending exactly at va."""
    start = _sync_start(dbg, va)
    if start is not None:
        insns = _decode_to(dbg, start, va)
        if insns:
            return [(i.address, i.size, f"{i.mnemonic} {i.op_str}".strip()) for i in insns[-count:]]
    # Back-scan: the longest decode chain from va-k that lands on va.
    for k in range(min(count * 6, 90), 0, -1):
        insns = _decode_to(dbg, va - k, va)
        if insns and len(insns) >= min(count, 3):
            return [(i.address, i.size, f"{i.mnemonic} {i.op_str}".strip()) for i in insns[-count:]]
    return []


def disasm_around(dbg, va, before=6, after=8):
    """[(va, size, text, is_current)] around va."""
    rows = [(a, s, t, False) for a, s, t in instructions_before(dbg, va, before)]
    rows += [(a, s, t, a == va) for a, s, t in disasm(dbg, va, after)]
    return rows


_STACK_PUSH = ("push", "call")


def _reg(name, regs, next_ip):
    if name == "rip":
        return next_ip
    if name in regs:
        return regs[name]
    if len(name) == 3 and name[0] == "e" and "r" + name[1:] in regs:  # eax -> rax
        return regs["r" + name[1:]] & 0xFFFFFFFF
    if name.endswith("d") and name[:-1] in regs:  # r8d -> r8
        return regs[name[:-1]] & 0xFFFFFFFF
    return None


def _accesses(full, regs):
    """[(ea, size, kind)] of the memory operands (and implicit stack accesses) of an instruction,
    computed with the registers after it executed (bases it overwrote give wrong addresses)."""
    out = []
    next_ip = full.address + full.size
    for op in full.operands:
        if op.type != capstone.x86.X86_OP_MEM:
            continue
        m = op.mem
        if m.segment and full.reg_name(m.segment) in ("fs", "gs"):
            continue
        ea = m.disp
        if m.base:
            v = _reg(full.reg_name(m.base), regs, next_ip)
            if v is None:
                continue
            ea += v
        if m.index:
            v = _reg(full.reg_name(m.index), regs, next_ip)
            if v is None:
                continue
            ea += v * m.scale
        acc = op.access
        kind = ("r" if acc & capstone.CS_AC_READ else "") + ("w" if acc & capstone.CS_AC_WRITE else "")
        out.append((ea & 0xFFFFFFFFFFFFFFFF, op.size or 8, kind or "?"))
    mn = full.mnemonic
    if mn.startswith(_STACK_PUSH):
        out.append((regs["rsp"], 8, "w"))
    elif mn.startswith("pop"):
        out.append((regs["rsp"] - 8, 8, "r"))
    return out


def _detail(dbg, insn_va, size):
    data = dbg.try_read(insn_va, size)
    return next(_cs_detail.disasm(data, insn_va), None) if data else None


def access_insn(dbg, rip_after, regs=None, watched=()):
    """The instruction that made a data breakpoint fire. Traps report the next instruction, so
    candidates are: the instruction ending at rip_after decoded from the .pdata function start,
    single instructions ending at rip_after (back-scan, for obfuscated code), and a call whose
    return address is at [rsp] (a call traps at its target). With regs (the thread's registers)
    and watched [(va, size)], the first candidate whose memory operand overlaps a watched range
    wins ('verified'). Returns (va, text, kind, verified) with kind 'r', 'w', 'rw' or '?', or None."""
    if regs:
        # A rep string instruction traps between iterations with RIP still on itself.
        cur = _detail(dbg, rip_after, 15)
        if cur is not None and cur.prefix[0] in (0xF2, 0xF3) and cur.mnemonic.split()[-1][:4] in (
                "movs", "stos", "lods", "cmps", "scas"):
            text = f"{cur.mnemonic} {cur.op_str}".strip()
            for reg, kind in (("rdi", "w" if "stos" in text or "movs" in text else "r"), ("rsi", "r")):
                v = regs[reg]
                if any(v - 16 < wva + wsize and wva < v + 16 for wva, wsize in watched):
                    return rip_after, text, kind, True
    cands = []
    start = _sync_start(dbg, rip_after)
    if start is not None:
        insns = _decode_to(dbg, start, rip_after)
        if insns:
            cands.append((insns[-1].address, insns[-1].size))
            if (regs and insns[-1].mnemonic == "call"
                    and any(regs["rsp"] - 8 < wva + wsize and wva < regs["rsp"] for wva, wsize in watched)):
                # rip_after is a return address: a callee's ret read the slot.
                return rip_after, "ret (a callee returned here)", "r", True
    if regs:
        ret = dbg.try_read(regs["rsp"], 8)
        if ret:
            ret = int.from_bytes(ret, "little")
            for n in (7, 6, 5, 3, 2):  # longest first: 41 FF D4 (call r12) also decodes as FF D4
                full = _detail(dbg, ret - n, n)
                if full is not None and full.size == n and full.mnemonic == "call":
                    cands.append((ret - n, n))
                    break
    for k in range(15, 0, -1):
        data = dbg.try_read(rip_after - k, k)
        if data:
            got = list(_cs.disasm(data, rip_after - k))
            if len(got) == 1 and got[0].size == k and (rip_after - k, k) not in cands:
                cands.append((rip_after - k, k))
    first = None
    for va, size in cands:
        full = _detail(dbg, va, size)
        if full is None:
            continue
        accs = _accesses(full, regs) if regs else []
        text = f"{full.mnemonic} {full.op_str}".strip()
        for ea, n, kind in accs:
            if any(ea < wva + wsize and wva < ea + n for wva, wsize in watched):
                return va, text, kind, True
        if first is None:
            first = (va, text, accs[0][2] if accs else "?", False)
    return first
