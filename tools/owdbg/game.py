"""Starting the game the same way for every owdbg CLI: ORPHEUS `serve` on a temp copy of the user's
state folder, then Overwatch.debug.exe (windowed) under the debugger with ORPHEUS's relay DLL loaded
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
from .names import Names, ORPHEUS_BASE
from .pe import open_pe

GAME_DIR = paths.GAME_DIR
GAME_EXE = paths.GAME_EXE
SERVER_GAME = GAME_DIR / "Overwatch.exe"  # the server reads its game data from this install
ORPHEUS_EXE = paths.ORPHEUS_EXE
RELAY_DLL = paths.RELAY_DLL
RELAY_LOG = RELAY_DLL.parent / "log" / "wfd.log"
USER_STATE = Path(os.environ.get("APPDATA", "")) / "orpheus"
STATE_ITEMS = ("cache", "profiles", "profile.json")
GAME_ARGS = ["--BNetServer=127.0.0.1:1119", "--tank_Locale=enUS", "--tank_NoFullScreen=1"]
LOBBY_PORT = 3724
OUT_DIR = paths.OWDBG_OUT  # git-ignored


def out_path(tool, suffix):
    """Default output file research/out/owdbg/<tool>-<timestamp><suffix>."""
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    return OUT_DIR / f"{tool}-{time.strftime('%Y%m%d-%H%M%S')}{suffix}"


def add_session_args(p, duration=None):
    g = p.add_argument_group("session")
    g.add_argument("--exe", help="debug another program instead of the game (no server, no relay)")
    g.add_argument("--exe-args", nargs="*", default=[], help="arguments for --exe")
    g.add_argument("--no-server", action="store_true", help="do not start ORPHEUS serve (use a running one)")
    g.add_argument("--no-relay", action="store_true", help="do not load owwfd_relay.dll")
    g.add_argument("--account", default="Researcher", help="ORPHEUS account (default Researcher)")
    g.add_argument("--game-arg", action="append", default=[], help="extra game argument (repeatable)")
    g.add_argument("--keep-state", action="store_true", help="keep the temp ORPHEUS state folder (logs)")
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
    """ORPHEUS `serve` with a temp copy of the user's state folder (never the user's own)."""

    def __init__(self, account="Researcher", log=print, keep_state=False, env=None):
        """env: the server's environment (None: ours), e.g. dict(os.environ, ORPHEUS_DEBUG_SEND="1")
        for the marker endpoint (owdbg.msg)."""
        self.account, self.log, self.keep_state, self.env = account, log, keep_state, env
        self.proc = None
        self.state = None
        self.job = None

    def start(self, timeout=90):
        if port_open(LOBBY_PORT):
            raise RuntimeError(f"TCP {LOBBY_PORT} is already listening (another ORPHEUS server?); "
                               "stop it or pass --no-server")
        self.state = Path(tempfile.mkdtemp(prefix="owdbg-orpheus-"))
        for item in STATE_ITEMS:
            src = USER_STATE / item
            if src.is_dir():
                shutil.copytree(src, self.state / item)
            elif src.is_file():
                shutil.copy2(src, self.state / item)
        cmd = [str(ORPHEUS_EXE), "serve", "--state", str(self.state), "--game", str(SERVER_GAME),
               "--account", self.account]
        self.log(f"[owdbg] server: {' '.join(cmd)}")
        self.out = open(self.state / "serve.log", "wb")
        self.proc = subprocess.Popen(cmd, cwd=str(ORPHEUS_EXE.parent), stdout=self.out, stderr=subprocess.STDOUT,
                                     stdin=subprocess.DEVNULL, env=self.env,
                                     creationflags=w.CREATE_NEW_PROCESS_GROUP | w.CREATE_NO_WINDOW)
        # Kill the server with us even if Python dies.
        self.job = w.kill_on_close_job(int(self.proc._handle))
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                raise RuntimeError(f"orpheus serve exited with {self.proc.returncode}; log {self.state / 'serve.log'}")
            if port_open(LOBBY_PORT):
                self.log(f"[owdbg] server ready (TCP {LOBBY_PORT}), state {self.state}")
                return
            time.sleep(0.25)
        raise RuntimeError(f"orpheus serve did not listen on {LOBBY_PORT} within {timeout} s")

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
        if self.is_game and not self.args.no_server:
            self.server = Server(self.args.account, self.log, self.args.keep_state,
                                 env=getattr(self.args, "server_env", None))
            try:
                self.server.start()
            except BaseException:
                self.server.stop()
                raise
        return self

    def __exit__(self, *exc):
        self.close()
        return False

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
        pid = dbg.create(str(self.exe), args, cwd)
        self.log(f"[owdbg] started {self.exe.name} pid {pid}: {' '.join(args)}")
        dbg.on("create_process", lambda: self.log(
            f"[owdbg] image base 0x{dbg.main.base:X} (ORPHEUS base 0x{ORPHEUS_BASE:X})"))
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
        the loader lock is free, before the client's first Battle.net connection."""
        dbg = self.dbg
        path = (str(RELAY_DLL) + "\0").encode("utf-16-le")
        remote = w.VirtualAllocEx(dbg.process, None, len(path), w.MEM_COMMIT | w.MEM_RESERVE, w.PAGE_READWRITE)
        if not remote:
            self.log(f"[owdbg] relay: VirtualAllocEx failed {w.last_error()}")
            return
        dbg.write(remote, path)
        k32 = w.GetModuleHandleW("kernel32.dll")
        ours = dbg.module("kernel32.dll")
        if ours is None or ours.base != k32:
            self.log("[owdbg] relay: kernel32 base differs in the target; not injecting")
            return
        loader = w.GetProcAddress(k32, b"LoadLibraryW")
        tid = wt.DWORD()
        h = w.CreateRemoteThread(dbg.process, None, 0, loader, remote, 0, ctypes.byref(tid))
        if not h:
            self.log(f"[owdbg] relay: CreateRemoteThread failed {w.last_error()}")
            return
        w.CloseHandle(h)
        self._relay_thread = tid.value
        self.log(f"[owdbg] relay: LoadLibraryW thread {tid.value} created")

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
        """The relay piped a Battle.net stream to ORPHEUS (set by the auto-login timer)."""
        return self._login["done"]

    def _auto_login(self):
        """The client's first Battle.net stream usually fails ("SORRY, WE WERE UNABLE TO LOG YOU
        IN. DISCONNECTED"): the relay swaps it but the client never sends on it. Clicking LOGIN
        makes a new stream that the relay pipes to ORPHEUS. Click whenever the yellow LOGIN button
        shows until the relay reports `relay CONNECTED` (every 10 s, at most 8 clicks; clicks in the
        first ~30 s after the failed attempt are ignored by the client)."""
        st = self._login
        if st["done"]:
            return
        text = self.relay_log()
        if "relay CONNECTED" in text:
            st["done"] = True
            self.log(f"[owdbg] relay connected to ORPHEUS (t={self.dbg.elapsed:.1f}s, {st['clicks']} LOGIN clicks)")
            return
        first = next((line for line in text.splitlines() if "BYTE-STREAM" in line), None)
        if first and "port=1119" not in first and not st.get("warned"):
            # The relay keeps the socket class of the first stream it swaps; when that is not the
            # Battle.net stream, no later stream is swapped and the login cannot work.
            st["warned"] = True
            self.log(f"[owdbg] WARNING: the relay swapped a non-Battle.net stream first ({first.strip()}); "
                     "the login will fail in this run (ORPHEUS relay race): restart")
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
