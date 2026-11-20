"""Input scripts, frame by frame: text -> the probe's input steps (crates/probe/src/input.rs).

    walk 120; walk+jump 1; walk 60          forward for 120 input samples, then with jump for one, ...
    forward+right 2s; wait 500ms            a duration in samples (frames), seconds or milliseconds
    look 90 -10; turn -45; free             absolute / relative view in degrees (pitch + down); stop holding
    press crouch; ...; release crouch       buttons held across steps
    key 81                                  a logical button's action code in one sample (hero select is 81)

Parts of a step (joined with '+'): forward (walk), back, left, right, wait/idle and the buttons fire, fire2,
interact, ability1 (shift), ability2 (e), ultimate (q), jump, crouch, reload, melee. Without a duration a step
lasts one input sample. One input sample is one rendered frame; the client merges every sample of a command
frame, so a one-sample button reaches the command.
"""
import math
import re

from .client import BUTTONS

UNITS_PER_DEGREE = 65536.0 / 360.0
ALIASES = {"walk": "forward", "secondary": "fire2", "shift": "ability1", "e": "ability2", "q": "ultimate",
           "ult": "ultimate", "idle": "wait"}
MOVES = {"forward": (127, 0), "back": (-127, 0), "left": (0, -127), "right": (0, 127)}


def _angle(text, raw):
    try:
        return int(round(float(text) * UNITS_PER_DEGREE))
    except ValueError:
        raise ValueError(f"{raw!r}: {text!r} is not a number of degrees")


def _wrap(units):
    return (units + 0x8000) % 0x10000 - 0x8000


def _duration(text, raw):
    """'120' / '120f' samples, '2s', '500ms' -> ('frames', n) | ('ms', n)."""
    m = re.fullmatch(r"(\d+(?:\.\d+)?)(f|s|ms)?", text)
    if not m:
        raise ValueError(f"{raw!r}: bad duration {text!r} (120, 120f, 2s, 500ms)")
    value, unit = float(m.group(1)), m.group(2) or "f"
    if unit == "f":
        return {"frames": int(value)}
    return {"ms": int(value * 1000 if unit == "s" else value)}


def parse(text):
    """Steps for Probe.input(); ValueError on a bad step."""
    from tools.owdbg.actions import ACTION_BUTTONS

    steps = []
    for raw in re.split(r"[;\n]", text):
        words = raw.split()
        if not words:
            continue
        verb = words[0].lower()
        if verb in ("look", "turn"):
            if len(words) < 2:
                raise ValueError(f"{raw!r}: {verb} YAW [PITCH]")
            yaw = _angle(words[1], raw)
            pitch = _angle(words[2], raw) if len(words) > 2 else 0
            steps.append({"op": verb, "yaw": _wrap(yaw), "pitch": max(-32768, min(32767, pitch))})
        elif verb == "free":
            steps.append({"op": "free"})
        elif verb in ("press", "release"):
            bits = 0
            for part in words[1:]:
                name = ALIASES.get(part.lower(), part.lower())
                if name not in BUTTONS:
                    raise ValueError(f"{raw!r}: unknown button {part!r}")
                bits |= BUTTONS[name]
            steps.append({"op": verb, "buttons": bits})
        elif verb == "key":
            if len(words) != 2 or not words[1].isdigit() or int(words[1]) not in ACTION_BUTTONS:
                raise ValueError(f"{raw!r}: key N, N a logical button of the action table {ACTION_BUTTONS}")
            steps.append({"op": "action", "code": ACTION_BUTTONS.index(int(words[1])) + 1})
        else:
            step = {"op": "run", "buttons": 0, "forward": 0, "right": 0, "frames": 1}
            for part in words[0].lower().split("+"):
                name = ALIASES.get(part, part)
                if name == "wait":
                    continue
                if name in MOVES:
                    forward, right = MOVES[name]
                    step["forward"] = step["forward"] or forward
                    step["right"] = step["right"] or right
                elif name in BUTTONS:
                    step["buttons"] |= BUTTONS[name]
                else:
                    raise ValueError(f"{raw!r}: unknown part {part!r}")
            if len(words) > 2:
                raise ValueError(f"{raw!r}: one duration after the parts")
            if len(words) == 2:
                del step["frames"]
                step.update(_duration(words[1].lower(), raw))
            steps.append(step)
    if not steps:
        raise ValueError("empty input script")
    return steps


def degrees(units):
    return units * 360.0 / 65536.0


def yaw_toward(dx, dz):
    """The view yaw (16-bit units) facing (dx, dz) on the ground: forward = (sin, 0, cos)."""
    return _wrap(int(round(math.atan2(dx, dz) * 65536.0 / (2 * math.pi))))
