"""Searches over the client's code and data that the index doesn't answer (built by tools/owsearch/build.py
into research/out/owsearch/owsearch.sqlite):

    python -m tools.owsearch const 0x1C6EC712 | 0x0D800000000004AA | 068A.025 | 04AA
    python -m tools.owsearch guid 0x0240000000000307
    python -m tools.owsearch cvar 027006C2
    python -m tools.owsearch read 0x7FF78BD46748 --qword

    from tools.owsearch import consts, cvars, guid
    consts.search("0x1C6EC712")["hits"]; cvars.lookup(connect(), 0x027006C2); guid.describe(0x0240000000000307)
"""

import sqlite3

from tools import paths

DB = paths.OWSEARCH / "owsearch.sqlite"


def connect(path=None):
    """Read-only connection to the search database."""
    path = path or DB
    if not path.exists():
        raise FileNotFoundError(f"{path}: build it with `python -m tools.owsearch.build` (Lucy root)")
    return sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True, check_same_thread=False)
