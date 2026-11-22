"""Addresses and names: game RVAs (named through tools.owindex when research/out/index/ow_index.sqlite
exists, else the research repo's docs/analysis/*.json), module!export for system DLLs, address specs
and memory expressions."""
import glob
import json
import os
import re
from pathlib import Path

from tools import paths

from . import win32 as w

INDEX_DB = paths.INDEX
IDA_BASE = 0x140000000
LUCY_BASE = 0x7FF788F10000
KJS_BASE = 0x7FF6D1220000


def open_index():
    """tools.owindex.Index when the database exists, else None."""
    if not INDEX_DB.exists():
        return None
    try:
        from tools.owindex import Index
        return Index.open(INDEX_DB)
    except Exception as e:  # a half-built index must not stop a debugging session
        print(f"[owdbg] index unavailable ({e}); using raw RVAs", flush=True)
        return None


def _docs_names():
    """rva -> name from docs/analysis/*.json (functions and globals, VAs at 0x140000000)."""
    out = {}
    for path in sorted(glob.glob(str(paths.ANALYSIS_DIR / "*.json"))):
        try:
            data = json.load(open(path, encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        for key in ("functions", "globals"):
            table = data.get(key)
            if isinstance(table, dict):
                for va, name in table.items():
                    try:
                        v = int(va, 16)
                    except ValueError:
                        continue
                    if v >= IDA_BASE and isinstance(name, str):
                        out.setdefault(v - IDA_BASE, name)
    return out


def hx(v):
    return None if v is None else f"0x{v:X}"


class Names:
    """Symbolizer for one debugging session (dbg may be None before the process exists)."""

    def __init__(self, dbg=None, game_pe=None, use_index=True):
        self.dbg = dbg
        self.game_pe = game_pe
        self.idx = open_index() if use_index else None
        self._docs = None
        self._docs_rev = None
        self._cache = {}

    # ---- game names
    @property
    def docs(self):
        if self._docs is None:
            self._docs = _docs_names()
            self._docs_rev = {n: r for r, n in self._docs.items()}
        return self._docs

    def game_name(self, rva):
        """Name of the game function/global starting at rva."""
        if self.idx is not None:
            try:
                n = self.idx.name(rva)
            except Exception:
                n = None
            if n:
                return n
        return self.docs.get(rva)

    def lookup(self, name):
        """Game RVA of a name (index, then docs/analysis)."""
        if self.idx is not None and hasattr(self.idx, "lookup"):
            try:
                r = self.idx.lookup(name)
            except Exception:
                r = None
            if r is not None:
                return r
        self.docs
        return self._docs_rev.get(name)

    def is_game(self, mod):
        return mod is not None and self.dbg is not None and mod is self.dbg.main and mod.name.lower().startswith("overwatch")

    # ---- describing addresses
    def describe(self, va):
        """{'va', 'module', 'rva', 'func_rva', 'name', 'text'} (+ 'lucy' for the game).
        text: 'Name+0xOFF' / 'module!export+0xOFF' / 'module+0xRVA' / '0xVA'."""
        d = self._cache.get(va)
        if d is not None:
            return d
        dbg = self.dbg
        mod = dbg.module_at(va) if dbg else None
        if mod is None:
            d = {"va": hx(va), "module": None, "text": hx(va)}
        else:
            rva = va - mod.base
            pe = mod.pe
            func = pe.function_start(rva) if pe else None
            d = {"va": hx(va), "module": mod.name, "rva": hx(rva)}
            if self.is_game(mod):
                d["lucy"] = hx(LUCY_BASE + rva)
                name = self.game_name(rva) if func is None or func != rva else None
                if name:  # a named global/label exactly here
                    d.update(func_rva=hx(func), name=name, text=name)
                elif func is not None:
                    fname = self.game_name(func)
                    d.update(func_rva=hx(func), name=fname)
                    base = fname or f"sub_{func:X}"
                    d["text"] = base if rva == func else f"{base}+0x{rva - func:X}"
                else:
                    d["text"] = f"rva_{rva:X}"
            else:
                name = None
                if pe:
                    start = func if func is not None else rva
                    ex = pe.export_at(start)
                    if ex is None and func is None:
                        near = pe.nearest_export(rva)
                        if near and rva - near[0] < 0x400:
                            start, ex = near
                    if ex:
                        name = ex
                        d.update(func_rva=hx(start), name=f"{mod.name}!{ex}")
                        d["text"] = f"{mod.name}!{ex}" + (f"+0x{rva - start:X}" if rva != start else "")
                if not name:
                    if func is not None:
                        d["func_rva"] = hx(func)
                    d["text"] = f"{mod.name}+0x{rva:X}"
        if len(self._cache) < 200000:
            self._cache[va] = d
        return d

    def text(self, va):
        return self.describe(va)["text"]

    def game_rva(self, va):
        mod = self.dbg.module_at(va) if self.dbg else None
        return va - mod.base if mod is not None and mod is self.dbg.main else None

    # ---- parsing address specs
    def parse(self, spec):
        """Address spec -> ('rva', game_rva) | ('va', absolute) | ('export', module, name).
        Accepted: a game name (index/docs), 'module!export', a hex number: VA at the IDB base
        0x140000000, the Lucy base 0x7FF788F10000 or the kjs base 0x7FF6D1220000 (-> RVA), a
        value below the image size (RVA), anything else an absolute VA; 'va:0x...' forces absolute."""
        s = spec.strip()
        if s.lower().startswith("va:"):
            return ("va", int(s[3:], 0))
        if "!" in s:
            mod, name = s.split("!", 1)
            return ("export", mod, name)
        if re.fullmatch(r"(0x)?[0-9A-Fa-f`]+", s) and (s.lower().startswith("0x") or not self.lookup(s)):
            v = int(s.replace("`", ""), 16)
            size = self.game_pe.size_of_image if self.game_pe else 0x4000000
            for base in (IDA_BASE, LUCY_BASE, KJS_BASE):
                if base <= v < base + size:
                    return ("rva", v - base)
            if v < size:
                return ("rva", v)
            return ("va", v)
        rva = self.lookup(s)
        if rva is None:
            raise ValueError(f"unknown name {s!r} (not in the index or docs/analysis)")
        return ("rva", rva)

    def resolve(self, parsed):
        """Parsed spec -> absolute VA in the running process (None if its module is not loaded)."""
        kind = parsed[0]
        if kind == "rva":
            return self.dbg.main.base + parsed[1]
        if kind == "va":
            return parsed[1]
        mod = self.dbg.module(parsed[1])
        if mod is None or mod.pe is None:
            return None
        rva = mod.pe.export_rva(parsed[2])
        return None if rva is None else mod.base + rva

    def spec_text(self, parsed):
        if parsed[0] == "rva":
            name = self.game_name(parsed[1])
            return f"{name or 'rva'} 0x{parsed[1]:X} (lucy 0x{LUCY_BASE + parsed[1]:X})"
        if parsed[0] == "va":
            return f"0x{parsed[1]:X}"
        return f"{parsed[1]}!{parsed[2]}"


# ---- memory expressions: "[rcx+0x18]:8", "[[rcx+8]+0x10]:4", "rdx:str", "[rsp+0x28]:16"
_TOKEN = re.compile(r"\s*(?:(\[)|(\])|([+-])|([A-Za-z][A-Za-z0-9]*)|(0x[0-9A-Fa-f]+|\d+))")


class Expr:
    """expr ':' size; size: 1/2/4/8 (integer), N>8 (hex bytes), str, wstr. Without ':size' the
    value of the expression itself (an address or register)."""

    def __init__(self, text):
        self.text = text
        body, _, size = text.rpartition(":") if ":" in text else (text, "", "")
        self.size = size.strip() or None
        if self.size and self.size not in ("str", "wstr") and not self.size.isdigit():
            raise ValueError(f"bad size in {text!r}")
        self.tree = self._parse(body)

    def _parse(self, s):
        toks = []
        pos = 0
        while pos < len(s):
            m = _TOKEN.match(s, pos)
            if not m or m.end() == pos:
                if s[pos:].strip() == "":
                    break
                raise ValueError(f"bad expression {s!r} at {pos}")
            pos = m.end()
            if m.group(1):
                toks.append("[")
            elif m.group(2):
                toks.append("]")
            elif m.group(3):
                toks.append(m.group(3))
            elif m.group(4):
                name = m.group(4).lower()
                if name not in w.CONTEXT_FIELD:
                    raise ValueError(f"unknown register {name!r}")
                toks.append(("reg", name))
            else:
                toks.append(("num", int(m.group(5), 0)))
        self._toks, self._i = toks, 0
        tree = self._expr()
        if self._i != len(toks):
            raise ValueError(f"trailing tokens in {s!r}")
        return tree

    def _expr(self):
        node = self._term()
        while self._i < len(self._toks) and self._toks[self._i] in ("+", "-"):
            op = self._toks[self._i]
            self._i += 1
            node = (op, node, self._term())
        return node

    def _term(self):
        t = self._toks[self._i] if self._i < len(self._toks) else None
        if t == "[":
            self._i += 1
            inner = self._expr()
            if self._i >= len(self._toks) or self._toks[self._i] != "]":
                raise ValueError("missing ]")
            self._i += 1
            return ("deref", inner)
        if isinstance(t, tuple):
            self._i += 1
            return t
        raise ValueError(f"unexpected token {t!r}")

    def _eval(self, node, hit):
        op = node[0]
        if op == "reg":
            return hit.reg(node[1])
        if op == "num":
            return node[1]
        if op == "deref":
            return hit.dbg.read_u64(self._eval(node[1], hit))
        a, b = self._eval(node[1], hit), self._eval(node[2], hit)
        return (a + b if op == "+" else a - b) & 0xFFFFFFFFFFFFFFFF

    def address(self, hit):
        """Value of the expression; for '[x]:N' the address x."""
        tree = self.tree
        if self.size and tree[0] == "deref":
            tree = tree[1]
        return self._eval(tree, hit)

    def number(self, hit):
        """The expression as an integer: its value, or the 1/2/4/8 bytes read for ':N'. None when
        unreadable or not an integer size."""
        try:
            if not self.size:
                return self._eval(self.tree, hit)
            if self.size not in ("1", "2", "4", "8"):
                return None
            return int.from_bytes(hit.dbg.read(self.address(hit), int(self.size)), "little")
        except Exception:
            return None

    def value(self, hit):
        """Formatted value: hex int, hex bytes, or text. Unreadable -> None."""
        try:
            if not self.size:
                return hx(self._eval(self.tree, hit))
            addr = self.address(hit)
            dbg = hit.dbg
            if self.size == "str":
                return dbg.read_cstr(addr, 128)
            if self.size == "wstr":
                return dbg.read_cstr(addr, 128, wide=True)
            n = int(self.size)
            data = dbg.read(addr, n)
            if n in (1, 2, 4, 8):
                return hx(int.from_bytes(data, "little"))
            return data.hex()
        except Exception:
            return None


def field_spec(text):
    """'rcx+0xA8:8' or '[rcx+8]+0x10:16' -> (Expr for the address, size)."""
    body, _, size = text.rpartition(":")
    if not body or not size.isdigit():
        raise ValueError(f"field spec must be <expr>:<size>, got {text!r}")
    return Expr(body), int(size)


def split_ranges(va, size):
    """Cover [va, va+size) with aligned 1/2/4/8-byte debug register ranges."""
    out = []
    end = va + size
    while va < end:
        for n in (8, 4, 2, 1):
            if va % n == 0 and va + n <= end:
                out.append((va, n))
                va += n
                break
    return out
