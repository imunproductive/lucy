"""Build the headless game IDB research/out/ida_game/ow_game.i64 from the research repo's image dump
dumps/ingame_7FF788F10000.exe (locations: tools/paths.py). Run from the Lucy root.

    python -m tools.ida_game.build                     # every phase not done yet, in order
    python -m tools.ida_game.build --phase create      # one phase (re-runs it)
    python -m tools.ida_game.build --detach            # same, as a detached background process
    python -m tools.ida_game.build --status            # what is done, analysis progress

Phases (state in research/out/ida_game/build_state.json, log in research/out/ida_game/build.log):
  image   check the dump for code the protector re-encrypted after use (the research repo's
          tools/dumpfix.py; a dump from its tools/dump_image.py is already clean), run Rose on the
          obfuscated functions (census strong+weak, masking stub, opposite-jcc
          byte scan; ~7000 candidates, ~3100 patched) and write the Rose-patched input
          research/out/ida_game/ow_game.exe (see image.py). No IDA. ~2 min.
  create  load ow_game.exe as a PE at 0x7FF788F10000 into ow_game.i64 and run IDA's full
          auto-analysis in passes over 1 MB chunks of each segment, one log line per chunk and a
          checkpoint save every 15 min (resumable: a killed run continues after the last
          checkpoint), then auto_wait for the rest. ~13 min.
  rose    apply Rose's results headless (the research repo's tools/ida_apply_rose.py
          patch/code/funcs/spd, plus own xrefs/clean/tails variants for a fully analyzed database)
          and re-analyze.
  vslots  define a function at every target of the index's runtime vtables (registrations),
          component and message vtables, factories, constructors and message handlers that is
          not one yet (Rose-hidden stubs between filler; funcs.py).
  names   names from tools.owindex (research/out/index/ow_index.sqlite; component getters as
          GetComp<N>) and the research repo's docs/analysis/*.json.
  types   one struct STU_<HASH> per STU class of the index; `STU_<H> *stu` on config evaluators
          and action runs, `StateRt_<H> *this` (property block typed) on state readers/writers.
Only one process can have the database open (lock file ow_game.i64.lock); open_game_idb() waits
or fails while a build runs, and a build fails while another process has it open (`--wait S`).
"""
import bisect
import argparse
import json
import os
import subprocess
import sys
import time

from . import paths
from . import _idalib

PHASES = ["image", "create", "rose", "vslots", "names", "types"]
CHUNK = 0x100000


def log(msg):
    line = "%s %s" % (time.strftime("%Y-%m-%d %H:%M:%S"), msg)
    print(line, flush=True)
    os.makedirs(paths.OUT_DIR, exist_ok=True)
    with open(paths.LOG, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def load_state():
    if os.path.exists(paths.STATE):
        return json.load(open(paths.STATE))
    return {}


def save_state(state):
    tmp = paths.STATE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(state, f, indent=1)
    os.replace(tmp, paths.STATE)


def checkpoint():
    import ida_loader
    t = time.time()
    ok = ida_loader.save_database(paths.IDB, 0)
    log("  checkpoint save %s (%.0f s)" % ("ok" if ok else "FAILED", time.time() - t))


# ---------------------------------------------------------------- phases

def phase_image(state):
    from . import image
    log("image: restoring re-encrypted code")
    r = image.restore()
    log("image: %s" % r)
    log("image: running Rose")
    rr = image.rose()
    log("image: Rose %s" % {k: v for k, v in rr.items() if k != "errors"})
    for va, err in rr["errors"].items():
        log("image:   Rose failed %s: %s" % (va, err))
    return {"restore": r, "rose": rr}


def phase_create(state):
    import ida_auto
    import ida_funcs
    import ida_segment
    if not os.path.exists(paths.INPUT_EXE):
        raise RuntimeError("run --phase image first")
    progress = state.setdefault("create_progress", {"done_chunks": [], "seconds": 0.0})
    fresh = not os.path.exists(paths.IDB)
    t0 = time.time()
    _idalib.open_db(paths.IDB, paths.INPUT_EXE)
    log("create: %s database (%.0f s), %d functions"
        % ("loaded new" if fresh else "reopened", time.time() - t0, ida_funcs.get_func_qty()))
    if fresh:
        checkpoint()
    ida_auto.enable_auto(True)
    chunks = []
    for i in range(ida_segment.get_segm_qty()):
        seg = ida_segment.getnseg(i)
        for a in range(seg.start_ea, seg.end_ea, CHUNK):
            chunks.append((a, min(a + CHUNK, seg.end_ea), ida_segment.get_segm_name(seg)))
    # Analysis of one chunk queues work in others (callees, data refs): repeat passes over the
    # chunks while they still find a lot of work, then let auto_wait finish the rest.
    last_save = time.time()
    while True:
        done = set(progress["done_chunks"])
        pass_no = progress.setdefault("pass", 1)
        steps_total = progress.setdefault("pass_steps", 0)
        for n, (a, b, name) in enumerate(chunks):
            if a in done:
                continue
            t = time.time()
            steps = ida_auto.auto_wait_range(a, b)
            progress["seconds"] += time.time() - t
            progress["done_chunks"].append(a)
            progress["pass_steps"] = steps_total = steps_total + max(steps, 0)
            log("create: pass %d chunk %d/%d %s %#x..%#x: %d steps, %.0f s, %d functions"
                % (pass_no, n + 1, len(chunks), name, a, b, steps, time.time() - t,
                   ida_funcs.get_func_qty()))
            if time.time() - last_save > 900:
                # The state only records chunks whose analysis is in the saved database. Work
                # lost by a kill after a checkpoint is still in the saved queues.
                checkpoint()
                save_state(state)
                last_save = time.time()
        progress.setdefault("pass_history", []).append(steps_total)
        log("create: pass %d done, %d steps" % (pass_no, steps_total))
        first = progress["pass_history"][0]
        if pass_no >= 4 or steps_total < max(100000, first // 20):
            break
        progress.update({"pass": pass_no + 1, "pass_steps": 0, "done_chunks": []})
        checkpoint()
        save_state(state)
    checkpoint()
    save_state(state)
    log("create: final auto_wait (no progress output until it returns)")
    t = time.time()
    ida_auto.auto_wait()
    progress["seconds"] += time.time() - t
    nfuncs = ida_funcs.get_func_qty()
    log("create: auto-analysis finished (final wait %.0f s, total analysis %.0f s), %d functions"
        % (time.time() - t, progress["seconds"], nfuncs))
    _idalib.close_db(save=True)
    return {"functions": nfuncs, "analysis_seconds": round(progress["seconds"])}


def _rose_phase(state):
    import ida_auto
    import ida_funcs
    from tools.paths import research_tool
    ar = research_tool("ida_apply_rose")
    batch = os.path.join(paths.ROSE_DIR, "batch.json")
    data, blocks, funcs = ar._load(batch)
    out = {}
    for name, fn in [("patch", lambda: ar.phase_patch(data)),
                     ("code", lambda: ar.phase_code(blocks)),
                     ("xrefs", lambda: _xrefs(ar, blocks, funcs)),
                     ("funcs", lambda: _funcs(ar, blocks, funcs)),
                     ("clean", lambda: _clean(ar, blocks, funcs)),
                     ("tails", lambda: _tails(ar, blocks, funcs)),
                     ("spd", lambda: ar.phase_spd(data))]:
        t = time.time()
        r = fn()
        ida_auto.auto_wait()
        out[name] = r
        log("rose: %s %.0f s: %s" % (name, time.time() - t, json.dumps(r)[:400]))
    bad = [hex(ea) for ea in funcs if not (ida_funcs.get_func(ea) and ida_funcs.get_func(ea).start_ea == ea)]
    out["functions_ok"] = len(funcs) - len(bad)
    out["functions_missing"] = bad
    log("rose: %d/%d Rose functions are IDA functions" % (out["functions_ok"], len(funcs)))
    return out


def _funcs(ar, blocks, funcs):
    """ida_apply_rose.phase_funcs for a fully analyzed database. A function that owns Rose block
    code but is not a Rose entry is deleted only when its start is not a .pdata function start
    (then it was built from junk). Real functions are kept: auto-analysis made a Rose entry's
    code a tail chunk of a stub that jumps there, or flowed past the function end; drop the
    tail chunks that hold block code and cut the main chunk back to the .pdata end."""
    import ida_bytes
    import ida_funcs
    import ida_hexrays
    import idc
    from . import image
    primary, _ = image.pdata_functions(open(paths.INPUT_EXE, "rb").read())
    entries = set(funcs)
    owners = {}
    for start, end in blocks:
        ea = start
        while ea != idc.BADADDR and ea < end:
            f = ida_funcs.get_func(ea)
            if f and f.start_ea not in entries:
                owners.setdefault(f.start_ea, []).append(ea)
            ea = ida_bytes.next_head(ea, end)
    out = {"bogus_funcs_deleted": [], "real_funcs_trimmed": [], "real_funcs_conflict": []}
    for start, eas in owners.items():
        pfn = ida_funcs.get_func(start)
        if pfn is None:
            continue
        if start - image.DUMP_BASE not in primary:
            ida_funcs.del_func(start)
            out["bogus_funcs_deleted"].append(hex(start))
            continue
        for tail in [c.start_ea for c in ida_funcs.func_tail_iterator_t(pfn) if c.start_ea != start]:
            chunk = ida_funcs.get_fchunk(tail)
            if any(chunk.start_ea <= ea < chunk.end_ea for ea in eas):
                ida_funcs.remove_func_tail(pfn, tail)
        pdata_end = image.DUMP_BASE + primary[start - image.DUMP_BASE]
        pfn = ida_funcs.get_func(start)
        if any(pfn.start_ea <= ea < pfn.end_ea for ea in eas):
            if all(ea >= pdata_end for ea in eas if pfn.start_ea <= ea < pfn.end_ea):
                ida_funcs.set_func_end(start, pdata_end)
            else:
                out["real_funcs_conflict"].append(hex(start))
                continue
        out["real_funcs_trimmed"].append(hex(start))
    # Rose entries: as in ida_apply_rose.phase_funcs.
    for ea in funcs:
        f = ida_funcs.get_func(ea)
        if f and f.start_ea == ea:
            for start in [c.start_ea for c in ida_funcs.func_tail_iterator_t(f) if c.start_ea != ea]:
                ida_funcs.remove_func_tail(f, start)
            bounds = ida_funcs.func_t(ea)
            found = ida_funcs.find_func_bounds(
                bounds, ida_funcs.FIND_FUNC_DEFINE | ida_funcs.FIND_FUNC_IGNOREFN)
            if found == ida_funcs.FIND_FUNC_OK and bounds.end_ea > f.end_ea:
                ida_funcs.set_func_end(ea, bounds.end_ea)
            ida_funcs.reanalyze_function(ida_funcs.get_func(ea))
        else:
            ida_funcs.add_func(ea)
        ida_hexrays.mark_cfunc_dirty(ea)
    out["functions"] = sum(1 for ea in funcs if ida_funcs.get_func(ea))
    for k in ("bogus_funcs_deleted", "real_funcs_trimmed", "real_funcs_conflict"):
        out[k + "_count"] = len(out[k])
        out[k] = out[k][:20]
    return out


def _clean(ar, blocks, funcs):
    """Undefine the code items inside each Rose function that Rose's blocks do not reach. Full
    auto-analysis decoded junk bytes (between real blocks, after junk jumps, in chained .pdata
    fragments); set_func_end in 'funcs' pulls them into the function, and their bogus stack
    operands (`[rbp+74B30A2Ch]`) make Hex-Rays fail with "stack frame is too big" / "function
    frame is wrong". Kept: block items and whatever IDA's flow/jump references reach from them
    (switch targets Rose leaves unresolved). Functions that lost items are recreated so their
    frame is rebuilt from the remaining instructions; 'tails' re-adds separate chunks."""
    import ida_bytes
    import ida_funcs
    import ida_xref
    import idautils
    starts = [s for s, _ in blocks]
    out = {"functions": 0, "items_removed": 0, "recreate_failed": []}
    for entry in funcs:
        pfn = ida_funcs.get_func(entry)
        if pfn is None or pfn.start_ea != entry:
            continue
        items = set(idautils.FuncItems(entry))
        keep, todo = set(), [ea for ea in items if ar._in_blocks(blocks, starts, ea)]
        while todo:
            ea = todo.pop()
            if ea in keep:
                continue
            keep.add(ea)
            for x in idautils.XrefsFrom(ea, 0):
                if x.iscode and x.type in (ida_xref.fl_F, ida_xref.fl_JN, ida_xref.fl_JF) and x.to in items:
                    todo.append(x.to)
        junk = sorted(items - keep)
        if not junk:
            continue
        for ea in junk:
            ida_bytes.del_items(ea, ida_bytes.DELIT_SIMPLE, 1)
        ida_funcs.del_func(entry)
        if not ida_funcs.add_func(entry):
            # Flow from the entry runs into something add_func refuses: make the function just
            # the entry block; 'tails' appends the other blocks from the per-function report.
            i = bisect.bisect_right(starts, entry) - 1
            if not (i >= 0 and ida_funcs.add_func(entry, blocks[i][1])):
                out["recreate_failed"].append(hex(entry))
        out["functions"] += 1
        out["items_removed"] += len(junk)
    return out


def _xrefs(ar, blocks, funcs):
    """ida_apply_rose.phase_xrefs, except that references to Rose function entries are kept: in
    this fully analyzed database the callers of the Rose functions are real code (the GUI IDB's
    phase removes every outside reference, which would delete those call instructions)."""
    import ida_bytes
    import ida_xref
    import idautils
    import idc
    entries = set(funcs)
    starts = [s for s, _ in blocks]
    removed = set()
    for start, end in blocks:
        ea = start
        while ea != idc.BADADDR and ea < end:
            if ea not in entries:
                for x in list(idautils.XrefsTo(ea, 0)):
                    if x.iscode and x.type != ida_xref.fl_F and not ar._in_blocks(blocks, starts, x.frm):
                        ida_xref.del_cref(x.frm, ea, False)
                        if x.frm not in removed and idc.is_code(ida_bytes.get_flags(x.frm)):
                            ida_bytes.del_items(x.frm, ida_bytes.DELIT_SIMPLE, 1)
                            removed.add(x.frm)
            ea = ida_bytes.next_head(ea, end)
    return {"junk_sources_removed": len(removed), "examples": sorted(map(hex, removed))[:20]}


def _tails(ar, blocks, funcs):
    """Like ida_apply_rose.append_tails, but per instruction and without per-function reports:
    add_func's flow can stop inside a block (after a call it takes for noreturn), so a block
    whose start is owned can still have an unowned rest (Hex-Rays then prints JUMPOUT to it).
    Every unowned run of block instructions goes to the Rose function whose code jumps or flows
    into it; repeated until nothing changes (runs reached only through other runs)."""
    import ida_bytes
    import ida_funcs
    import ida_hexrays
    import idautils
    import idc
    entries = set(funcs)
    runs = []
    for start, end in blocks:
        ea = start
        while ea != idc.BADADDR and ea < end:
            size = ida_bytes.get_item_size(ea) if idc.is_code(ida_bytes.get_flags(ea)) else 0
            if size and ida_funcs.get_func(ea) is None:
                if runs and runs[-1][1] == ea:
                    runs[-1][1] = ea + size
                else:
                    runs.append([ea, ea + size])
            ea = ida_bytes.next_head(ea, end)
    out = {"unowned_runs": len(runs), "tails_added": 0, "functions": set()}
    changed = True
    while changed and runs:
        changed, left = False, []
        for a, b in runs:
            owner = None
            if a in entries:  # a Rose entry is its own function, never another one's tail
                left.append([a, b])
                continue
            for x in idautils.XrefsTo(a, 0):
                f = ida_funcs.get_func(x.frm) if x.iscode else None
                if f is not None and f.start_ea in entries:
                    owner = f
                    break
            if owner is not None and ida_funcs.append_func_tail(owner, a, b):
                out["tails_added"] += 1
                out["functions"].add(owner.start_ea)
                ida_hexrays.mark_cfunc_dirty(owner.start_ea)
                changed = True
            else:
                left.append([a, b])
        runs = left
    out["functions"] = len(out["functions"])
    out["unowned_runs_left"] = len(runs)
    out["missing_entries"] = [hex(ea) for ea in funcs
                              if not (ida_funcs.get_func(ea) and ida_funcs.get_func(ea).start_ea == ea)][:20]
    return out


def phase_rose(state):
    _idalib.open_db(paths.IDB)
    try:
        r = _rose_phase(state)
    except BaseException:
        _idalib.close_db(save=False)
        raise
    _idalib.close_db(save=True)
    return r


def phase_vslots(state):
    from . import funcs
    _idalib.open_db(paths.IDB)
    try:
        r = funcs.apply_vslots(log)
    except BaseException:
        _idalib.close_db(save=False)
        raise
    _idalib.close_db(save=True)
    return r


def phase_names(state):
    from . import names
    _idalib.open_db(paths.IDB)
    try:
        r = names.apply(log)
    except BaseException:
        _idalib.close_db(save=False)
        raise
    _idalib.close_db(save=True)
    return r


def phase_types(state):
    from . import stutypes
    _idalib.open_db(paths.IDB)
    try:
        r = stutypes.apply(log, paths.CODECS_JSON)
    except BaseException:
        _idalib.close_db(save=False)
        raise
    _idalib.close_db(save=True)
    return r


RUN = {"image": phase_image, "create": phase_create, "rose": phase_rose, "vslots": phase_vslots,
       "names": phase_names, "types": phase_types}


def run_phase(name, state, wait=0.0):
    log("=== phase %s" % name)
    t = time.time()
    if name == "image":
        result = RUN[name](state)
    else:
        _idalib.ensure()
        with _idalib.DbLock(timeout=wait):
            result = RUN[name](state)
    state.setdefault("phases", {})[name] = {"done": True, "seconds": round(time.time() - t),
                                            "finished": time.strftime("%Y-%m-%d %H:%M:%S"),
                                            "result": result}
    save_state(state)
    log("=== phase %s done in %.0f s" % (name, time.time() - t))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--phase", choices=PHASES, action="append",
                    help="run this phase (repeatable); default: every phase not done yet")
    ap.add_argument("--detach", action="store_true", help="run as a detached background process")
    ap.add_argument("--status", action="store_true")
    ap.add_argument("--rebuild", action="store_true",
                    help="delete the database and the phase state first, then run every phase")
    ap.add_argument("--wait", type=float, default=0.0, metavar="S",
                    help="wait up to S seconds for the database lock (another process has it open)")
    args = ap.parse_args()
    if args.rebuild and not args.detach:
        with _idalib.DbLock(timeout=1800):
            for ext in (".i64", ".id0", ".id1", ".id2", ".nam", ".til"):
                path = os.path.splitext(paths.IDB)[0] + ext
                if os.path.exists(path):
                    os.remove(path)
            if os.path.exists(paths.STATE):
                os.remove(paths.STATE)
        log("=== rebuild: database and state deleted")
    state = load_state()
    if args.status:
        phases = state.get("phases", {})
        for p in PHASES:
            print("%-7s %s" % (p, "done %s (%s s)" % (phases[p]["finished"], phases[p]["seconds"])
                               if p in phases else "pending"))
        prog = state.get("create_progress")
        if prog and "create" not in phases:
            print("create: %d chunks analyzed, %.0f s so far" % (len(prog["done_chunks"]), prog["seconds"]))
        return
    if args.detach:
        cmd = [sys.executable, "-m", "tools.ida_game.build"]
        for p in args.phase or []:
            cmd += ["--phase", p]
        if args.wait:
            cmd += ["--wait", str(args.wait)]
        if args.rebuild:
            cmd.append("--rebuild")
        flags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
        os.makedirs(paths.OUT_DIR, exist_ok=True)
        out = open(os.path.join(paths.OUT_DIR, "build.out"), "ab")
        p = subprocess.Popen(cmd, cwd=paths.LUCY, creationflags=flags, stdout=out, stderr=out,
                             stdin=subprocess.DEVNULL)
        print("started pid %d; follow %s" % (p.pid, paths.LOG))
        return
    todo = args.phase or [p for p in PHASES if p not in state.get("phases", {})]
    for name in todo:
        run_phase(name, state, args.wait)


if __name__ == "__main__":
    main()
