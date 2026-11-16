"""Pseudocode of several functions from the game IDB in one run (opened once, in place, under the
shared lock; nothing is saved).

    python -m tools.ida_game.decompile 0x7FF789E74940 0xF66AF0 Eval_STU_AC829876 [--define] [--out F]

Addresses: RVA, VA at 0x7FF788F10000 or 0x140000000 (hex, `0x` optional, windbg backticks allowed)
or a name in the database. Each function is printed once. `--define`: an address no function holds
gets one first (GameIdb.define; in memory only). When Hex-Rays fails, the function's disassembly
(or --lines instructions from the address) is printed instead.
"""
import argparse
import sys

from . import open_game_idb, DecompileError


def _resolve(idb, text):
    import ida_name
    s = text.strip().replace("`", "")
    try:
        return idb.ea(int(s[2:] if s.lower().startswith("0x") else s, 16))
    except ValueError:
        ea = ida_name.get_name_ea(0xFFFFFFFFFFFFFFFF, s)
        return None if ea == 0xFFFFFFFFFFFFFFFF else ea


def _disasm(idb, ea, lines):
    import ida_funcs
    import idautils
    f = ida_funcs.get_func(ea)
    if f is None:
        return idb.disasm(ea, lines)
    return "\n".join(idb.disasm(x, 1) for x in idautils.FuncItems(f.start_ea))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("addr", nargs="+")
    ap.add_argument("--define", action="store_true", help="define a function where none holds the address")
    ap.add_argument("--out", help="write here instead of stdout")
    ap.add_argument("--lines", type=int, default=60, help="instructions shown when no function holds an address")
    ap.add_argument("--timeout", type=float, default=1800, help="seconds to wait for the database lock")
    args = ap.parse_args(argv)
    out = open(args.out, "w", encoding="utf-8") if args.out else sys.stdout
    seen = set()
    try:
        with open_game_idb(timeout=args.timeout) as idb:
            for text in args.addr:
                ea = _resolve(idb, text)
                if ea is None:
                    out.write("// %s: unknown name\n\n" % text)
                    continue
                if args.define:
                    idb.define(ea)
                f = idb.func(ea)
                key = f["start_va"] if f else ea
                if key in seen:
                    continue
                seen.add(key)
                if f and f["start_va"] != ea:
                    out.write("// %s = %#x is inside %s\n" % (text, ea, f["name"]))
                try:
                    out.write(idb.decompile(ea) + "\n")
                except DecompileError as e:
                    out.write("// %s: %s; disassembly:\n%s\n\n" % (text, e, _disasm(idb, ea, args.lines)))
                out.flush()
    finally:
        if args.out:
            out.close()


if __name__ == "__main__":
    main()
