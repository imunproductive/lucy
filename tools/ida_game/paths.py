"""File locations of the game IDB pipeline, as strings (from tools/paths.py; every generated file
lives in research/out/ida_game/, git-ignored)."""
import os

from tools import paths as _p

ORPHEUS = str(_p.ORPHEUS)  # cwd of the detached build
IDA_DIR = str(_p.IDA_DIR)
ROSE_BIN = str(_p.ROSE_BIN)

DUMP = str(_p.DUMP)
GAME_EXE = str(_p.GAME_EXE)  # the exe the dump ran
OBFUSCATED_JSON = str(_p.OBFUSCATED_JSON)
ANALYSIS_DIR = str(_p.ANALYSIS_DIR)
# ORPHEUS's statescript codec table (read only): state property-block offsets/types for `types`
CODECS_JSON = str(_p.CODECS_JSON)

OUT_DIR = str(_p.IDA_GAME)
RESTORED = os.path.join(OUT_DIR, "restored.exe")     # dump + re-encrypted code restored
RESTORE_JSON = os.path.join(OUT_DIR, "restore.json")
ROSE_DIR = os.path.join(OUT_DIR, "rose")             # batch.json, fn/<VA>.json
INPUT_EXE = os.path.join(OUT_DIR, "ow_game.exe")     # restored + Rose patches: what IDA loads
IDB = os.path.join(OUT_DIR, "ow_game.i64")
LOCK = IDB + ".lock"
LOG = os.path.join(OUT_DIR, "build.log")
STATE = os.path.join(OUT_DIR, "build_state.json")
