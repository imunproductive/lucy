"""Where a class is used in the game data: this repo's target/release/orpheus.exe (env ORPHEUS_BIN)
`script query class <hash>` on a temp copy of the ORPHEUS state folder (never the user's own folder)."""
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from tools import paths

ORPHEUS = paths.ORPHEUS_EXE
USER_STATE = Path(os.environ.get("APPDATA", "")) / "orpheus"
STATE_COPY = Path(tempfile.gettempdir()) / "studossier" / "orpheus_state"
_LOG = re.compile(r"^\d{4}-\d\d-\d\dT[\d:.]+Z\s+(INFO|WARN|DEBUG|TRACE)\b")


def _state():
    """Temp copy of the ORPHEUS state folder (made once; its derived-data cache makes queries fast)."""
    if not STATE_COPY.exists():
        STATE_COPY.parent.mkdir(parents=True, exist_ok=True)
        if USER_STATE.is_dir():
            shutil.copytree(USER_STATE, STATE_COPY, ignore=shutil.ignore_patterns("logs"))
        else:
            STATE_COPY.mkdir()
    return STATE_COPY


def where_used(class_hash, timeout=90):
    """-> {"command", "lines"} or {"error"}."""
    if not ORPHEUS.exists():
        return {"error": "%s missing (cargo build --release, or set ORPHEUS_BIN)" % ORPHEUS}
    game = os.environ.get("ORPHEUS_GAME") or str(paths.GAME_DIR.parent)
    cmd = [str(ORPHEUS), "script", "query", "--state", str(_state()), "--game", game, "class", class_hash]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, encoding="utf-8", errors="replace")
    except subprocess.TimeoutExpired:
        return {"error": "orpheus timed out after %d s" % timeout, "command": cmd}
    lines = [ln for ln in (p.stdout + p.stderr).splitlines() if ln.strip() and not _LOG.match(ln)]
    if p.returncode:
        return {"error": "orpheus exit %d: %s" % (p.returncode, " | ".join(lines[-3:])), "command": cmd}
    return {"command": cmd, "lines": lines}


CODECS = paths.CODECS_JSON
_codecs = []


def codec(class_hash):
    """ORPHEUS's decoded network codec of a state class (statescript_codecs.json "classes" entry) or None."""
    if not _codecs:
        try:
            import json
            with open(CODECS, encoding="utf-8") as f:
                _codecs.append(json.load(f).get("classes", {}))
        except (OSError, ValueError):
            _codecs.append({})
    return _codecs[0].get(class_hash)
