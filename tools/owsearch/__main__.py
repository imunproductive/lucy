"""python -m tools.owsearch <command> ... (from the ORPHEUS root; build first: python -m tools.owsearch.build)

  const <value|GUID|short id> [--limit N] [--json]
      code that uses a constant: 0x1C6EC712, 0x0D800000000004AA (a GUID), 068A.025 / 000000000307.025 /
      097:0001 (a GUID by ORPHEUS's name), 04AA (a GUID index of any type: 1-6 hex digits without 0x).
  guid <value|short id> ...        decode GUIDs the way ORPHEUS writes them (0307.025, game message)
  cvar <hash|#index> ... | --all [--type T] [--json]
      console variables: value and default in the in-game dump, type, object address
  read <address> [--qword|--dword|--float|--double|--string|--bytes N] [--count N]
      typed read from the in-game dump (VA at a known base or an RVA)
"""

import argparse
import json
import struct
import sys

from tools import paths

from . import connect, consts, cvars
from . import guid as G

BASE = 0x7FF788F10000


def _va(rva):
    return f"{BASE + rva:#X}".replace("0X", "0x")


def _index():
    try:
        from tools.owindex import Index

        return Index.open()
    except FileNotFoundError:
        return None


# ------------------------------------------------------------------ const

def cmd_const(args):
    try:
        r = consts.search(args.value, limit=args.limit)
    except ValueError as e:
        return print(e)
    if args.json:
        return print(json.dumps(r, indent=1))
    g = r["guid"]
    if g:
        if g["value"]:
            print(f"GUID {g['value']} = {g['index']:04X}.{g['type']} ({g['type_name'] or 'type ' + g['type']})")
        else:
            print(f"GUID index {g['index']:04X}, any type")
    shown = len(r["hits"])
    print(f"{r['total']} uses" + (f" (first {shown}; --limit)" if shown < r["total"] else ""))
    for h in r["hits"]:
        note = {"imm": f"imm{8 * h['width']}", "data": f"loads {h['width']} bytes from {_va(h['at'])}",
                "raw": f"outside .pdata functions (raw match; constant at {_va(h['at'])})"}[h["kind"]]  # fmt: skip
        val = f"  [{h['guid']}]" if h.get("guid") else ""
        print(f"  {_va(h['rva'])}  {h['instruction']}  ; {note}{val}")
        if h["function"] is not None:
            print(f"      in {h['function_name'] or 'sub_%X' % (BASE + h['function'])} {_va(h['function'])}"
                  + (f": {h['what']}" if h["what"] else ""))  # fmt: skip
        elif h["what"]:
            print(f"      {h['what']}")
        if h["callers"]:
            more = h["callers_total"] - len(h["callers"])
            print("      called from " + ", ".join(f"{n or 'sub_%X' % (BASE + c)}" + (f" {_va(c)}" if n else "")
                                               for c, n in h["callers"])  # fmt: skip
                  + (f" (+{more} more)" if more > 0 else ""))  # fmt: skip
        for ref, what in h["refs"]:
            print(f"      referenced from {_va(ref)}" + (f": {what}" if what else ""))
    if r["data"]:
        db = connect()
        print(f"in data ({len(r['data'])}{'+' if len(r['data']) >= 20 else ''}):")
        for d in r["data"]:
            var = cvars.containing(db, d["rva"])
            what = d["what"] or (f"hash of console variable #{var['index']} ({var['type']})"
                                 if var and d["rva"] == var["rva"] + 0x10 else "")  # fmt: skip
            nxt = ""
            if d["next"] is not None:
                nxt = f"  next qword -> {d['next_name'] or 'sub_%X' % (BASE + d['next'])}"
            print(f"  {_va(d['rva'])}  {what}{nxt}".rstrip())
    return None


# ------------------------------------------------------------------ guid

def cmd_guid(args):
    for s in args.values:
        try:
            q = consts.parse(s)
        except ValueError as e:
            print(e)
            continue
        v = q["value"]
        if q["kind"] == "guid" and v is None:
            print(f"{s}: a GUID index of any type; name the type ({q['index']:04X}.01C, {q['index']:04X}.025, ...)"
                  f" or search its uses with `const {s}`")  # fmt: skip
            continue
        if not G.is_guid(v):
            print(f"{s}: not a GUID (a GUID has asset-type bits 48..59 and an index below 2^32)")
            continue
        d = G.describe(v)
        extra = f"  extra bits {d['extra_bits']}" if d["extra_bits"] else ""
        print(f"{d['value']}  {d['short']}  {d['type_name'] or 'type ' + d['type']}  ({d['name']}){extra}")


# ------------------------------------------------------------------ cvar

def _show(v, kind):
    if kind == "bool":
        return f"{int(v)} ({'true' if v else 'false'})"
    return cvars.show(v)


def _cvar_line(v):
    obj = _va(v["rva"])
    if v["type"] == "list":
        val = f"(heap list, not in the image dump){'  formats ' + v['hint'] if v['hint'] else ''}"
    else:
        val = f"value {_show(v['value'], v['type'])}  default {_show(v['default'], v['type'])}"
    return (f"cvar 0x{v['hash']:08X}  #{v['index']}  {v['type']}  {val}  object {obj} (value at +0x20 ="
            f" {_va(v['rva'] + 0x20)})")  # fmt: skip


def _cvar_users(c, idx, v):
    """[{'site', 'function', 'name', 'offset', 'access'}] of code referencing the variable's object."""
    out = []
    for target, site, root, access in c.refs(v["rva"], v["rva"] + (v["size"] or 0x28)):
        name = idx.name(root) if idx else None
        out.append({"site": site, "function": root, "name": name, "offset": target - v["rva"], "access": access})
    return out


def cmd_cvar(args):
    db = connect()
    if args.all:
        rows = [v for v in cvars.all_vars(db) if not args.type or v["type"] == args.type]
    else:
        rows = []
        for k in args.keys:
            found = cvars.lookup(db, k)
            if not found and not args.json:
                print(f"{k}: no console variable with this hash")
            rows += found
        c, idx = consts.Consts(), _index()
        for v in rows:
            v["code"] = _cvar_users(c, idx, v)
    if args.json:
        return print(json.dumps(rows, indent=1))
    for v in rows:
        print(_cvar_line(v))
        for u in v.get("code", []):
            fn = u["name"] or "sub_%X" % (BASE + u["function"])
            print(f"    {u['access']:<10} +{u['offset']:#04x}  {_va(u['site'])}  in {fn}")
    return None


# ------------------------------------------------------------------ read

def cmd_read(args):
    from tools.owindex import BASES, _parse_int
    from tools.owindex.image import Image

    img = Image(paths.DUMP)
    v = _parse_int(args.address)
    for b in (img.base, *BASES.values()):
        if b <= v < b + img.size:
            rva = v - b
            break
    else:
        if not 0 <= v < img.size:
            return print(f"{args.address}: not an RVA or a VA in the dump's image")
        rva = v
    va = img.base + rva
    if args.string:
        raw = img.data[rva:rva + 512].split(b"\0", 1)[0]
        print(f"{_va(rva)}: {raw.decode('utf-8', 'replace')!r}")
    elif args.bytes:
        for i in range(0, args.bytes, 16):
            chunk = img.data[rva + i:rva + min(i + 16, args.bytes)]
            text = "".join(chr(c) if 32 <= c < 127 else "." for c in chunk)
            print(f"{_va(rva + i)}  {chunk.hex(' '):<47}  {text}")
    else:
        fmt, size = {"qword": ("<Q", 8), "dword": ("<I", 4), "float": ("<f", 4), "double": ("<d", 8)}[args.kind]
        for i in range(args.count):
            (x,) = struct.unpack_from(fmt, img.data, rva + i * size)
            if isinstance(x, float):
                text = repr(x)
            else:
                text = f"0x{x:0{2 * size}X} ({x})"
                if size == 8 and G.is_guid(x):
                    text += f"  GUID {G.short(x)}"
                elif size == 8 and img.base <= x < img.base + img.size:
                    text += f"  -> rva {x - img.base:#x}"
            print(f"{_va(rva + i * size)}: {args.kind} {text}")
    idx = _index()
    if idx:
        a = idx.addr(rva)
        if a["name"] or a["what"]:
            print(f"  {a['name'] or ''} {a['what'] or ''}".rstrip())
    try:
        var = cvars.containing(connect(), rva)
    except FileNotFoundError:
        var = None
    if var:
        print(f"  in console variable 0x{var['hash']:08X} #{var['index']} ({var['type']}) at +{rva - var['rva']:#x}")
    return None


def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m tools.owsearch", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)  # fmt: skip
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("const", help="code that uses a constant")
    p.add_argument("value")
    p.add_argument("--limit", type=int, default=100)
    p.add_argument("--json", action="store_true")
    p = sub.add_parser("guid", help="decode GUIDs")
    p.add_argument("values", nargs="+")
    p = sub.add_parser("cvar", help="console variables")
    p.add_argument("keys", nargs="*")
    p.add_argument("--all", action="store_true")
    p.add_argument("--type", help="with --all: bool, int32, uint32, int64, uint64, double, string, list")
    p.add_argument("--json", action="store_true")
    p = sub.add_parser("read", help="typed read from the in-game dump")
    p.add_argument("address")
    k = p.add_mutually_exclusive_group()
    for kind in ("qword", "dword", "float", "double"):
        k.add_argument(f"--{kind}", dest="kind", action="store_const", const=kind)
    k.add_argument("--string", action="store_true")
    k.add_argument("--bytes", type=int)
    p.add_argument("--count", type=int, default=1)
    args = ap.parse_args(argv)
    if args.cmd == "read" and not args.kind:
        args.kind = "qword"
    if args.cmd == "cvar" and not args.keys and not args.all:
        ap.error("cvar: give a hash, #index or --all")
    {"const": cmd_const, "guid": cmd_guid, "cvar": cmd_cvar, "read": cmd_read}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
