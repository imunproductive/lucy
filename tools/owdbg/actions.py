"""In-match action scripts (`--actions`) for every owdbg session command: the debugger writes the
client's own input. Research: D:/Work/Coding/refs/reports/ClientInput.md.

    --actions "wait 3; tap fire; wait 1; jump; ability 1; aim-at-bot; hold fire 2; hero Ana"

How: a breakpoint in the client's input sampler (0x7FF78961B350, at 0x7FF78961C7FC: rdi = the local
player's component 59, the input sample at +0x103C is complete and not yet merged into the command)
ORs button bits into the sample (+0x1050), writes the throttles (+0x104B right, +0x104C forward) and
the action byte (+0x1054). The client predicts with that command and sends it, so the server sees
the same input. The view is turned by calling the client's own set-view function 0x7FF7894E00A0
(component 59, quaternion), the one messages 25006/25008/25009 call, on the sampler's thread:
camera, prediction and command agree. Positions come from component 4 (+0x160, about 1 m above
the feet): a background scan of the heap for component 4's vtable, the owner entity decoded from
+0x8 and its id at entity+0xB0; bots are entities 0x80008800.. (aim-at-bot skips those on a
team-0 spawn point of the server's ai_spawners_174.json), our body is the `body=` of the server's
`orpheus::owner` trace (RUST_LOG is set for it).

The script starts when the match is entered: with --enter-range when the hero select closed, else
at the first input sample after the server's '[MM] Match found', then after a first position scan
(~20 s); with --actions-start login right after the login. Steps (`;` or newlines):

    wait S                     pause S seconds
    tap BUTTON [S]             press for S seconds (default 0.06), then release
    hold BUTTON S              the same, any length (tap and hold block the script)
    press BUTTON / release BUTTON   keep held until released (does not block)
    fire [S]  jump  crouch [S]  melee  reload  interact  ultimate
    ability 1|2                Shift / E
    move DIR S                 DIR forward|back|left|right (or forward-left, ...), S seconds
    look YAW [PITCH]           absolute view, degrees (yaw: forward = (sin, 0, cos); pitch + down)
    turn DYAW [DPITCH]         relative view, degrees
    aim-at-bot [N]             turn to the nearest enemy Training Bot about level with us (N: entity
                               0x80008800+N) and keep
                               tracking it until the next look/turn/aim/`aim off`
    aim off                    stop tracking
    hero NAME                  open hero select (H: action byte 42 = logical button 81 in one input
                               sample), click NAME (and SELECT). NOT WORKING YET in a match: the
                               injected H does not reach graph 288B (ClientInput.md); with
                               --foreground-input it presses H with SendInput. At the start of a
                               session use --hero NAME (picked at the first hero select).
    key N                      put logical button N's action code into one input sample (N one of
                               the action table's buttons: 15-19, 21, 22, 56, 71, 81, 90-109, ...)
    key CHORD                  press a key with modifiers and release it: alt+`, ctrl+shift+f,
                               enter, escape, f1, a, ... (KEY_NAMES); posted to the game window's
                               message queue (WM_KEYDOWN/WM_SYSKEYDOWN with the Alt context bit),
                               with --foreground-input sent with SendInput
    type TEXT                  type TEXT (the rest of the step, spaces kept; no ';'): WM_CHAR per
                               character (SendInput Unicode events with --foreground-input)
    die                        ORPHEUS kills our hero (POST /api/debug/kill: the server runs
                               with ORPHEUS_DEBUG_SEND=1); the respawn follows after the mode's
                               time. Refused in the spawn room (invulnerable there): in the
                               Practice Range walk out first (`move forward 4`)
    screenshot [NAME]          PNG of the window into research/out/owdbg/
    position                   log our position and the bots'
    trace S                    for S seconds (the script goes on meanwhile), log at every input
                               sample the newest command frame in component 59's ring and our
                               body's component 4 position: the client's prediction, frame by frame
    poke ADDRESS TYPE VALUE    write memory: TYPE u8/i8/u16/i16/u32/i32/u64/i64/f32/f64; an address
                               in the game image is an ORPHEUS VA (base 0x7FF788F10000), others as is

BUTTON: fire, fire2 (secondary), ability1, ability2, ultimate, jump, crouch, melee, reload,
interact, or a hex bit mask.
--foreground-input sends buttons and throttles with SendInput to the focused game window instead
(the window is brought to the front; the desktop is busy meanwhile); the view still turns through
set-view.
"""
import ctypes
import json
import math
import os
import re
import struct
import threading
import time

from . import win32 as w
from .names import ORPHEUS_BASE

# ORPHEUS VAs (base 0x7FF788F10000), see ClientInput.md
SAMPLE_SITE = 0x7FF78961C7FC  # in the input sampler 0x7FF78961B350, before the merge 0x7FF789B3D650
SET_VIEW = 0x7FF7894E00A0  # set-view(component 59, float quaternion[4])
POINTER_KEY = 0x7FF78BDF47FC  # component +0x8 = entity ^ rol64([key], 2) ^ POINTER_SALT
POINTER_SALT = 0xF320E42472A94F14
COMP4_VTABLE_RVA = 0x260C1E8  # component 4: position at +0x160 (3 floats, about 1 m above the feet)

# component 59 (the local player's command target)
SAMPLE_YAW, SAMPLE_PITCH = 0x1042, 0x1044  # i16 angles of the sample being built
SAMPLE_RIGHT, SAMPLE_FORWARD = 0x104B, 0x104C  # i8 throttles
SAMPLE_BUTTONS = 0x1050  # u32 command bits
VIEW_YAW, VIEW_PITCH = 0x107A, 0x107C  # i16, the stored view the next sample starts from
COMMAND_RING, COMMAND_RECORD, COMMAND_SLOTS = 0xB60, 36, 32  # {u32 frame; 28 bytes} (ClientInput.md)

ENTITY_ID = 0xB0
COMP4_POSITION = 0x160
BOT_ENTITY = 0x80008800  # orpheus_sim::bots::BOT_ENTITY + slot
UNITS = 65536.0 / (2 * math.pi)

# command bit -> logical button (client 0x7FF78961DDA0, server orpheus_script::timeline)
BUTTONS = {
    "fire": 0x1,  # 20 primary fire
    "fire2": 0x2,  # 29 secondary fire
    "interact": 0x4,  # 30
    "ability1": 0x8,  # 35
    "ability2": 0x10,  # 36
    "ultimate": 0x20,  # 37
    "jump": 0x40,  # 11
    "crouch": 0x80,  # 12
    "reload": 0x400,  # 14
    "melee": 0x800,  # 31
}
ALIASES = {"secondary": "fire2", "shift": "ability1", "e": "ability2", "q": "ultimate", "ult": "ultimate"}
SAMPLE_ACTION = 0x1054  # u8 action byte: the first newly pressed button of the table below
# Action table 0x7FF78BC278F0 (sampler 0x7FF78961DD00): code -> logical button. The command takes
# the byte (merge 0x7FF789B3D650) and 0x7FF789C66AA0 turns it into a press and a release of that
# button in statescript, on the client as on the server. A real press sets it in ONE sample (the
# pressed edge); a byte held for several command frames presses the button once per frame.
ACTION_BUTTONS = (15, 16, 17, 18, 19, 21, 22, 56, 71, 91, 92, 93, 94, 95, 96, 120, 97, 98, 99, 100,
                  101, 102, 103, 104, 105, 106, 107, 108, 133, 134, 135, 136, 137, 138, 139, 140, 141,
                  142, 117, 118, 109, 81, 90, 2, 3)  # codes 1..45
HERO_SELECT_BUTTON = 81  # graph 288B's hero select (H), action code 42
# Default keys for --foreground-input: (scan code, extended) or a mouse button.
KEYS = {"fire": "lmb", "fire2": "rmb", "interact": (0x21, False), "ability1": (0x2A, False),
        "ability2": (0x12, False), "ultimate": (0x10, False), "jump": (0x39, False),
        "crouch": (0x1D, False), "reload": (0x13, False), "melee": (0x2F, False)}
MOVE_KEYS = {"forward": 0x11, "back": 0x1F, "left": 0x1E, "right": 0x20}
HERO_SELECT_KEY = 0x23  # H

# Hero select row (1.74, 31 heroes; client x of the portrait centres as fractions of the width at
# 1584x891, y 0.856): tank, damage and support groups, 0.0285 apart.
HERO_ROW_Y = 0.856
HERO_STEP = 0.0285
HERO_GROUPS = (
    (0.0561, ("dva", "orisa", "reinhardt", "roadhog", "sigma", "winston", "wreckingball", "zarya")),
    (0.2934, ("ashe", "bastion", "cassidy", "doomfist", "echo", "genji", "hanzo", "junkrat", "mei",
              "pharah", "reaper", "soldier76", "sombra", "symmetra", "torbjorn", "tracer", "widowmaker")),
    (0.7857, ("ana", "baptiste", "brigitte", "lucio", "mercy", "moira", "zenyatta")),
)
HERO_NAMES = {"mccree": "cassidy", "soldier": "soldier76", "76": "soldier76", "hammond": "wreckingball",
              "torb": "torbjorn", "widow": "widowmaker", "bap": "baptiste", "zen": "zenyatta"}


POKE_TYPES = {"u8": "B", "i8": "b", "u16": "H", "i16": "h", "u32": "I", "i32": "i", "u64": "Q", "i64": "q",
              "f32": "f", "f64": "d"}

# key CHORD: virtual-key codes. Letters and digits are their ASCII codes, f1-f24 0x70+.
MODIFIER_VK = {"ctrl": 0x11, "control": 0x11, "alt": 0x12, "shift": 0x10}
KEY_NAMES = {"backspace": 0x08, "tab": 0x09, "enter": 0x0D, "return": 0x0D, "escape": 0x1B, "esc": 0x1B,
             "space": 0x20, "pageup": 0x21, "pagedown": 0x22, "end": 0x23, "home": 0x24, "left": 0x25,
             "up": 0x26, "right": 0x27, "down": 0x28, "insert": 0x2D, "delete": 0x2E, "`": 0xC0,
             "backtick": 0xC0, "grave": 0xC0, "tilde": 0xC0, "-": 0xBD, "minus": 0xBD, "=": 0xBB,
             "equals": 0xBB, "[": 0xDB, "]": 0xDD, "\\": 0xDC, "semicolon": 0xBA, "'": 0xDE, ",": 0xBC,
             ".": 0xBE, "/": 0xBF}
EXTENDED_VK = {0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E}
WM_KEYDOWN, WM_KEYUP, WM_CHAR, WM_SYSKEYDOWN, WM_SYSKEYUP = 0x100, 0x101, 0x102, 0x104, 0x105
KEYEVENTF_UNICODE = 0x4


def _chord(text):
    """'alt+`' -> ((VK_MENU,), VK_OEM_3); SystemExit on unknown names."""
    parts = text.lower().split("+") if text not in ("+",) else [text]
    *mods, key = parts
    for m in mods:
        if m not in MODIFIER_VK:
            raise SystemExit(f"--actions: key {text!r}: unknown modifier {m!r} (ctrl, alt, shift)")
    if key in KEY_NAMES:
        vk = KEY_NAMES[key]
    elif len(key) == 1 and key.isalnum():
        vk = ord(key.upper())
    elif re.fullmatch(r"f([1-9]|1[0-9]|2[0-4])", key):
        vk = 0x6F + int(key[1:])
    else:
        raise SystemExit(f"--actions: key {text!r}: unknown key {key!r} (a letter, digit, f1-f24 or "
                         f"{', '.join(KEY_NAMES)})")
    return tuple(dict.fromkeys(MODIFIER_VK[m] for m in mods)), vk


def _key_lparam(vk, up, alt):
    scan = w.MapVirtualKeyW(vk, 0)
    lp = 1 | (scan << 16) | ((1 << 24) if vk in EXTENDED_VK else 0)
    if alt:
        lp |= 1 << 29  # context code: Alt is down
    if up:
        lp |= 3 << 30  # previous state down, transition up
    return lp


def _key_char(mods, vk):
    """The character TranslateMessage makes of the key, None for none (or a Ctrl/Alt chord)."""
    if any(m != MODIFIER_VK["shift"] for m in mods):
        return None
    shift = bool(mods)
    special = {0x0D: "\r", 0x09: "\t", 0x08: "\b", 0x1B: "\x1b", 0x20: " "}
    if vk in special:
        return special[vk]
    if 0x41 <= vk <= 0x5A:
        return chr(vk) if shift else chr(vk).lower()
    if shift:
        return None  # shifted digits and punctuation depend on the layout
    ch = w.MapVirtualKeyW(vk, 2) & 0x7FFF  # MAPVK_VK_TO_CHAR: the unshifted character
    return chr(ch) if ch else None


def post_chord(hwnd, mods, vk, gap=0.03):
    """Modifiers down, the key down (and its WM_CHAR, as TranslateMessage would post it) and up,
    modifiers up, posted to the window as Windows sends them: with Alt down WM_SYSKEY* and the
    context bit, Alt's own release a WM_KEYUP (blocks about gap per message). The client's text
    fields (the console) read the WM_CHAR: Enter submits with it."""
    alt_vk = MODIFIER_VK["alt"]
    alt = False
    events = []
    for m in mods:
        alt = alt or m == alt_vk
        events.append((WM_SYSKEYDOWN if alt else WM_KEYDOWN, m, False, alt))
    events.append((WM_SYSKEYDOWN if alt else WM_KEYDOWN, vk, False, alt))
    ch = _key_char(mods, vk)
    if ch is not None:
        events.append((WM_CHAR, ord(ch), None, False))
    events.append((WM_SYSKEYUP if alt else WM_KEYUP, vk, True, alt))
    for m in reversed(mods):
        if m == alt_vk:
            alt = False
            events.append((WM_KEYUP, m, True, False))
        else:
            events.append((WM_SYSKEYUP if alt else WM_KEYUP, m, True, alt))
    for msg, code, up, ctx in events:
        lp = 1 if msg == WM_CHAR else _key_lparam(code, up, ctx)
        w.PostMessageW(hwnd, msg, code, lp)
        time.sleep(gap)


def post_text(hwnd, text, gap=0.03):
    for ch in text:
        w.PostMessageW(hwnd, WM_CHAR, ord(ch), 1)
        time.sleep(gap)


def add_action_args(p):
    g = p.add_argument_group("in-match actions")
    g.add_argument("--enter-range", action="store_true",
                   help="after the login click TRAINING > PRACTICE RANGE > SELECT")
    g.add_argument("--hero", help="with --enter-range: pick this hero at the first hero select (e.g. Soldier, Ana)")
    g.add_argument("--actions", help="action script run once the match is entered (see tools/owdbg/actions.py), "
                                     "e.g. \"wait 3; tap fire; jump; ability 1; aim-at-bot; hold fire 2; hero Ana\"")
    g.add_argument("--actions-start", choices=("match", "login"), default="match",
                   help="start the script when the match is entered (default) or right after the login "
                        "(menu-time steps such as poke)")
    g.add_argument("--foreground-input", action="store_true",
                   help="send buttons with SendInput to the focused game window instead of writing the "
                        "client's input (takes the desktop)")
    g.add_argument("--close-after", type=float, default=5.0,
                   help="seconds the session stays up after the script ends, then it closes the game and the "
                        "server and frees the game lock (default 5)")
    g.add_argument("--keep-running", action="store_true",
                   help="keep the game up after the script ends (until --duration or the game exits)")


def prepare(args):
    """Before Session(args): checks the script and turns on the server's owner trace (our body id,
    and the buttons it received)."""
    if args.hero:
        _hero(args.hero)
    if not getattr(args, "actions", None):
        return
    steps = parse(args.actions)
    env = dict(getattr(args, "server_env", None) or os.environ)
    level = env.get("RUST_LOG", "info")
    if "orpheus::owner" not in level:
        env["RUST_LOG"] = level + ",orpheus::owner=debug"
    if any(step[0] == "die" for step in steps):
        env["ORPHEUS_DEBUG_SEND"] = "1"  # /api/debug/kill
    args.server_env = env


def start(s, args):
    """After Session(args), before s.start(): --enter-range and --actions. Returns the Actions or None."""
    from .msg import RangeEntry
    rng = RangeEntry(s, _hero(args.hero)[1] if args.hero else None) if args.enter_range else None
    if rng is not None:
        rng.start()
    if not getattr(args, "actions", None):
        return None
    act = Actions(s, args, rng)
    act.start()
    return act


# ---------------------------------------------------------------- script
def _button(word):
    word = ALIASES.get(word.lower(), word.lower())
    if word in BUTTONS:
        return word, BUTTONS[word]
    try:
        return word, int(word, 16)
    except ValueError:
        raise SystemExit(f"--actions: unknown button {word!r} (known: {', '.join(BUTTONS)} or a hex mask)")


def _hero(name):
    key = re.sub(r"[^a-z0-9]", "", name.lower())
    key = HERO_NAMES.get(key, key)
    for x0, heroes in HERO_GROUPS:
        if key in heroes:
            return key, (x0 + HERO_STEP * heroes.index(key), HERO_ROW_Y)
    raise SystemExit(f"--actions: unknown hero {name!r}")


def parse(text):
    """Steps as (verb, args) tuples; SystemExit on errors."""
    steps = []
    for raw in re.split(r"[;\n]", text):
        words = raw.split()
        if not words:
            continue
        verb, rest = words[0].lower(), words[1:]

        def num(i, default=None):
            if len(rest) > i:
                try:
                    return float(rest[i])
                except ValueError:
                    raise SystemExit(f"--actions: {raw.strip()!r}: {rest[i]!r} is not a number")
            if default is None:
                raise SystemExit(f"--actions: {raw.strip()!r} needs a number")
            return default

        if verb == "wait":
            steps.append(("wait", num(0)))
        elif verb in ("tap", "hold"):
            if not rest:
                raise SystemExit(f"--actions: {raw.strip()!r} needs a button")
            name, bits = _button(rest[0])
            rest = rest[1:]
            steps.append(("hold", name, bits, num(0, 0.06) if verb == "tap" else num(0)))
        elif verb in ("press", "release"):
            if not rest:
                raise SystemExit(f"--actions: {raw.strip()!r} needs a button")
            name, bits = _button(rest[0])
            steps.append((verb, name, bits))
        elif verb == "ability":
            n = int(num(0))
            if n not in (1, 2):
                raise SystemExit("--actions: ability 1 or ability 2")
            name = f"ability{n}"
            steps.append(("hold", name, BUTTONS[name], 0.06))
        elif ALIASES.get(verb, verb) in BUTTONS:
            name = ALIASES.get(verb, verb)
            steps.append(("hold", name, BUTTONS[name], num(0, 0.06)))
        elif verb == "move":
            if not rest:
                raise SystemExit("--actions: move DIR S")
            fwd = right = 0
            for part in rest[0].lower().split("-"):
                if part == "forward":
                    fwd = 127
                elif part == "back":
                    fwd = -127
                elif part == "right":
                    right = 127
                elif part == "left":
                    right = -127
                else:
                    raise SystemExit(f"--actions: move direction {rest[0]!r}")
            rest = rest[1:]
            steps.append(("move", fwd, right, num(0)))
        elif verb == "look":
            steps.append(("look", num(0), num(1, 0.0)))
        elif verb == "turn":
            steps.append(("turn", num(0), num(1, 0.0)))
        elif verb in ("aim-at-bot", "aim"):
            if rest and rest[0].lower() == "off":
                steps.append(("aim-off",))
            else:
                steps.append(("aim-bot", int(num(0)) if rest else None))
        elif verb == "hero":
            if not rest:
                raise SystemExit("--actions: hero NAME")
            steps.append(("hero",) + _hero(" ".join(rest)))
        elif verb == "screenshot":
            steps.append(("screenshot", rest[0] if rest else None))
        elif verb == "position":
            steps.append(("position",))
        elif verb == "trace":
            steps.append(("trace", num(0)))
        elif verb == "key":
            if not rest:
                raise SystemExit("--actions: key N (a logical button) or key CHORD (alt+`, enter, ...)")
            if rest[0].isdigit():
                if int(rest[0]) not in ACTION_BUTTONS:
                    raise SystemExit(f"--actions: key {rest[0]}: not in the action table ({ACTION_BUTTONS})")
                steps.append(("key", int(rest[0])))
            else:
                steps.append(("chord", rest[0]) + _chord(rest[0]))
        elif verb == "type":
            text = raw.strip()[len(words[0]):].strip()
            if not text:
                raise SystemExit("--actions: type TEXT")
            steps.append(("type", text))
        elif verb == "die":
            steps.append(("die",))
        elif verb == "poke":
            if len(rest) != 3 or rest[1].lower() not in POKE_TYPES:
                raise SystemExit(f"--actions: poke ADDRESS TYPE VALUE, TYPE one of {', '.join(POKE_TYPES)}")
            kind = rest[1].lower()
            try:
                addr = int(rest[0], 0)
                value = float(rest[2]) if kind[0] == "f" else int(rest[2], 0)
                data = struct.pack("<" + POKE_TYPES[kind], value)
            except (ValueError, struct.error) as e:
                raise SystemExit(f"--actions: {raw.strip()!r}: {e}")
            steps.append(("poke", addr, kind, data))
        else:
            raise SystemExit(f"--actions: unknown step {raw.strip()!r}")
    if not steps:
        raise SystemExit("--actions: empty script")
    return steps


def wrap16(v):
    return (int(round(v)) + 0x8000) % 0x10000 - 0x8000


def quaternion(yaw, pitch):
    """Rotation by `yaw` about +y, then `pitch` about the rotated +x (radians), as (x, y, z, w)."""
    sy, cy = math.sin(yaw / 2), math.cos(yaw / 2)
    sx, cx = math.sin(pitch / 2), math.cos(pitch / 2)
    return (cy * sx, cx * sy, -sy * sx, cy * cx)


def aim_angles(eye, point):
    """(yaw, pitch) in command units looking from eye at point (orpheus_sim::ai::aim_at)."""
    dx, dy, dz = (point[i] - eye[i] for i in range(3))
    return wrap16(math.atan2(dx, dz) * UNITS), wrap16(math.atan2(-dy, math.hypot(dx, dz)) * UNITS)


# ---------------------------------------------------------------- foreground input
def _send(inputs):
    arr = (w.INPUT * len(inputs))(*inputs)
    return w.SendInput(len(inputs), arr, ctypes.sizeof(w.INPUT))


def _key_input(scan, extended, up):
    i = w.INPUT(type=w.INPUT_KEYBOARD)
    i.u.ki = w.KEYBDINPUT(0, scan, w.KEYEVENTF_SCANCODE | (w.KEYEVENTF_KEYUP if up else 0)
                          | (w.KEYEVENTF_EXTENDEDKEY if extended else 0), 0, 0)
    return i


def _mouse_input(button, up):
    flags = {("lmb", False): w.MOUSEEVENTF_LEFTDOWN, ("lmb", True): w.MOUSEEVENTF_LEFTUP,
             ("rmb", False): w.MOUSEEVENTF_RIGHTDOWN, ("rmb", True): w.MOUSEEVENTF_RIGHTUP}[(button, up)]
    i = w.INPUT(type=w.INPUT_MOUSE)
    i.u.mi = w.MOUSEINPUT(0, 0, 0, flags, 0, 0)
    return i


class Foreground:
    """SendInput key/mouse state for --foreground-input."""

    def __init__(self, s):
        self.s = s
        self.down = set()

    def sync(self, buttons, fwd, right):
        want = {name for name, bit in BUTTONS.items() if buttons & bit}
        keys = {("move", k) for k, on in (("forward", fwd > 0), ("back", fwd < 0), ("right", right > 0),
                                          ("left", right < 0)) if on}
        want |= keys
        if want == self.down:
            return
        hwnd = self.s.window()
        if hwnd and w.GetForegroundWindow() != hwnd:
            w.SetForegroundWindow(hwnd)
        events = []
        for item in want ^ self.down:
            up = item in self.down
            if isinstance(item, tuple):
                events.append(_key_input(MOVE_KEYS[item[1]], False, up))
            else:
                key = KEYS[item]
                events.append(_mouse_input(key, up) if isinstance(key, str) else _key_input(*key, up))
        _send(events)
        self.down = want

    def tap_key(self, scan):
        hwnd = self.s.window()
        if hwnd and w.GetForegroundWindow() != hwnd:
            w.SetForegroundWindow(hwnd)
        _send([_key_input(scan, False, False)])
        time.sleep(0.05)
        _send([_key_input(scan, False, True)])

    def _focus(self):
        hwnd = self.s.window()
        if hwnd and w.GetForegroundWindow() != hwnd:
            w.SetForegroundWindow(hwnd)
            time.sleep(0.2)

    def chord(self, mods, vk, gap=0.03):
        self._focus()
        keys = [(w.MapVirtualKeyW(k, 0), k in EXTENDED_VK) for k in mods + (vk,)]
        for scan, ext in keys:
            _send([_key_input(scan, ext, False)])
            time.sleep(gap)
        for scan, ext in reversed(keys):
            _send([_key_input(scan, ext, True)])
            time.sleep(gap)

    def type(self, text, gap=0.03):
        self._focus()
        for ch in text:
            for up in (False, True):
                i = w.INPUT(type=w.INPUT_KEYBOARD)
                i.u.ki = w.KEYBDINPUT(0, ord(ch), KEYEVENTF_UNICODE | (w.KEYEVENTF_KEYUP if up else 0), 0, 0)
                _send([i])
            time.sleep(gap)


# ---------------------------------------------------------------- positions
class Positions:
    """Entity id -> component 4 addresses (position at +0x160), from a heap scan on a worker thread
    (about 3 GB in 15-20 s; the debug loop keeps running)."""

    def __init__(self, s):
        self.s, self.dbg = s, s.dbg
        self.comp4 = {}  # entity id -> [component 4 addresses]
        self.busy = False
        self.scans = 0
        self.ended = 0.0
        self.mask = None
        self.lock = threading.Lock()
        self._friendly = None

    def rescan(self, min_gap=0.0):
        """Starts a scan unless one runs (or one ended less than min_gap seconds ago)."""
        if self.busy or time.monotonic() - self.ended < min_gap:
            return
        self.busy = True
        threading.Thread(target=self._scan, daemon=True).start()

    def _owner_id(self, comp):
        """Entity id of a component's owner (+0x8, an encoded entity pointer), or None."""
        raw = self.dbg.try_read(comp + 8, 8, mask=False)
        if raw is None:
            return None
        owner = struct.unpack("<Q", raw)[0] ^ self.mask
        if not 0x10000 < owner < 1 << 47:
            return None
        raw = self.dbg.try_read(owner + ENTITY_ID, 4, mask=False)
        return struct.unpack("<I", raw)[0] if raw else None

    def _scan(self):
        dbg = self.dbg
        try:
            base = dbg.main.base
            vt = struct.pack("<Q", base + COMP4_VTABLE_RVA)
            key = dbg.read_u64(base + POINTER_KEY - ORPHEUS_BASE)
            self.mask = (((key << 2) | (key >> 62)) & 0xFFFFFFFFFFFFFFFF) ^ POINTER_SALT
            found = {}
            t = time.monotonic()
            total = 0
            va = 0x10000
            while va < 0x7FF000000000:
                mbi = dbg.query(va)
                if mbi is None:
                    break
                start, size = mbi.BaseAddress or 0, mbi.RegionSize
                if mbi.State == w.MEM_COMMIT and mbi.Protect == w.PAGE_READWRITE and mbi.Type == 0x20000:
                    off = 0
                    while off < size:
                        n = min(size - off, 1 << 24)
                        data = dbg.try_read(start + off, n, mask=False) or b""
                        total += len(data)
                        i = data.find(vt)
                        while i >= 0:
                            if i % 8 == 0:
                                comp = start + off + i
                                eid = self._owner_id(comp)
                                if eid is not None:
                                    found.setdefault(eid, []).append(comp)
                            i = data.find(vt, i + 1)
                        off += n
                va = start + size
            with self.lock:
                self.comp4 = found
            self.scans += 1
            bots = sorted(e for e in found if e & 0xFFFFFF00 == BOT_ENTITY)
            self.s.log(f"[actions] scan {self.scans}: {total >> 20} MB in {time.monotonic() - t:.1f} s, "
                       f"{len(found)} entities with component 4, bots {' '.join(f'{e:08X}' for e in bots)}")
        except Exception as e:
            self.s.log(f"[actions] position scan failed: {e!r}")
        finally:
            self.ended = time.monotonic()
            self.busy = False

    def position(self, eid):
        """Component 4 position of entity eid (the last live candidate; a destroyed component keeps
        its vtable, so the owner is checked again), or None."""
        with self.lock:
            addrs = list(self.comp4.get(eid, ()))
        for addr in reversed(addrs):
            raw = self.dbg.try_read(addr, COMP4_POSITION + 12, mask=False)
            if raw is None or struct.unpack_from("<Q", raw)[0] != self.dbg.main.base + COMP4_VTABLE_RVA:
                continue
            if self._owner_id(addr) != eid:
                continue
            return struct.unpack_from("<3f", raw, COMP4_POSITION)
        return None

    def known(self, eid):
        with self.lock:
            return eid in self.comp4

    def friendly_spawns(self):
        """Team-0 (our team in the Range) Training Bot spawn points of the Practice Range, from the
        server's derived data (ai_spawners_174.json in its state folder)."""
        if self._friendly is None:
            self._friendly = []
            server = self.s.server
            for path in sorted(server.state.glob("cache/*/data/ai_spawners_174.json")) if server else []:
                try:
                    data = json.loads(path.read_text(encoding="utf-8"))
                    for row in data["maps"].get("0x0800000000000688", {}).get("spawners", []):
                        if row.get("team") == 0:
                            self._friendly.append(tuple(row["position"]))
                except (OSError, ValueError, KeyError) as e:
                    self.s.log(f"[actions] {path}: {e!r}")
        return self._friendly

    def bots(self, enemies=False):
        """{bot entity id: position}; enemies: without the bots standing on a team-0 spawn point."""
        with self.lock:
            ids = [e for e in self.comp4 if e & 0xFFFFFF00 == BOT_ENTITY]
        out = {e: p for e in sorted(ids) if (p := self.position(e)) is not None}
        if enemies:
            friends = self.friendly_spawns()
            out = {e: p for e, p in out.items()
                   if not any(math.hypot(p[0] - f[0], p[2] - f[2]) < 1.5 for f in friends)}
        return out


# ---------------------------------------------------------------- the script runner
class Actions:
    TAP = 0.06
    EYE = 0.65  # the eye above a body's component 4 position (about 1 m above the feet)
    CHEST = 0.15  # the aim point above a bot's component 4 position

    def __init__(self, s, args, range_entry=None):
        self.s, self.args, self.dbg = s, args, s.dbg
        self.rng = range_entry
        self.steps = parse(args.actions)
        self.foreground = Foreground(s) if args.foreground_input else None
        self.positions = Positions(s)
        self.comp59 = None
        self.samples = 0
        self.last_sample = None
        self.armed = False
        self.started = None  # elapsed time the script started
        self.match_at = None
        self.i = 0
        self.until = None  # the current step's end (elapsed)
        self.held = {}  # name -> bits (press/release)
        self.timed = []  # [end, name, bits]
        self.move = None  # (forward, right, end)
        self.actions = []  # action codes to send, one per input sample
        self.want_view = None  # (yaw16, pitch16) to set with set-view
        self.lock_view = None  # (yaw16, pitch16) written into every sample
        self.track = None  # bot entity id
        self.calib = None  # set-view input mapping: (sign yaw, offset yaw, sign pitch, offset pitch)
        self.calib_points = []
        self.call = None  # a set-view call in progress
        self.scratch = None
        self.calls = 0
        self.body = None  # our body's entity id (server owner trace)
        self._log_off = {}
        self.jump_watch = None
        self.trace_until = None  # `trace S`: log frame and position at every sample until then
        self.hero_step = None
        self.done = False
        self._scan_wait = None
        self._next_track = 0.0
        self.worker = None  # the thread of a key/type/die step; the next step waits for it

    def log(self, msg):
        self.s.log(f"[actions] t={self.dbg.elapsed:.2f} {msg}")

    def start(self):
        self.dbg.every(0.5, self._poll)
        self.dbg.every(0.02, self._tick)

    # ---- arming and the script start
    def _poll(self):
        if not self.armed and self.s.logged_in:
            va = self.dbg.main.base + SAMPLE_SITE - ORPHEUS_BASE
            self.dbg.add_bp(va, self._on_sample)
            self.armed = True
            self.log(f"input sampler breakpoint at 0x{va:X}")
        self._find_body()
        if self.started is not None or not self.armed:
            return
        if self.args.actions_start == "login":
            self.started = self.dbg.elapsed
            self.log(f"logged in: script starts ({len(self.steps)} steps)")
            return
        if self.match_at is None:
            from .msg import match_found
            if match_found(self.s):
                self.match_at = self.dbg.elapsed
        ready = self.match_at is not None and self.last_sample is not None \
            and self.dbg.elapsed - self.last_sample < 0.5
        if self.rng is not None:
            ready = ready and self.rng.state == "done"
        if not ready:
            return
        if self._scan_wait is None:
            # Positions first (aim-at-bot, the jump check): the scan takes about 20 s.
            self._scan_wait = self.dbg.elapsed
            self.positions.rescan()
            self.log("match entered: scanning for positions before the script starts")
            return
        if self.positions.busy and self.dbg.elapsed - self._scan_wait < 60:
            return
        self.started = self.dbg.elapsed
        self.log(f"script starts ({len(self.steps)} steps)")

    def _find_body(self):
        """Our body: the latest `owner frame queued body=X` of the server's owner trace."""
        server = self.s.server
        if server is None or server.state is None:
            return
        for path in (server.state / "serve.log",):  # the server's output (logs/ rotates)
            off = self._log_off.get(path, 0)
            try:
                with open(path, "rb") as f:
                    f.seek(off)
                    data = f.read(1 << 22)
            except OSError:
                continue
            cut = data.rfind(b"\n") + 1
            self._log_off[path] = off + cut
            for m in re.finditer(rb"owner frame queued body=([0-9A-F]{8})", data[:cut]):
                body = int(m.group(1), 16)
                if body != self.body:
                    self.body = body
                    self.log(f"our body is entity {body:08X} (server owner trace)")

    # ---- the input sample
    def _on_sample(self, hit):
        c = hit.reg("rdi")
        self.comp59 = c
        self.samples += 1
        self.last_sample = self.dbg.elapsed
        if self.call is not None:
            return  # back from set-view (the restored context hits this site again)
        self._tick()
        buttons, fwd, right = self._input()
        if self.foreground is None:
            if buttons:
                old = struct.unpack("<I", self.dbg.read(c + SAMPLE_BUTTONS, 4))[0]
                self.dbg.write(c + SAMPLE_BUTTONS, struct.pack("<I", old | buttons))
            if fwd or right:
                self.dbg.write(c + SAMPLE_RIGHT, struct.pack("<bb", right, fwd))
            if self.actions:
                code = self.actions.pop(0)
                self.dbg.write(c + SAMPLE_ACTION, bytes([code]))
                self.log(f"action byte {code} (logical button {ACTION_BUTTONS[code - 1]}) in one sample")
        if self.trace_until is not None:
            self._trace(c)
        if self.lock_view is not None:
            self.dbg.write(c + SAMPLE_YAW, struct.pack("<hh", *self.lock_view))
        if self.want_view is not None:
            self._set_view(hit, c)

    def press(self, button):
        """Logical button `button` pressed once: its action code in the next input sample."""
        self.actions.append(ACTION_BUTTONS.index(button) + 1)

    def _input(self):
        buttons = 0
        for bits in self.held.values():
            buttons |= bits
        for _, _, bits in self.timed:
            buttons |= bits
        fwd, right = (self.move[0], self.move[1]) if self.move else (0, 0)
        return buttons, fwd, right

    # ---- set-view: a call on the sampler's thread
    def _set_view(self, hit, c):
        """Calls set-view(comp59, quaternion) from the sample site: the thread's registers are
        kept, rip/rsp/rcx/rdx point at the call, and a one-shot breakpoint on a scratch return
        address restores them (the sample site then runs as before)."""
        dbg = self.dbg
        if self.scratch is None:
            self.scratch = w.VirtualAllocEx(dbg.process, None, 0x1000, w.MEM_COMMIT | w.MEM_RESERVE,
                                            w.PAGE_EXECUTE_READWRITE)
            if not self.scratch:
                self.log(f"VirtualAllocEx failed ({w.last_error()}): the view cannot turn")
                self.want_view = None
                return
            dbg.write(self.scratch, b"\x90" * 16)
            dbg.injected_returns.add(self.scratch)  # probes on set-view skip our calls
        target = self.want_view
        if self.calib is None:
            # Calibrate the quaternion -> (yaw, pitch) mapping: inputs (0, 0) and (45, 10) degrees.
            n = len(self.calib_points)
            yaw, pitch = ((0.0, 0.0), (math.pi / 4, math.radians(10)))[n]
        else:
            sy, oy, sp, op = self.calib
            yaw = sy * wrap16(target[0] - oy) / UNITS
            pitch = sp * (target[1] - op) / UNITS
        q = quaternion(yaw, pitch)
        dbg.write(self.scratch + 0x100, struct.pack("<4f", *q))
        saved = {name: hit.reg(name) for name in w.GPR_NAMES + ("rip", "eflags")}
        rsp = ((saved["rsp"] - 0x1000) & ~0xF) - 8
        dbg.write(rsp, struct.pack("<Q", self.scratch))
        hit.set("rsp", rsp)
        hit.set("rcx", c)
        hit.set("rdx", self.scratch + 0x100)
        hit.set("rip", dbg.main.base + SET_VIEW - ORPHEUS_BASE)
        self.call = {"saved": saved, "input": (yaw, pitch), "target": target, "comp59": c}
        dbg.add_bp(self.scratch, self._set_view_done, oneshot=True)

    def _set_view_done(self, hit):
        call, self.call = self.call, None
        for name, value in call["saved"].items():
            hit.set(name, value)
        self.calls += 1
        got = struct.unpack("<hh", self.dbg.read(call["comp59"] + VIEW_YAW, 4))
        if self.calib is None:
            self.calib_points.append(got)
            if len(self.calib_points) == 2:
                (y0, p0), (y1, p1) = self.calib_points
                sy = 1 if wrap16(y1 - y0) > 0 else -1
                sp = 1 if p1 - p0 > 0 else -1
                self.calib = (sy, y0, sp, p0)
                self.log(f"set-view calibrated: (0,0) -> {self.calib_points[0]}, (45,10) deg -> "
                         f"{self.calib_points[1]}: yaw sign {sy}, pitch sign {sp}")
            return
        target = call["target"]
        err = (wrap16(got[0] - target[0]), got[1] - target[1])
        if self.want_view == target:
            if abs(err[0]) > 40 or abs(err[1]) > 40:
                self.log(f"set-view gave {got} for {target}; correcting")
                self.calib = (self.calib[0], self.calib[1] + err[0], self.calib[2], self.calib[3] + err[1])
            else:
                self.want_view = None
        if self.track is None or abs(err[0]) > 40:
            self.log(f"view set: yaw {got[0]} pitch {got[1]} (wanted {target[0]} {target[1]}; "
                     f"{math.degrees(got[0] / UNITS):.1f} / {math.degrees(got[1] / UNITS):.1f} deg)")

    def view(self):
        if self.comp59 is None:
            return None
        return struct.unpack("<hh", self.dbg.read(self.comp59 + VIEW_YAW, 4))

    def turn_to(self, yaw16, pitch16):
        pitch16 = int(round(max(-16000, min(16000, pitch16))))
        self.want_view = (wrap16(yaw16), pitch16)
        self.lock_view = self.want_view

    # ---- the step machine
    def _tick(self):
        if self.started is None or self.call is not None:
            return
        now = self.dbg.elapsed
        self.timed = [t for t in self.timed if t[0] > now]
        if self.move and self.move[2] <= now:
            self.move = None
        if self.track is not None:
            self._track(now)
        if self.jump_watch is not None:
            self._jump_watch(now)
        if self.foreground is not None:
            buttons, fwd, right = self._input()
            self.foreground.sync(buttons, fwd, right)
        while not self.done:
            if self.hero_step is not None:
                if not self._hero(now):
                    return
                self.hero_step = None
                self.i += 1
                continue
            if self.until is not None:
                if now < self.until or self.worker is not None and self.worker.is_alive():
                    return
                self.until = None
                self.i += 1
            if self.i >= len(self.steps):
                self.done = True
                self.log(f"script done ({self.samples} input samples, {self.calls} set-view calls)")
                if not self.args.keep_running:
                    linger = max(0.0, self.args.close_after)
                    self.log(f"closing in {linger:g} s (--keep-running keeps the game up)")
                    self.dbg.after(linger, self.dbg.stop)
                return
            self._begin(self.steps[self.i], now)

    def _begin(self, step, now):
        verb = step[0]
        self.log(" ".join(str(x) for x in step))
        if verb == "wait":
            self.until = now + step[1]
        elif verb == "hold":
            _, name, bits, secs = step
            self.timed.append([now + secs, name, bits])
            self.until = now + secs
            if bits & BUTTONS["jump"]:
                self.jump_watch = {"end": now + 1.5, "ys": []}
        elif verb == "press":
            self.held[step[1]] = step[2]
            self.until = now
        elif verb == "release":
            self.held.pop(step[1], None)
            self.until = now
        elif verb == "move":
            self.move = (step[1], step[2], now + step[3])
            self.until = now + step[3]
        elif verb in ("look", "turn"):
            self.track = None
            cur = self.view() or (0, 0)
            yaw, pitch = step[1] * UNITS * math.pi / 180, step[2] * UNITS * math.pi / 180
            if verb == "turn":
                yaw, pitch = cur[0] + yaw, cur[1] + pitch
            self.turn_to(yaw, pitch)
            self.until = now + 0.3
        elif verb == "aim-off":
            self.track = None
            self.lock_view = None
            self.until = now
        elif verb == "aim-bot":
            self.until = now
            if not self._aim_bot(step[1]):
                self.positions.rescan()
                self.log("aim-at-bot: no bot position yet (scan running); retrying for 30 s")
                self.until = None
                self.hero_step = {"kind": "aim", "slot": step[1], "end": now + 30}
                return
        elif verb == "hero":
            self.hero_step = {"kind": "hero", "hero": step[1], "at": step[2], "phase": "open", "t": now}
        elif verb == "screenshot":
            self.screenshot(step[1])
            self.until = now
        elif verb == "trace":
            self.trace_until = now + step[1]
            self.until = now
        elif verb == "position":
            self.log_positions()
            self.until = now
        elif verb == "key":
            self.press(step[1])
            self.until = now
        elif verb in ("chord", "type"):
            hwnd = self.s.window()
            if not hwnd:
                self.log(f"{verb}: no game window")
                self.until = now
                return
            if verb == "chord":
                _, _, mods, vk = step
                send = (lambda: self.foreground.chord(mods, vk)) if self.foreground else \
                    (lambda: post_chord(hwnd, mods, vk))
            else:
                send = (lambda: self.foreground.type(step[1])) if self.foreground else \
                    (lambda: post_text(hwnd, step[1]))
            self._work(send)
        elif verb == "die":
            self._work(self._die)
        elif verb == "poke":
            _, addr, kind, data = step
            va = self.game_va(addr)
            old = self.dbg.try_read(va, len(data))
            try:
                self.dbg.write(va, data)
                self.log(f"poke 0x{va:X} {kind}: {old.hex() if old else '??'} -> {data.hex()}")
            except Exception as e:
                self.log(f"poke 0x{va:X} failed: {e}")
            self.until = now

    def game_va(self, addr):
        """An ORPHEUS VA (base 0x7FF788F10000) inside the game image -> this run's VA; else as is."""
        main = self.dbg.main
        if ORPHEUS_BASE <= addr < ORPHEUS_BASE + main.size:
            return main.base + addr - ORPHEUS_BASE
        return addr

    # ---- aiming
    def _eye(self):
        if self.body is None:
            return None
        p = self.positions.position(self.body)
        if p is None:
            self.positions.rescan(min_gap=10)  # a new body (hero change, respawn)
            return None
        return (p[0], p[1] + self.EYE, p[2])

    def _aim_bot(self, slot):
        eye = self._eye()
        bots = self.positions.bots(enemies=slot is None)
        if eye is None or not bots:
            return False
        if slot is not None:
            eid = BOT_ENTITY + slot
            if eid not in bots:
                return False
        else:
            # The nearest bot about level with us (a bot on a platform is mostly behind its edge),
            # else the nearest.
            level = [e for e in bots if abs(bots[e][1] + self.CHEST - eye[1]) < 2.0] or list(bots)
            eid = min(level, key=lambda e: math.dist(eye, bots[e]))
        self.track = eid
        p = bots[eid]
        self.log(f"aiming at bot {eid:08X} at ({p[0]:.2f}, {p[1]:.2f}, {p[2]:.2f}), "
                 f"{math.dist(eye, p):.1f} m from our eye ({eye[0]:.2f}, {eye[1]:.2f}, {eye[2]:.2f})")
        self._track(self.dbg.elapsed, force=True)
        return True

    def _track(self, now, force=False):
        if not force and now < getattr(self, "_next_track", 0):
            return
        self._next_track = now + 0.1
        eye = self._eye()
        p = self.positions.position(self.track)
        if p is None and self.positions.known(self.track):
            # The component 4 died with its entity: a killed Training Bot is destroyed (and
            # comes back as a new entity).
            self.log(f"target bot {self.track:08X} is gone (killed?); tracking stops")
            self.track = None
            self.positions.rescan(min_gap=5)
            return
        if eye is None or p is None:
            return
        yaw, pitch = aim_angles(eye, (p[0], p[1] + self.CHEST, p[2]))
        if self.lock_view is None or abs(wrap16(yaw - self.lock_view[0])) > 20 or abs(pitch - self.lock_view[1]) > 20:
            self.turn_to(yaw, pitch)

    def _trace(self, c):
        """One `trace` line: the newest command frame in the ring and our body's position."""
        if self.dbg.elapsed >= self.trace_until:
            self.trace_until = None
            self.log("trace ends")
            return
        ring = self.dbg.read(c + COMMAND_RING, COMMAND_RECORD * COMMAND_SLOTS)
        frame = max(struct.unpack_from("<I", ring, COMMAND_RECORD * i)[0] for i in range(COMMAND_SLOTS))
        p = self.positions.position(self.body) if self.body is not None else None
        at = f"({p[0]:.4f}, {p[1]:.4f}, {p[2]:.4f})" if p else "None"
        self.log(f"trace frame={frame} position={at} view={self.view()}")

    def log_positions(self):
        eye = self._eye()
        self.log(f"our body {self.body and f'{self.body:08X}'} eye {eye}, view {self.view()}")
        enemies = self.positions.bots(enemies=True)
        for e, p in self.positions.bots().items():
            self.log(f"  {'enemy' if e in enemies else 'team-0'} bot {e:08X} at ({p[0]:.2f}, {p[1]:.2f}, {p[2]:.2f})"
                     + (f", {math.dist(eye, p):.1f} m" if eye else ""))

    def _jump_watch(self, now):
        jw = self.jump_watch
        if self.body is not None and (p := self.positions.position(self.body)) is not None:
            jw["ys"].append(p[1])
        if now >= jw["end"]:
            self.jump_watch = None
            ys = jw["ys"]
            if ys:
                self.log(f"jump: our body's y from {ys[0]:.3f} up to {max(ys):.3f} (+{max(ys) - ys[0]:.3f} m), "
                         f"back to {ys[-1]:.3f} ({len(ys)} samples)")
            else:
                self.log("jump: no position of our body (scan or server trace missing)")

    # ---- multi-tick steps (hero select, waiting for positions)
    def _hero(self, now):
        """True when the step is over."""
        st = self.hero_step
        if st["kind"] == "aim":
            if self._aim_bot(st["slot"]):
                return True
            if now > st["end"]:
                self.log("aim-at-bot: no bot found; skipped")
                return True
            return False
        from .game import capture, post_click
        from .msg import RangeEntry
        hwnd = self.s.window()
        if not hwnd:
            return now - st["t"] > 10
        rng = self.rng or RangeEntry(self.s)
        if st["phase"] == "open":
            if rng.select_visible():
                st["phase"], st["t"] = "pick", now
            elif self.foreground is not None:
                self.foreground.tap_key(HERO_SELECT_KEY)
                st["phase"], st["t"] = "wait", now
            else:
                self.press(HERO_SELECT_BUTTON)
                st["phase"], st["t"] = "wait", now
            return False
        if st["phase"] == "wait":
            if now - st["t"] >= 1.5:
                self.screenshot(f"hero-{st['hero']}-open")
                st["phase"], st["t"] = "pick", now
            return False
        if st["phase"] == "pick":
            width, height, _ = capture(hwnd)
            x, y = st["at"]
            post_click(hwnd, int(width * x), int(height * y))
            self.log(f"hero select: clicked {st['hero']} at ({int(width * x)}, {int(height * y)})")
            st["phase"], st["t"], st["clicks"] = "confirm", now, 0
            return False
        if st["phase"] == "confirm":
            if now - st["t"] < 1.0:
                return False
            if rng.select_visible() and st["clicks"] < 4:
                post_click(hwnd, *(int(v * f) for v, f in zip(capture(hwnd)[:2], RangeEntry.SELECT)))
                st["clicks"] += 1
                st["t"] = now
                self.log("hero select: clicked SELECT")
                return False
            self.screenshot(f"hero-{st['hero']}-picked")
            st["phase"], st["t"] = "spawn", now
            return False
        if st["phase"] == "spawn":
            back = self.last_sample is not None and now - self.last_sample < 0.3
            if back and now - st["t"] > 2 or now - st["t"] > 15:
                self.log(f"hero select: {'input samples again' if back else 'no input samples after 15 s'}")
                self.positions.rescan()  # the new body has a new transform
                return True
            return False
        return True

    def _work(self, fn):
        """Run fn on a worker thread (it sleeps between messages or waits for HTTP); the step
        machine moves on when it is done."""
        self.worker = threading.Thread(target=fn, daemon=True)
        self.worker.start()
        self.until = self.dbg.elapsed

    def _die(self):
        """POST /api/debug/kill (a worker thread: the debugger loop keeps running)."""
        import urllib.error
        import urllib.request
        from .msg import DASHBOARD
        req = urllib.request.Request(f"{DASHBOARD}/api/debug/kill", data=b"{}",
                                     headers={"Content-Type": "application/json"}, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                res = r.read().decode(errors="replace")
        except urllib.error.HTTPError as e:
            res = f"HTTP {e.code}: {e.read().decode(errors='replace')[:300]}"
        except Exception as e:
            res = f"failed: {e}"
        self.log(f"die: {res}")

    def screenshot(self, name):
        from .game import out_path, screenshot
        hwnd = self.s.window()
        if not hwnd:
            self.log("screenshot: no window")
            return
        path = out_path(f"actions-{name}" if name else "actions", ".png")
        try:
            screenshot(hwnd, str(path))
            self.log(f"screenshot -> {path}")
        except Exception as e:
            self.log(f"screenshot failed: {e!r}")

    def summary(self):
        return {"steps": len(self.steps), "done": self.done, "step": self.i, "samples": self.samples,
                "set_view_calls": self.calls, "body": self.body and f"{self.body:08X}",
                "position_scans": self.positions.scans}
