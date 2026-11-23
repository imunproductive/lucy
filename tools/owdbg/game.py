"""Starting the game the same way for every owdbg CLI: Lucy `serve` on a temp copy of the user's
state folder, then Overwatch.debug.exe (windowed) under the debugger with Lucy's relay DLL loaded
at the system breakpoint. Everything is terminated on exit (also on Ctrl+C).

    parser = argparse.ArgumentParser(); add_session_args(parser)
    with Session(args) as s:          # starts the server (unless --no-server / --exe)
        s.dbg.on(...); s.start()      # creates the process under s.dbg
        s.dbg.run(duration=args.duration)
"""
import ctypes
import os
import shutil
import socket
import subprocess
import tempfile
import time
from ctypes import wintypes as wt
from pathlib import Path

from tools import paths

from . import win32 as w
from .core import Debugger
from .names import Names, LUCY_BASE
from .pe import open_pe

GAME_DIR = paths.GAME_DIR
GAME_EXE = paths.GAME_EXE
SERVER_GAME = GAME_DIR / "Overwatch.exe"  # the server reads its game data from this install
LUCY_EXE = paths.LUCY_EXE
RELAY_DLL = paths.RELAY_DLL
PROBE_DLL = paths.PROBE_DLL
RELAY_LOG = RELAY_DLL.parent / "log" / "wfd.log"
# The state folder each session copies (cache, profiles); OWDBG_STATE_SOURCE names another one, e.g.
# a folder whose cache this checkout's build derived, so the session doesn't derive it again.
USER_STATE = Path(os.environ.get("OWDBG_STATE_SOURCE") or Path(os.environ.get("APPDATA", "")) / "lucy")
STATE_ITEMS = ("cache", "profiles", "profile.json")
GAME_ARGS = ["--BNetServer=127.0.0.1:1119", "--tank_Locale=enUS", "--tank_NoFullScreen=1"]
LOBBY_PORT = 3724
OUT_DIR = paths.OWDBG_OUT  # git-ignored
# One owdbg game at a time on this PC, across checkouts and worktrees (outside every repo).
GAME_LOCK = Path(os.environ.get("OWDBG_GAME_LOCK", Path(tempfile.gettempdir()) / "owdbg-game.lock"))


def out_path(tool, suffix):
    """Default output file research/out/owdbg/<tool>-<timestamp><suffix>."""
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    return OUT_DIR / f"{tool}-{time.strftime('%Y%m%d-%H%M%S')}{suffix}"


def add_session_args(p, duration=None):
    g = p.add_argument_group("session")
    g.add_argument("--exe", help="debug another program instead of the game (no server, no relay)")
    g.add_argument("--exe-args", nargs="*", default=[], help="arguments for --exe")
    g.add_argument("--no-server", action="store_true", help="do not start Lucy serve (use a running one)")
    g.add_argument("--no-relay", action="store_true", help="do not load owwfd_relay.dll")
    g.add_argument("--probe", type=int, nargs="?", const=21150, metavar="PORT",
                   help="also load owprobe.dll (tools/owprobe), listening on 127.0.0.1:PORT (default 21150)")
    g.add_argument("--account", default="Researcher", help="Lucy account (default Researcher)")
    g.add_argument("--game-arg", action="append", default=[], help="extra game argument (repeatable)")
    g.add_argument("--keep-state", action="store_true", help="keep the temp Lucy state folder (logs)")
    g.add_argument("--duration", type=float, default=duration,
                   help="seconds to run before terminating everything" + (f" (default {duration})" if duration else ""))
    g.add_argument("--no-login", action="store_true",
                   help="do not click LOGIN when the first Battle.net attempt fails")
    g.add_argument("--no-index", action="store_true", help=f"do not load the index {paths.INDEX}")
    g.add_argument("--hw-delay", type=float,
                   help="write debug registers only N seconds after start (default 8 for the game: "
                        "the protector rewrites them during startup; 0 for --exe)")


def port_open(port, host="127.0.0.1"):
    with socket.socket() as s:
        s.settimeout(0.2)
        return s.connect_ex((host, port)) == 0


class Server:
    """Lucy `serve` with a temp copy of the user's state folder (never the user's own)."""

    def __init__(self, account="Researcher", log=print, keep_state=False, env=None):
        """env: the server's environment (None: ours)."""
        self.account, self.log, self.keep_state, self.env = account, log, keep_state, env
        self.proc = None
        self.state = None
        self.job = None

    def start(self, timeout=None):
        # A derivation version bump makes the first start re-derive the copied cache (minutes).
        timeout = timeout or float(os.environ.get("OWDBG_SERVER_TIMEOUT", "600"))
        if port_open(LOBBY_PORT):
            raise RuntimeError(f"TCP {LOBBY_PORT} is already listening (another Lucy server?); "
                               "stop it or pass --no-server")
        self.state = Path(tempfile.mkdtemp(prefix="owdbg-lucy-"))
        for item in STATE_ITEMS:
            src = USER_STATE / item
            if src.is_dir():
                shutil.copytree(src, self.state / item)
            elif src.is_file():
                shutil.copy2(src, self.state / item)
        cmd = [str(LUCY_EXE), "serve", "--state", str(self.state), "--game", str(SERVER_GAME),
               "--account", self.account]
        self.log(f"[owdbg] server: {' '.join(cmd)}")
        self.out = open(self.state / "serve.log", "wb")
        self.proc = subprocess.Popen(cmd, cwd=str(LUCY_EXE.parent), stdout=self.out, stderr=subprocess.STDOUT,
                                     stdin=subprocess.DEVNULL, env=self.env,
                                     creationflags=w.CREATE_NEW_PROCESS_GROUP | w.CREATE_NO_WINDOW)
        # Kill the server with us even if Python dies.
        self.job = w.kill_on_close_job(int(self.proc._handle))
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                raise RuntimeError(f"lucy serve exited with {self.proc.returncode}; log {self.state / 'serve.log'}")
            if port_open(LOBBY_PORT):
                self.log(f"[owdbg] server ready (TCP {LOBBY_PORT}), state {self.state}")
                return
            time.sleep(0.25)
        raise RuntimeError(f"lucy serve did not listen on {LOBBY_PORT} within {timeout} s")

    def stop(self):
        if self.proc is not None and self.proc.poll() is None:
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(self.proc.pid)], capture_output=True)
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        if self.job:
            w.CloseHandle(self.job)
            self.job = None
        if getattr(self, "out", None):
            self.out.close()
        if self.state is not None:
            if self.keep_state:
                self.log(f"[owdbg] server state kept: {self.state}")
            else:
                shutil.rmtree(self.state, ignore_errors=True)


class _GameLock:
    """One game at a time on this PC: an OS byte lock on GAME_LOCK, shared by every checkout and
    worktree (the game and the server need fixed ports). Waits up to OWDBG_LOCK_WAIT seconds
    (default 3600); Windows releases the lock when the process dies."""

    def __init__(self, log):
        self.log, self.fh = log, None

    def acquire(self):
        import msvcrt
        import time
        GAME_LOCK.parent.mkdir(parents=True, exist_ok=True)
        self.fh = open(GAME_LOCK, "a+b")
        deadline = time.monotonic() + float(os.environ.get("OWDBG_LOCK_WAIT", "3600"))
        told = False
        while True:
            try:
                self.fh.seek(0)
                msvcrt.locking(self.fh.fileno(), msvcrt.LK_NBLCK, 1)
                return
            except OSError:
                if time.monotonic() > deadline:
                    self.fh.close()
                    raise RuntimeError(f"another owdbg game run holds {GAME_LOCK}")
                if not told:
                    self.log(f"[owdbg] waiting for the game lock {GAME_LOCK} (another run is using the game)")
                    told = True
                time.sleep(5)

    def release(self):
        import msvcrt
        if self.fh is not None:
            try:
                self.fh.seek(0)
                msvcrt.locking(self.fh.fileno(), msvcrt.LK_UNLCK, 1)
            except OSError:
                pass
            self.fh.close()
            self.fh = None


class Session:
    """One debugging session: server (game only), debugger, relay injection, cleanup."""

    def __init__(self, args, log=None):
        self.args = args
        self.log = log or (lambda m: print(m, flush=True))
        self.is_game = not args.exe
        self.exe = Path(args.exe).resolve() if args.exe else GAME_EXE
        self.dbg = Debugger(log=self.log)
        self.names = Names(self.dbg, open_pe(self.exe), use_index=not args.no_index)
        self.dbg.hw_hold = args.hw_delay if args.hw_delay is not None else (8.0 if self.is_game else 0.0)
        self.server = None
        self.relay_base = None
        self.relay_log_offset = None
        self._relay_thread = None
        self._login = {"done": False, "clicks": 0, "next": 0}

    def __enter__(self):
        if self.is_game:
            self._lock = _GameLock(self.log)
            self._lock.acquire()
        try:
            if self.is_game and not self.args.no_server:
                self.server = Server(self.args.account, self.log, self.args.keep_state,
                                     env=getattr(self.args, "server_env", None))
                try:
                    self.server.start()
                except BaseException:
                    self.server.stop()
                    raise
        except BaseException:
            self._release_lock()
            raise
        return self

    def __exit__(self, *exc):
        try:
            self.close()
        finally:
            self._release_lock()
        return False

    def _release_lock(self):
        lock = getattr(self, "_lock", None)
        if lock is not None:
            lock.release()
            self._lock = None

    def close(self):
        try:
            if self.dbg.alive:
                self.dbg.terminate()
                self.log(f"[owdbg] process terminated (exit code {self.dbg.exit_code})")
        finally:
            if self.server is not None:
                self.server.stop()
                self.server = None

    def start(self):
        """Create the process under the debugger. Hooks registered before this run from the start."""
        dbg = self.dbg
        if self.is_game:
            if not self.args.no_relay:
                dbg.on("system_breakpoint", self._inject_relay)
                dbg.on("load_dll", self._relay_loaded)
                self.relay_log_offset = RELAY_LOG.stat().st_size if RELAY_LOG.exists() else 0
                if not self.args.no_login:
                    dbg.every(3.0, self._auto_login, first=10)
            args = GAME_ARGS + list(self.args.game_arg)
            cwd = str(GAME_DIR)
        else:
            args = list(self.args.exe_args)
            cwd = str(self.exe.parent)
            # Debug registers written at the system breakpoint are lost on the main thread (the
            # loader's NtContinue): write them again at the entry point.
            dbg.on("system_breakpoint", lambda: dbg.add_bp(
                dbg.main.base + dbg.main.pe.entry_rva, lambda hit: dbg.hw_active() and dbg.apply_hw(), oneshot=True))
        env = None
        if self.is_game and getattr(self.args, "probe", None):
            env = dict(os.environ, LUCY_PROBE_PORT=str(self.args.probe),
                       LUCY_PROBE_LOG=str(PROBE_DLL.parent / "log" / "probe.log"))
        pid = dbg.create(str(self.exe), args, cwd, env=env)
        self.log(f"[owdbg] started {self.exe.name} pid {pid}: {' '.join(args)}")
        dbg.on("create_process", lambda: self.log(
            f"[owdbg] image base 0x{dbg.main.base:X} (Lucy base 0x{LUCY_BASE:X})"))
        return pid

    def run(self, duration=None, status_every=15.0):
        """Debug loop with a periodic status line; Ctrl+C stops it. Returns True if the process
        is still alive (the caller's `with` then terminates it)."""
        dbg = self.dbg

        def status():
            s = dbg.stats
            passed = sum(n for (code, first), n in dbg.exceptions.items() if first)
            self.log(f"[owdbg] t={dbg.elapsed:.0f}s threads={len(dbg.threads)} bp_hits={s['bp_hits']} "
                     f"hw_hits={s['hw_hits']} passed_exceptions={passed} bps={len(dbg.bps)}")

        if status_every:
            dbg.every(status_every, status)
        try:
            dbg.run(duration=duration)
        except KeyboardInterrupt:
            self.log("[owdbg] interrupted")
        if not dbg.alive:
            self.log(f"[owdbg] process exited with 0x{(dbg.exit_code or 0) & 0xFFFFFFFF:X}")
        return dbg.alive

    def log_exceptions(self, limit=12):
        """Exceptions passed to the program, by code, and the most frequent sites (no single steps)."""
        self.log("[owdbg] exceptions passed to the program:")
        for k, n in self.dbg.summary().items():
            self.log(f"    {k}: {n}")
        for (code, va), n in self.dbg.exception_sites.most_common(limit):
            self.log(f"      {code:08X} at 0x{va:X} {self.names.text(va)}: {n}")

    # ---- relay
    def _inject_relay(self):
        """CreateRemoteThread(LoadLibraryW, relay) at the system breakpoint: the thread runs once
        the loader lock is free, before the client's first Battle.net connection. The probe
        (--probe) the same way."""
        self._load_dll(RELAY_DLL, "relay")
        if getattr(self.args, "probe", None):
            self._load_dll(PROBE_DLL, "probe")

    def _load_dll(self, dll, what):
        dbg = self.dbg
        path = (str(dll) + "\0").encode("utf-16-le")
        remote = w.VirtualAllocEx(dbg.process, None, len(path), w.MEM_COMMIT | w.MEM_RESERVE, w.PAGE_READWRITE)
        if not remote:
            self.log(f"[owdbg] {what}: VirtualAllocEx failed {w.last_error()}")
            return
        dbg.write(remote, path)
        k32 = w.GetModuleHandleW("kernel32.dll")
        ours = dbg.module("kernel32.dll")
        if ours is None or ours.base != k32:
            self.log(f"[owdbg] {what}: kernel32 base differs in the target; not injecting")
            return
        loader = w.GetProcAddress(k32, b"LoadLibraryW")
        tid = wt.DWORD()
        h = w.CreateRemoteThread(dbg.process, None, 0, loader, remote, 0, ctypes.byref(tid))
        if not h:
            self.log(f"[owdbg] {what}: CreateRemoteThread failed {w.last_error()}")
            return
        w.CloseHandle(h)
        if what == "relay":
            self._relay_thread = tid.value
        self.log(f"[owdbg] {what}: LoadLibraryW thread {tid.value} created")

    def _relay_loaded(self, mod):
        if mod.path and os.path.basename(mod.path).lower() == RELAY_DLL.name.lower():
            self.relay_base = mod.base
            self.log(f"[owdbg] relay loaded at 0x{mod.base:X} (t={self.dbg.elapsed:.1f}s)")

    def relay_log(self):
        """Relay log text written since this session started the game."""
        if self.relay_log_offset is None or not RELAY_LOG.exists():
            return ""
        with open(RELAY_LOG, "rb") as f:
            f.seek(self.relay_log_offset)
            return f.read().decode("utf-8", "replace")

    def relay_hooked(self):
        """The relay log line showing the client's Battle.net stream went to 127.0.0.1:1119."""
        for line in self.relay_log().splitlines():
            if "BYTE-STREAM" in line and "127.0.0.1" in line:
                return line.strip()
        return None

    @property
    def logged_in(self):
        """The relay piped a Battle.net stream to Lucy (set by the auto-login timer)."""
        return self._login["done"]

    def _auto_login(self):
        """The client's first Battle.net stream usually fails ("SORRY, WE WERE UNABLE TO LOG YOU
        IN. DISCONNECTED"): the relay swaps it but the client never sends on it. Clicking LOGIN
        makes a new stream that the relay pipes to Lucy. Click whenever the yellow LOGIN button
        shows until the relay reports `relay CONNECTED` (every 10 s, at most 8 clicks; clicks in the
        first ~30 s after the failed attempt are ignored by the client)."""
        st = self._login
        if st["done"]:
            return
        text = self.relay_log()
        if "relay CONNECTED" in text:
            st["done"] = True
            self.log(f"[owdbg] relay connected to Lucy (t={self.dbg.elapsed:.1f}s, {st['clicks']} LOGIN clicks)")
            return
        first = next((line for line in text.splitlines() if "BYTE-STREAM" in line), None)
        if first and "port=1119" not in first and not st.get("warned"):
            # The relay keeps the socket class of the first stream it swaps; when that is not the
            # Battle.net stream, no later stream is swapped and the login cannot work.
            st["warned"] = True
            self.log(f"[owdbg] WARNING: the relay swapped a non-Battle.net stream first ({first.strip()}); "
                     "the login will fail in this run (Lucy relay race): restart")
        if st["clicks"] >= 8 or self.dbg.elapsed < st["next"]:
            return
        hwnd = self.window()
        if not hwnd:
            return
        try:
            width, height, rgb = capture(hwnd)
        except RuntimeError:
            return
        y = int(height * 0.88)
        r, g, b = rgb(int(width * 0.44), y)  # inside the button, left of its text
        # The yellow LOGIN button: (231, 175, 56), hovered (163, 114, 42).
        if r > 140 and 0.6 * r < g < 0.85 * r and b < 0.4 * r:
            post_click(hwnd, width // 2, y)
            st["clicks"] += 1
            st["next"] = self.dbg.elapsed + 10
            self.log(f"[owdbg] login failed screen: clicked LOGIN (t={self.dbg.elapsed:.1f}s)")

    # ---- window
    def window(self):
        return find_window(self.dbg.pid)


def find_window(pid):
    """Largest visible top-level window of pid, or None."""
    found = []

    def cb(hwnd, _):
        p = wt.DWORD()
        w.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid and w.IsWindowVisible(hwnd):
            r = wt.RECT()
            w.GetWindowRect(hwnd, ctypes.byref(r))
            found.append(((r.right - r.left) * (r.bottom - r.top), hwnd))
        return True

    w.EnumWindows(w.WNDENUMPROC(cb), 0)
    return max(found)[1] if found else None


def window_title(hwnd):
    buf = ctypes.create_unicode_buffer(256)
    w.GetWindowTextW(hwnd, buf, 256)
    return buf.value


def capture(hwnd):
    """PrintWindow(PW_RENDERFULLCONTENT) of the client area: (width, height, rgb(x, y))."""
    r = wt.RECT()
    w.GetClientRect(hwnd, ctypes.byref(r))
    width, height = r.right - r.left, r.bottom - r.top
    if width <= 0 or height <= 0:
        raise RuntimeError("window has no client area (minimized?)")
    hdc = w.GetDC(hwnd)
    mem = w.CreateCompatibleDC(hdc)
    bmp = w.CreateCompatibleBitmap(hdc, width, height)
    old = w.SelectObject(mem, bmp)
    try:
        w.PrintWindow(hwnd, mem, 1 | 2)  # PW_CLIENTONLY | PW_RENDERFULLCONTENT
        header = (ctypes.c_uint32 * 10)(40, width, (-height) & 0xFFFFFFFF, 1 | (32 << 16), 0, 0, 0, 0, 0, 0)
        buf = ctypes.create_string_buffer(width * height * 4)
        w.GetDIBits(mem, bmp, 0, height, buf, header, 0)
    finally:
        w.SelectObject(mem, old)
        w.DeleteObject(bmp)
        w.DeleteDC(mem)
        w.ReleaseDC(hwnd, hdc)
    data = buf.raw

    def rgb(x, y):
        i = (y * width + x) * 4
        return data[i + 2], data[i + 1], data[i]
    rgb.data = data
    return width, height, rgb


def screenshot(hwnd, path):
    """Client area of the window into a PNG (needs Pillow)."""
    from PIL import Image
    width, height, rgb = capture(hwnd)
    Image.frombuffer("RGBA", (width, height), rgb.data, "raw", "BGRA", 0, 1).convert("RGB").save(path)
    return width, height


WM_KEYDOWN, WM_KEYUP, WM_CHAR = 0x0100, 0x0101, 0x0102
WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP = 0x0200, 0x0201, 0x0202


def post_key(hwnd, vk):
    scan = w.MapVirtualKeyW(vk, 0)
    w.PostMessageW(hwnd, WM_KEYDOWN, vk, 1 | (scan << 16))
    w.PostMessageW(hwnd, WM_KEYUP, vk, 1 | (scan << 16) | (3 << 30))


def post_click(hwnd, x, y):
    lp = (y << 16) | (x & 0xFFFF)
    w.PostMessageW(hwnd, WM_MOUSEMOVE, 0, lp)
    w.PostMessageW(hwnd, WM_LBUTTONDOWN, 1, lp)
    w.PostMessageW(hwnd, WM_LBUTTONUP, 0, lp)


def post_move(hwnd, x, y):
    w.PostMessageW(hwnd, WM_MOUSEMOVE, 0, (y << 16) | (x & 0xFFFF))


_LOG_OFFSETS = {}  # (state dir, log name) -> bytes read; ("found", state dir) -> the line


def match_found(s):
    """The server's first `[MM] Match found` line in <state>/serve.log (its whole output; logs/
    rotates every 2 MB), or None (read incrementally: the log grows by megabytes in a match)."""
    server = s.server
    if server is None or server.state is None:
        return None
    hit = _LOG_OFFSETS.get(("found", server.state))
    if hit:
        return hit
    for path in (server.state / "serve.log",):
        key = (server.state, path.name)
        try:
            with open(path, "rb") as f:
                f.seek(_LOG_OFFSETS.get(key, 0))
                data = f.read()
        except OSError:
            continue
        cut = data.rfind(b"\n") + 1  # keep a partial last line for the next read
        _LOG_OFFSETS[key] = _LOG_OFFSETS.get(key, 0) + cut
        for line in data[:cut].decode("utf-8", "replace").splitlines():
            if "[MM] Match found" in line:
                _LOG_OFFSETS[("found", server.state)] = line.strip()
                return line.strip()
    return None


class RangeEntry:
    """--enter-range: main menu TRAINING > PRACTICE RANGE > SELECT (the preselected hero) by
    PostMessage clicks at relative client coordinates (measured at 1584x891). TRAINING is clicked
    25 s after the login (the main menu needs that long), PRACTICE RANGE 8 s later; the server's
    `[MM] Match found: Practice Range` line confirms; without it after 60 s the sequence is
    retried once (Escape first). In hero select the yellow SELECT button is clicked until it
    disappears; with `hero` ((x, y) client fractions of a portrait) that portrait is clicked first."""

    TRAINING = (0.0789, 0.2974)
    PRACTICE_RANGE = (0.5, 0.5)
    SELECT = (0.5, 0.935)
    SELECT_PIXEL = (0.47, 0.935)  # inside the button, left of its text

    def __init__(self, s, hero=None):
        self.s = s
        self.hero = hero
        self.hero_clicked = False
        self.state = "wait"
        self.at = None
        self.tries = 0
        self.select_clicks = 0

    def start(self):
        self.s.dbg.every(2.0, self.poll, first=2.0)

    def click(self, frac, what):
        hwnd = self.s.window()
        if not hwnd:
            return False
        width, height, _ = capture(hwnd)
        post_click(hwnd, int(width * frac[0]), int(height * frac[1]))
        self.s.log(f"[range] t={self.s.dbg.elapsed:.0f}s clicked {what}")
        return True

    def select_visible(self):
        hwnd = self.s.window()
        if not hwnd:
            return False
        try:
            width, height, rgb = capture(hwnd)
        except RuntimeError:
            return False
        r, g, b = rgb(int(width * self.SELECT_PIXEL[0]), int(height * self.SELECT_PIXEL[1]))
        return r > 180 and 0.65 * r < g < 0.85 * r and b < 0.5 * r  # (213, 159, 89)

    def poll(self):
        t = self.s.dbg.elapsed
        if self.state == "done" or not self.s.logged_in:
            return
        if self.state == "wait":
            self.at, self.state = t + 25, "training"
        elif self.state == "training" and t >= self.at:
            if self.click(self.TRAINING, "TRAINING"):
                self.at, self.state = t + 8, "range"
        elif self.state == "range" and t >= self.at:
            if self.click(self.PRACTICE_RANGE, "PRACTICE RANGE"):
                self.at, self.state = t + 60, "match"
                self.tries += 1
        elif self.state == "match":
            if match_found(self.s):
                self.s.log(f"[range] t={t:.0f}s server: match found (Practice Range); waiting for hero select")
                self.at, self.state = t + 120, "select"
            elif t >= self.at:
                self.snap_screen()
                if self.tries >= 2:
                    self.s.log("[range] no match after 2 tries: enter the Practice Range by hand")
                    self.state = "done"
                    return
                hwnd = self.s.window()
                if hwnd:
                    post_key(hwnd, 0x1B)  # Escape back to the main menu
                self.s.log("[range] no match yet: retrying TRAINING > PRACTICE RANGE")
                self.at, self.state = t + 5, "training"
        elif self.state == "select":
            if self.select_visible():
                if self.hero and not self.hero_clicked:
                    self.click(self.hero, "the hero's portrait")
                    self.hero_clicked = True
                elif self.select_clicks < 6:
                    self.click(self.SELECT, "SELECT (hero select)")
                    self.select_clicks += 1
            elif self.select_clicks:
                self.s.log(f"[range] t={t:.0f}s SELECT gone: spawning in the Practice Range")
                self.state = "done"
            elif t >= self.at:
                self.s.log("[range] hero select not recognized; pick a hero by hand if needed")
                self.state = "done"

    def snap_screen(self):
        """The window as it was when no match came (what the clicks met), for the log."""
        hwnd = self.s.window()
        if not hwnd:
            return
        path = out_path("range-nomatch", ".png")
        try:
            screenshot(hwnd, path)
        except (RuntimeError, ImportError, OSError) as error:
            self.s.log(f"[range] no screenshot: {error}")
            return
        self.s.log(f"[range] screen without a match: {path}")
