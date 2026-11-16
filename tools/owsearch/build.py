"""Build research/out/owsearch/owsearch.sqlite (about 15 s):

    python -m tools.owsearch.build [--image PATH] [--dump PATH]

--image: the code image to sweep (default: the Rose-patched research/out/ida_game/ow_game.exe, else the dump);
--dump: the in-game dump the console variables are read from (default tools/paths.py DUMP).
The new database is written next to the old one and replaces it at the end; while another process has
the old one open the replace fails and the old one stays.
"""

import argparse
import json
import os
import sqlite3
import sys
import time
from pathlib import Path

from tools import paths
from tools.owindex.image import Image

from . import DB, consts, cvars


def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m tools.owsearch.build", description=__doc__.split("\n\n")[0])
    ap.add_argument("--image", type=Path, help="code image to sweep (memory layout)")
    ap.add_argument("--dump", type=Path, default=paths.DUMP, help="in-game image dump for the console variables")
    args = ap.parse_args(argv)
    t0 = time.time()
    DB.parent.mkdir(parents=True, exist_ok=True)
    tmp = DB.with_suffix(".sqlite.tmp")
    tmp.unlink(missing_ok=True)
    db = sqlite3.connect(tmp)
    db.execute("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT)")
    dump = Image(args.dump)
    cv = cvars.scan(dump)
    cvars.store(db, cv)
    watch = [(v["rva"], v["rva"] + (v["size"] or 0x28)) for v in cv["vars"]]
    meta = consts.build(db, args.image, watch=watch)
    print(f"cvars: registry {dump.base + cv['registry_rva']:#x}, {cv['count']} variables")
    meta.update({"dump": str(args.dump), "base": dump.base, "cvar_registry_rva": cv["registry_rva"],
                 "cvars": cv["count"], "built": time.strftime("%Y-%m-%d %H:%M:%S")})  # fmt: skip
    db.executemany("INSERT INTO meta VALUES (?, ?)", ((k, json.dumps(v)) for k, v in meta.items()))
    db.commit()
    db.close()
    try:
        os.replace(tmp, DB)
    except PermissionError:
        print(f"{DB} is open in another process: kept the old one; the new one is {tmp}", file=sys.stderr)
        return 1
    print(f"{DB} in {time.time() - t0:.0f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
