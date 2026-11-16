"""idalib bootstrap and the database lock shared by build.py and open_game_idb()."""
import msvcrt
import os
import sys
import time

from . import paths

_opened = None  # path of the database this process has open (idalib: one per process)


def ensure():
    """Import idapro (it must come before any ida_* module). Uses IDA 9.3's bundled package, so
    the system Python 3.11 works without installing anything."""
    if "idapro" in sys.modules:
        return sys.modules["idapro"]
    pkg = os.path.join(paths.IDA_DIR, "idalib", "python")
    if pkg not in sys.path:
        sys.path.insert(0, pkg)
    import idapro
    return idapro


class DbLock:
    """Exclusive OS lock on <idb>.lock (msvcrt byte lock; released by Windows when the process
    dies). idalib unpacks the .i64 into .id0/.id1/.nam/.til next to it while it is open, so only
    one process may have the database open at a time."""

    def __init__(self, path=paths.LOCK, timeout=0.0):
        self.path, self.timeout, self.fh = path, timeout, None

    def __enter__(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        self.fh = open(self.path, "a+b")
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                self.fh.seek(0)
                msvcrt.locking(self.fh.fileno(), msvcrt.LK_NBLCK, 1)
                break
            except OSError:
                if time.monotonic() >= deadline:
                    self.fh.close()
                    self.fh = None
                    raise RuntimeError(
                        "%s is in use by another process (build running or another "
                        "open_game_idb()); retry later or pass timeout=" % paths.IDB)
                time.sleep(1.0)
        self.fh.seek(0)
        self.fh.truncate()
        self.fh.write(("pid %d\n" % os.getpid()).encode())
        self.fh.flush()
        return self

    def __exit__(self, *exc):
        if self.fh:
            self.fh.seek(0)
            msvcrt.locking(self.fh.fileno(), msvcrt.LK_UNLCK, 1)
            self.fh.close()
            self.fh = None


def open_db(path, input_file=None):
    """Open `path` (an .i64) or, when it does not exist yet, create it from `input_file`.
    Auto-analysis is not waited for here."""
    global _opened
    idapro = ensure()
    if _opened is not None:
        raise RuntimeError("idalib already has %s open in this process" % _opened)
    if os.path.exists(path):
        rc = idapro.open_database(path, False)
    else:
        rc = idapro.open_database(input_file, False, args="-o" + path)
    if rc != 0:
        raise RuntimeError("idalib open_database(%s) failed: %d" % (path, rc))
    _opened = path


def close_db(save=False):
    global _opened
    idapro = ensure()
    idapro.close_database(save)
    _opened = None
