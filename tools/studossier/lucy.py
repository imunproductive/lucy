"""Where a class is used in the game data: this repo's target/release/lucy.exe (env LUCY_BIN)
`script query class <hash>` on the tools' shared state folder paths.LUCY_STATE (research/out/lucy_state,
never the user's own folder)."""
import os
import re
import shutil
import subprocess
from pathlib import Path

from tools import paths

LUCY = paths.LUCY_EXE
USER_STATE = Path(os.environ.get("APPDATA", "")) / "lucy"
STATE = paths.LUCY_STATE
_LOG = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z\s+(INFO|WARN|DEBUG|TRACE)\b")


def state():
    """The shared state folder. Made once; its cache/ (derived game data) is seeded from a copy of the user's
    when there is one, else the first query derives it (~240 s). Every later query reuses it."""
    if not STATE.exists():
        STATE.mkdir(parents=True)
        if (USER_STATE / "cache").is_dir():
            shutil.copytree(USER_STATE / "cache", STATE / "cache")
    return STATE


def query_command(*args):
    """`lucy script query ...` on the shared state folder and the research game install."""
    return [str(LUCY), "script", "query", "--state", str(state()), "--game", str(paths.LUCY_GAME), *args]


def where_used(class_hash, timeout=300):
    """-> {"command", "lines"} or {"error"}."""
    if not LUCY.exists():
        return {"error": "%s missing (cargo build --release, or set LUCY_BIN)" % LUCY}
    cmd = query_command("class", class_hash)
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, encoding="utf-8", errors="replace")
    except subprocess.TimeoutExpired:
        return {"error": "lucy timed out after %d s" % timeout, "command": cmd}
    lines = [ln for ln in (p.stdout + p.stderr).splitlines() if ln.strip() and not _LOG.match(ln)]
    if p.returncode:
        return {"error": "lucy exit %d: %s" % (p.returncode, " | ".join(lines[-3:])), "command": cmd}
    return {"command": cmd, "lines": lines}


CODECS = paths.CODECS_JSON
_codecs = []


def codec(class_hash):
    """Lucy's decoded network codec of a state class (statescript_codecs.json "classes" entry) or None."""
    if not _codecs:
        try:
            import json
            with open(CODECS, encoding="utf-8") as f:
                _codecs.append(json.load(f).get("classes", {}))
        except (OSError, ValueError):
            _codecs.append({})
    return _codecs[0].get(class_hash)
