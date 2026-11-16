"""Every file location the client question tools use (owindex, ida_game, studossier, stu.py,
who_reads.py, owdbg). Run the tools from the ORPHEUS root (`python -m tools.owindex ...`).

Inputs come from three places:
- the research repo (env ORPHEUS_RESEARCH_DIR, default D:/Work/Research/Overwatch): the image dump,
  the debug build of the game, the protector analyses and two research scripts (dumpfix.py,
  ida_apply_rose.py) the game IDB build loads from there;
- this repo: the release build of orpheus.exe and the relay DLL, OWLib's STU table, the statescript
  codec table;
- external programs: IDA 9.3 (env IDADIR) and rose-bin (env ROSE_BIN).
Everything the tools generate goes under research/out/ (git-ignored).
"""
import importlib.util
import os
import sys
from pathlib import Path

ORPHEUS = Path(__file__).resolve().parents[1]

# --- research repo inputs
RESEARCH = Path(os.environ.get("ORPHEUS_RESEARCH_DIR", "D:/Work/Research/Overwatch"))
DUMP = RESEARCH / "dumps" / "ingame_7FF788F10000.exe"  # image dump rebased to 0x7FF788F10000
GAME_DIR = RESEARCH / "game" / "_retail_"
GAME_EXE = GAME_DIR / "Overwatch.debug.exe"  # the runnable debug build (and the exe the dump ran)
ANALYSIS_DIR = RESEARCH / "docs" / "analysis"  # name/comment maps, VAs at 0x140000000
OBFUSCATED_JSON = ANALYSIS_DIR / "obfuscated_functions.json"  # obfuscation census
RESEARCH_TOOLS = RESEARCH / "tools"  # dumpfix.py, ida_apply_rose.py

# --- this repo
ORPHEUS_EXE = Path(os.environ.get("ORPHEUS_BIN", ORPHEUS / "target" / "release" / "orpheus.exe"))
RELAY_DLL = ORPHEUS / "target" / "release" / "owwfd_relay.dll"
OWLIB_TYPES = ORPHEUS / "crates" / "stu" / "data" / "types.txt"
CODECS_JSON = Path(os.environ.get("OW_CODECS_JSON",
                                  ORPHEUS / "crates" / "fixed" / "data" / "statescript_codecs.json"))

# --- external programs
IDA_DIR = Path(os.environ.get("IDADIR", "D:/Program Files/IDA 9.3"))
ROSE_BIN = Path(os.environ.get("ROSE_BIN", "D:/Work/Coding/rose/rose-bin/target/release/rose-bin.exe"))

# --- generated (git-ignored)
OUT = ORPHEUS / "research" / "out"
INDEX = OUT / "index" / "ow_index.sqlite"
IDA_GAME = OUT / "ida_game"  # game IDB, its input image, Rose results, build state and log
OWDBG_OUT = OUT / "owdbg"  # owdbg reports, minidumps, JSONL logs
OWSEARCH = OUT / "owsearch"  # owsearch.sqlite: code constants, branches, console variables
# Shared ORPHEUS state folder for `orpheus script query` (--state): its cache/ holds the derived game data, so only
# the first query pays the ~240 s derivation. Never the user's own state folder (%APPDATA%/orpheus).
ORPHEUS_STATE = OUT / "orpheus_state"
# The game install for `orpheus ... --game` (env ORPHEUS_GAME).
ORPHEUS_GAME = Path(os.environ.get("ORPHEUS_GAME", RESEARCH / "game"))


def research_tool(name):
    """Import RESEARCH_TOOLS/<name>.py (a plain script there, not a package) as a module."""
    key = "research_tools." + name
    if key in sys.modules:
        return sys.modules[key]
    path = RESEARCH_TOOLS / (name + ".py")
    if not path.exists():
        raise FileNotFoundError(f"{path} not found (set ORPHEUS_RESEARCH_DIR to the research repo)")
    spec = importlib.util.spec_from_file_location(key, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[key] = mod
    spec.loader.exec_module(mod)
    return mod
