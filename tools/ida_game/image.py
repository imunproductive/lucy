"""Input image for the game IDB: the in-game dump with re-encrypted code restored, then Rose.

The research repo's dump (`dumps/ingame_7FF788F10000.exe`, memory layout, base 0x7FF788F10000) holds
the code as it was in memory during a match. The protector encrypts its startup code and the game's
run-once init code (RVA 0x1000..0x12F000: Protector_Main, Ingame_RuntimeInit 0x108660, ...) again
after use. The research repo's `tools/dump_image.py` restores those ranges while dumping (its
tools/dumpfix.py, from the exe the game ran from), so a current dump is already clean; `restore()`
runs the same check again, which also fixes a dump made with `--no-restore` or by an older tool, and
records the result in research/out/ida_game/restore.json.

`rose()` runs rose-bin (`--nop-traps`, IDB only) on the restored image for the strong and weak
functions of the research repo's docs/analysis/obfuscated_functions.json, the masking stub, and
every .pdata function a byte scan finds an opposite-jcc pair in; then once per function Rose
patched (fn/<VA hex>.json), merges those into batch.json and writes the Rose-patched copy that IDA
loads, so auto-analysis already sees deobfuscated code.
"""
import bisect
import json
import os
import re
import struct
import subprocess
from concurrent.futures import ThreadPoolExecutor

from . import paths

DUMP_BASE = 0x7FF788F10000
IDB_BASE = 0x140000000


def _pdata(img):
    import pefile
    pe = pefile.PE(data=bytes(img[:0x1000]), fast_load=True)
    d = pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]
    out = []
    for i in range(d.VirtualAddress, d.VirtualAddress + d.Size, 12):
        b, e, _ = struct.unpack_from("<III", img, i)
        if b:
            out.append((b, e))
    return sorted(out)


def pdata_functions(img):
    """({primary begin RVA: end RVA}, {fragment begin RVA: primary begin RVA}). Entries whose
    unwind info has UNW_FLAG_CHAININFO are fragments of the function their chained
    RUNTIME_FUNCTION (after the aligned unwind-code array) leads to; the obfuscator splits
    functions into such fragments."""
    import pefile
    pe = pefile.PE(data=bytes(img[:0x1000]), fast_load=True)
    d = pe.OPTIONAL_HEADER.DATA_DIRECTORY[3]
    primary, parent = {}, {}
    for i in range(d.VirtualAddress, d.VirtualAddress + d.Size, 12):
        b, e, u = struct.unpack_from("<III", img, i)
        if not b:
            continue
        if (img[u] >> 3) & 4:
            parent[b] = struct.unpack_from("<I", img, u + 4 + 2 * ((img[u + 2] + 1) & ~1))[0]
        else:
            primary[b] = e
    fragment_of = {}
    for b in parent:
        p, seen = b, set()
        while p in parent and p not in seen:
            seen.add(p)
            p = parent[p]
        if p in primary:
            fragment_of[b] = p
    return primary, fragment_of


def restore(dump=paths.DUMP, exe=paths.GAME_EXE, out=paths.RESTORED, report=paths.RESTORE_JSON):
    from tools.paths import research_tool
    dumpfix = research_tool("dumpfix")
    img = bytearray(open(dump, "rb").read())
    result = dumpfix.restore(img, DUMP_BASE, exe)
    result.update({"dump": dump, "out": out})
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "wb") as f:
        f.write(img)
    with open(report, "w") as f:
        json.dump(result, f, indent=1)
    return {"restored_ranges": len(result["restored"]), "restored_bytes": result["restored_bytes"],
            "kept_as_dumped": len(result["kept_as_dumped"])}


# Rose roots besides the census: the masking stub 0x1400B7C60 (called from ~5100 game sites; as
# plain junk IDA never makes it a function and Hex-Rays prints `unk_7FF788FC7C60(...)` calls).
EXTRA_ROOT_RVAS = [0xB7C60]


def obfuscated_vas():
    """Strong and weak functions of the obfuscation census plus EXTRA_ROOT_RVAS. Weak ones (one
    fingerprint) are mostly game functions with an inline obfuscated pointer decryption (opposite
    jcc pair around `sar bl, 0`); left as is, auto-analysis decodes the junk and Hex-Rays fails
    with "stack frame is too big" (e.g. many statescript evaluators)."""
    data = json.load(open(paths.OBFUSCATED_JSON))
    vas = [int(f["addr"], 16) - IDB_BASE + DUMP_BASE
           for f in data["functions"] if f["confidence"] in ("strong", "weak")]
    return vas + [DUMP_BASE + rva for rva in EXTRA_ROOT_RVAS]


def jcc_pair_functions(img):
    """RVAs of the .pdata functions containing an opposite short-jcc pair with one target
    (`jcc X; <up to 8 bytes, e.g. sar bl, 0>; jncc X`), found by a byte scan. The census's
    linear sweep misses functions where junk desynchronizes it (e.g. evaluator 0x7FF78982EBB0);
    false positives only cost a Rose run (Rose patches what it proves)."""
    funcs = _pdata(img)
    starts = [b for b, _ in funcs]
    text_end = 0x1000 + struct.unpack_from("<I", img, _text_size_offset(img))[0]
    hits = set()
    for m in re.finditer(rb"[\x70-\x7f]", bytes(img[0x1000:text_end])):
        i = m.start() + 0x1000
        op, target = img[i], i + 2 + struct.unpack_from("b", img, i + 1)[0]
        for j in range(i + 2, i + 12):
            if img[j] == op ^ 1 and j + 2 + struct.unpack_from("b", img, j + 1)[0] == target:
                k = bisect.bisect_right(starts, i) - 1
                if k >= 0 and i < funcs[k][1]:
                    hits.add(funcs[k][0])
                break
    return sorted(hits)


def _text_size_offset(img):
    """File offset of the first section's VirtualSize (.text)."""
    pe = struct.unpack_from("<I", img, 0x3C)[0]
    opt_size = struct.unpack_from("<H", img, pe + 20)[0]
    return pe + 24 + opt_size + 8


def _rose_cmd(image, vas, out):
    cmd = [paths.ROSE_BIN, image, "--nop-traps", "--log-level", "error", "-o", out]
    for va in vas:
        cmd += ["-f", "%x" % va]
    return cmd


def _run(cmd):
    # rose-bin exits non-zero when some function fails; the report still lists every function.
    # CREATE_NO_WINDOW: rose-bin is a console program; without it every run flashes a window.
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800,
                   creationflags=subprocess.CREATE_NO_WINDOW)


def rose(image=paths.RESTORED, out_dir=paths.ROSE_DIR, patched=paths.INPUT_EXE, threads=4):
    """Rose on census + jcc-pair candidates, then again on the functions Rose patched only, so
    batch.json's blocks and functions agree (the IDB phases delete non-entry functions that own a
    listed block). Both passes run in batches of 1000 `-f` (the Windows command line limit), 4
    processes, no console windows. The merged patches are applied to `image` -> `patched`."""
    img = bytearray(open(image, "rb").read())
    _, fragment_of = pdata_functions(img)
    # A chained .pdata fragment is analyzed through its primary function (Rose follows the
    # jumps); as a root of its own it would make the IDB phases split the parent function.
    rvas = {va - DUMP_BASE for va in obfuscated_vas()} | set(jcc_pair_functions(img))
    vas = sorted({DUMP_BASE + fragment_of.get(r, r) for r in rvas})
    fn_dir = os.path.join(out_dir, "fn")  # per-function reports of older builds
    if os.path.isdir(fn_dir):
        for name in os.listdir(fn_dir):
            os.remove(os.path.join(fn_dir, name))
        os.rmdir(fn_dir)
    os.makedirs(out_dir, exist_ok=True)
    chunks = [vas[i:i + 1000] for i in range(0, len(vas), 1000)]
    scan_paths = [os.path.join(out_dir, "scan%d.json" % k) for k in range(len(chunks))]
    with ThreadPoolExecutor(threads) as pool:
        list(pool.map(_run, [_rose_cmd(image, c, p) for c, p in zip(chunks, scan_paths)]))
    status, errors, patched_fns = {}, {}, []
    for p in scan_paths:
        for f in json.load(open(p))["functions"]:
            status[f["status"]] = status.get(f["status"], 0) + 1
            if f["status"] != "ok":
                errors[f["addr"]] = f.get("error")
            elif f.get("patches"):
                patched_fns.append(int(f["addr"], 16))
        os.remove(p)
    chunks = [patched_fns[i:i + 1000] for i in range(0, len(patched_fns), 1000)]
    part_paths = [os.path.join(out_dir, "part%d.json" % k) for k in range(len(chunks))]
    with ThreadPoolExecutor(threads) as pool:
        list(pool.map(_run, [_rose_cmd(image, c, p) for c, p in zip(chunks, part_paths)]))
    merged = {"image": image, "image_base": DUMP_BASE, "functions": [], "patches": [], "blocks": [],
              "stack": [], "issues": [], "tail_calls": [], "halts": []}
    seen_patch, seen_block, conflicts = {}, set(), []
    for path in part_paths:
        rep = json.load(open(path))
        os.remove(path)
        merged["functions"] += rep["functions"]
        for p in rep["patches"]:
            key = (p["bytes"], p["original"])
            if p["addr"] in seen_patch and seen_patch[p["addr"]] != key:
                conflicts.append(p["addr"])
                continue
            if p["addr"] not in seen_patch:
                seen_patch[p["addr"]] = key
                merged["patches"].append(p)
        for b in rep["blocks"]:
            if (b["addr"], b["size"]) not in seen_block:
                seen_block.add((b["addr"], b["size"]))
                merged["blocks"].append(b)
        for k in ("stack", "issues", "tail_calls", "halts"):
            merged[k] += rep.get(k, [])
    with open(os.path.join(out_dir, "batch.json"), "w") as f:
        json.dump(merged, f)
    bad = 0
    for p in merged["patches"]:
        rva = int(p["addr"], 16) - DUMP_BASE
        new, old = bytes.fromhex(p["bytes"]), bytes.fromhex(p["original"])
        if bytes(img[rva:rva + len(old)]) != old:
            bad += 1
            continue
        img[rva:rva + len(new)] = new
    with open(patched, "wb") as f:
        f.write(img)
    return {"candidates": len(vas), "status": status, "patched_functions": len(patched_fns),
            "patches": len(merged["patches"]), "patch_conflicts": conflicts[:20],
            "patch_original_mismatch": bad, "errors": errors}
