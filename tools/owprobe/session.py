"""A game session for the probe: `orpheus play --probe` on a temporary copy of the state folder (never the user's
own), the decrypted debug build as the game, the login and, on request, the way into the Practice Range by
clicks (owdbg's RangeEntry positions). One game at a time: it takes the agents' game lock (tools/gamelock.py)
and owdbg's machine-wide byte lock, and at its end stops the game, then the server, and releases both.

    with Session(enter_range=True, hero="Soldier") as s:
        p = s.probe                    # a connected tools.owprobe.Probe, frame sampler on
        ...
"""
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

from tools import paths
from tools.owdbg import win32 as w
from tools.owdbg.game import STATE_ITEMS, USER_STATE, _GameLock, capture, find_window, post_click, post_key

from .client import Probe
from .screenshot import screenshot


def _pids_of(image):
    """The PIDs of the running processes with this image name."""
    out = subprocess.run(["tasklist", "/FO", "CSV", "/NH", "/FI", f"IMAGENAME eq {image}"], capture_output=True,
                         text=True).stdout
    pids = []
    for line in out.splitlines():
        fields = [f.strip('"') for f in line.split('","')]
        if len(fields) > 1 and fields[0].strip('"').lower() == image.lower() and fields[1].isdigit():
            pids.append(int(fields[1]))
    return pids


def _terminate(pids, wait):
    """TerminateProcess each, then wait up to `wait` s in all for them to be gone; returns those still running."""
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    kernel32.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    terminate_and_wait = 0x0001 | 0x00100000  # PROCESS_TERMINATE | SYNCHRONIZE
    handles = {}
    for pid in pids:
        handle = kernel32.OpenProcess(terminate_and_wait, False, pid)
        if handle:
            kernel32.TerminateProcess(handle, 1)
            handles[pid] = handle
    end = time.monotonic() + wait
    left = []
    for pid, handle in handles.items():
        remaining = max(0, int((end - time.monotonic()) * 1000))
        if kernel32.WaitForSingleObject(handle, remaining) != 0:
            left.append(pid)
        kernel32.CloseHandle(handle)
    return left


ORPHEUS_EXE = Path(os.environ.get("ORPHEUS_BIN", paths.ORPHEUS_EXE))
GAME_EXE = paths.GAME_EXE  # the decrypted debug build: code hooks need a writable image
LOBBY_PORT = 3724

# owdbg's RangeEntry positions (client fractions, measured at 1584x891).
TRAINING = (0.0789, 0.2974)
PRACTICE_RANGE = (0.5, 0.5)
SELECT = (0.5, 0.935)
LOGIN_PIXEL = (0.44, 0.88)

# Hero select is on screen while #1962 of the local player's `288B` is on: its st66 sets it to
# `#31435e && !#7265e && !STU_EE3E3594()` and st92 presents the screen 008C.05A under it
# (`orpheus script query show 288B`). Only the local player's controller runs 288B. #1962 is a StateStack's
# output, so its value is the bound one (statescript.store_variables); checked in the Range 2026-11-20: true
# while the screen shows, false once the pick closes it (ClientProbe.md "ProbeDll3").
HERO_SELECT_GRAPH = "288B"
SCREEN_SHOWN = 1962
# The player's entity variables after a pick: #2114e on once the pick is locked in, #940e the hero.
PICKED, PICKED_HERO = 2114, 940


class Session:
    def __init__(self, orpheus=None, game=None, port=None, enter_range=False, hero=None, account="Researcher",
                 keep_state=False, log=None, game_args=(), timeout=300.0, seed_state=None, rust_log=None,
                 team_lock=True, owner=None, play_args=()):
        """seed_state: the state folder whose cache (derived game data) and profiles the temp state copies
        (default the user's %APPDATA%/orpheus, read only); a cache from another code version is derived again
        on the first start (minutes, before the game starts). rust_log: the server's RUST_LOG, e.g.
        `info,orpheus::pose=debug` for the per-frame poses `compare` reads from `state/play.log`. team_lock: take
        the agents' game lock (`tools/gamelock.py`, in turn, as `owner`) for the session and release it
        at its end; False when the caller holds it already. play_args: more `orpheus play` options, e.g.
        ("--workshop", FILE)."""
        self.seed_state = Path(seed_state) if seed_state else USER_STATE
        self.rust_log = rust_log
        self.team_lock = team_lock
        self.owner = owner or os.environ.get("ORPHEUS_AGENT") or "owprobe"
        self._team_locked = False
        self.orpheus = Path(orpheus or ORPHEUS_EXE)
        self.game = Path(game or GAME_EXE)
        self.port = port or 21150
        self.enter_range = enter_range
        self.hero = hero
        self.account = account
        self.keep_state = keep_state
        self.log = log or (lambda message: print(message, flush=True))
        self.game_args = list(game_args)
        self.play_args = list(play_args)
        self._hero_select = None
        self.timeout = timeout
        self.proc = None
        self.job = None
        self.state = None
        self.probe = None
        self.pid = None
        self.t0 = time.monotonic()
        self._lock = _GameLock(self.log)
        self._offsets = {}

    # ---- lifetime
    def __enter__(self):
        if self.team_lock:
            self._take_team_lock()
        try:
            self._lock.acquire()
            self.start()
            if self.enter_range:
                self.enter_practice_range()
        except BaseException:
            self.close()
            raise
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def _take_team_lock(self):
        """The agents' game lock, taken in turn (`tools/gamelock.py acquire`: a queue, stale-lock takeover)."""
        import argparse
        from tools import gamelock
        self.log(f"[owprobe] queueing for the game lock as {self.owner} (tools/gamelock.py)")
        gamelock.acquire(argparse.Namespace(name=self.owner, stale=25))
        self._team_locked = True

    def _release_team_lock(self):
        if not self._team_locked:
            return
        import argparse
        from tools import gamelock
        try:
            gamelock.release(argparse.Namespace(name=self.owner))
        except SystemExit as error:
            self.log(f"[owprobe] could not release the game lock: {error}")
        self._team_locked = False

    def elapsed(self):
        return time.monotonic() - self.t0

    def start(self):
        if not self.orpheus.is_file():
            raise RuntimeError(f"{self.orpheus} is missing (set ORPHEUS_BIN; owprobe.dll must be next to it)")
        if not (self.orpheus.parent / "owprobe.dll").is_file():
            raise RuntimeError(f"owprobe.dll is not next to {self.orpheus}: cargo build --release -p orpheus-probe")
        # A game the previous lock holder closed can take a while to exit, and orpheus play refuses to start
        # beside it: wait for it (it is not this session's to kill).
        self._earlier = set(_pids_of(self.game.name))
        end = time.monotonic() + 120
        while self._earlier and time.monotonic() < end:
            self.log(f"[owprobe] waiting for an earlier game to exit: {sorted(self._earlier)}")
            time.sleep(5)
            self._earlier = set(_pids_of(self.game.name))
        self.state = Path(tempfile.mkdtemp(prefix="owprobe-orpheus-"))
        for item in STATE_ITEMS:
            src = self.seed_state / item
            if src.is_dir():
                shutil.copytree(src, self.state / item)
            elif src.is_file():
                shutil.copy2(src, self.state / item)
        cmd = [str(self.orpheus), "--state", str(self.state), "play", "--game", str(self.game), "--account",
               self.account, "--locale", "enUS", "--probe", str(self.port), "--timeout", "120", *self.play_args,
               "--", "--tank_NoFullScreen=1", *self.game_args]
        self.log(f"[owprobe] {' '.join(cmd)}")
        self.out = open(self.state / "play.log", "wb")
        env = dict(os.environ)
        env.pop("ORPHEUS_PROBE", None)
        if self.rust_log:
            env["RUST_LOG"] = self.rust_log
        self.proc = subprocess.Popen(cmd, cwd=str(self.orpheus.parent), stdout=self.out, stderr=subprocess.STDOUT,
                                     stdin=subprocess.DEVNULL, env=env,
                                     creationflags=w.CREATE_NEW_PROCESS_GROUP | w.CREATE_NO_WINDOW)
        # The game is started by orpheus: both die with this job (when Python dies too).
        self.job = w.kill_on_close_job(int(self.proc._handle))
        # The server derives the game data first when the seed's cache is stale: wait for it.
        end = time.monotonic() + max(self.timeout, 900)
        while True:
            if self.proc.poll() is not None:
                raise RuntimeError(f"orpheus play exited ({self.proc.returncode}); see {self.state / 'play.log'}")
            try:
                self.probe = Probe(self.port, timeout=10)
                # Right at the start the client can stall for seconds (its protector): ask again.
                self.pid = self.probe.info()["pid"]
                self.probe.sock.settimeout(30)
                break
            except OSError:
                if self.probe is not None:
                    self.probe.close()
                    self.probe = None
                if time.monotonic() > end:
                    raise
                time.sleep(1)
        self.log(f"[owprobe] probe up in PID {self.pid} at t={self.elapsed():.0f}s; state {self.state}")
        self.wait_login()

    def close(self):
        """Stop the game first (TerminateProcess, waiting until it is gone), then the server, then release the
        locks. The game is this session's PID and any other process of the game's image (the session holds the
        game lock, so they are its own: the client can restart itself outside orpheus's process tree)."""
        if self.probe is not None:
            try:
                self.probe.close()
            except OSError:
                pass
            self.probe = None
        if self.proc is not None or self.pid:
            pids = set(_pids_of(self.game.name)) - getattr(self, "_earlier", set())
            if self.pid:
                pids.add(self.pid)
            started = time.monotonic()
            left = _terminate(pids, wait=30.0)
            if pids:
                self.log(f"[owprobe] game {sorted(pids)} stopped in {time.monotonic() - started:.1f} s"
                         + (f"; still running: {left}" if left else ""))
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
                self.log(f"[owprobe] state kept: {self.state}")
            else:
                shutil.rmtree(self.state, ignore_errors=True)
        self._lock.release()
        self._release_team_lock()

    # ---- logs and the window
    def text(self, name):
        """New text of a log since the last call (play.log: the server's output; logs/relay.log)."""
        path = self.state / name
        offset = self._offsets.get(name, 0)
        try:
            with open(path, "rb") as f:
                f.seek(offset)
                data = f.read()
        except OSError:
            return ""
        cut = data.rfind(b"\n") + 1
        self._offsets[name] = offset + cut
        return data[:cut].decode("utf-8", "replace")

    def window(self):
        return find_window(self.pid)

    def click(self, frac, what):
        hwnd = self.window()
        if not hwnd:
            return False
        width, height, _ = capture(hwnd)
        post_click(hwnd, int(width * frac[0]), int(height * frac[1]))
        self.log(f"[owprobe] t={self.elapsed():.0f}s clicked {what}")
        return True

    def pixel(self, frac):
        hwnd = self.window()
        if not hwnd:
            return None
        try:
            width, height, rgb = capture(hwnd)
        except RuntimeError:
            return None
        return rgb(int(width * frac[0]), int(height * frac[1]))

    # ---- login and the Practice Range
    def wait_login(self):
        """Until the relay pipes the client's Battle.net stream to the server; clicks LOGIN on the login failed
        screen (yellow button) when the client's first attempt fails."""
        end = time.monotonic() + self.timeout
        relay = ""
        clicks, next_click = 0, 0.0
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                raise RuntimeError(f"orpheus play exited ({self.proc.returncode}); see {self.state / 'play.log'}")
            relay += self.text("logs/relay.log")
            if "relay CONNECTED" in relay:
                self.log(f"[owprobe] logged in (t={self.elapsed():.0f}s)")
                return
            px = self.pixel(LOGIN_PIXEL)
            if px and clicks < 8 and time.monotonic() > next_click:
                r, g, b = px
                if r > 140 and 0.6 * r < g < 0.85 * r and b < 0.4 * r:
                    self.click((0.5, LOGIN_PIXEL[1]), "LOGIN")
                    clicks += 1
                    next_click = time.monotonic() + 10
            time.sleep(1)
        raise RuntimeError("no login within the timeout")

    def _hero_select_instance(self):
        """The local player's screen `288B` instance (cached while it lives), or None."""
        from . import statescript as ss
        p = self.probe
        inst = self._hero_select
        if inst is None or p.read_u64(inst["address"] + ss.INSTANCE_GRAPH) != inst["guid"]:
            found = ss.find_instances(p, HERO_SELECT_GRAPH)
            inst = self._hero_select = min(found, key=lambda i: i["id"]) if found else None
        return inst

    def hero_select_shown(self):
        """Whether the client shows hero select (`SCREEN_SHOWN` of its `288B`), read from its memory; False
        while it has no 288B. The screen's 288B is the first (lowest id, the network's) of the local player's;
        the others are its team list entries (st127 makes one per teammate's `134F`, ids from 65535 down)."""
        from . import statescript as ss
        inst = self._hero_select_instance()
        return bool(inst) and ss.variables(self.probe, inst["address"], {SCREEN_SHOWN}).get(SCREEN_SHOWN) is True

    def picked_hero(self):
        """The hero the client has locked in (its `#940e`, a hero GUID such as 0x02E000000000012F), once its
        `#2114e` is on (288B rid114 on the server's `06E4`, rid237 when `#940e == #13161e`; st47 turns it off
        while hero select is open); else None."""
        from . import statescript as ss
        inst = self._hero_select_instance()
        if not inst:
            return None
        values = ss.host_entity_variables(self.probe, inst["host"], {PICKED, PICKED_HERO})
        hero = values.get(PICKED_HERO)
        if values.get(PICKED) is not True or not isinstance(hero, dict) or hero.get("type") != 4:
            return None
        return int.from_bytes(bytes.fromhex(hero["hex"])[:8], "little")

    def enter_practice_range(self):
        """Main menu TRAINING > PRACTICE RANGE, hero select (the hero, SELECT), then waits for input samples
        and our body's position from the frame sampler."""
        server = ""
        for attempt in range(2):
            time.sleep(25 if attempt == 0 else 5)
            self.click(TRAINING, "TRAINING")
            time.sleep(8)
            self.click(PRACTICE_RANGE, "PRACTICE RANGE")
            end = time.monotonic() + 60
            while time.monotonic() < end and "[MM] Match found" not in server:
                server += self.text("play.log")
                time.sleep(1)
            if "[MM] Match found" in server:
                break
            hwnd = self.window()
            if hwnd:
                post_key(hwnd, 0x1B)
            self.log("[owprobe] no match yet: retrying TRAINING > PRACTICE RANGE")
        else:
            raise RuntimeError("the Practice Range did not start")
        self.log(f"[owprobe] t={self.elapsed():.0f}s match found; hero select")
        if not self.pick_hero():
            shot = self.state / "no-pick.png"
            screenshot(self.pid, shot)
            raise RuntimeError(f"hero select: no pick taken; see {shot}")
        # The body the player possesses (component 44, from the probe's component scan) with its mover. The
        # client makes the player's component 44 late: 100-115 s after the pick in the Range (2026-11-20), while
        # it already shows the body.
        end = time.monotonic() + 240
        while time.monotonic() < end:
            info = self.probe.info()
            if info["frames"] and info.get("body") and info["comp31"]:
                self.log(f"[owprobe] t={self.elapsed():.0f}s in the Range: player {info['entity_id']:08X}, body "
                         f"{info['body']:08X}, mover {info['comp31']:#x}, {info['frames']} input samples")
                return
            time.sleep(1)
        shot = self.state / "no-body.png"
        screenshot(self.pid, shot)
        raise RuntimeError(f"no possessed body with a mover after hero select (frames {info['frames']}); see {shot}")

    def pick_hero(self, hero=None, timeout=60.0):
        """Hero select: waits for it, clicks the hero's portrait (`hero` or the session's; none keeps the one
        shown), then SELECT, until the client has locked a hero in and closed the screen (`picked_hero`).
        Returns that hero's GUID, None on the timeout. It also turns the frame sampler on (no recording), which
        finds the body later."""
        hero = hero or self.hero
        hero_at = None
        if hero:
            from tools.owdbg.actions import _hero
            hero_at = _hero(hero)[1]
        self.probe.sampler(every=0)
        end = time.monotonic() + timeout
        clicked_hero, selects = False, 0
        while time.monotonic() < end:
            shown = self.hero_select_shown()
            picked = None if shown else self.picked_hero()
            if picked:
                self.log(f"[owprobe] t={self.elapsed():.0f}s picked hero {picked:016X}")
                return picked
            if shown:
                if hero_at and not clicked_hero:
                    self.click(hero_at, f"{hero}'s portrait")
                    clicked_hero = True
                elif selects < 6:
                    self.click(SELECT, "SELECT")
                    selects += 1
            time.sleep(1.5)
        return None

    def server_log_matches(self, pattern):
        return re.findall(pattern, (self.state / "play.log").read_text("utf-8", "replace"))
