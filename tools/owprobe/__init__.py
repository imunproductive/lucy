"""owprobe: the Python side of owprobe.dll, the in-process probe of the Overwatch 1.74 client (crates/probe).

    python -m tools.owprobe launch --enter-range --hero Soldier --keep-running   # orpheus play --probe
    python -m tools.owprobe info
    python -m tools.owprobe trace --seconds 5 --out x.jsonl --input "walk 2s; walk+jump 1; walk 2s"
    python -m tools.owprobe hook 0x7FF78991F5D0 --name rollback --stack 4 --seconds 5
    python -m tools.owprobe screenshot --out shot.png

Library: client.Probe (requests), script.parse (input text -> steps), session.Session (a game with the
probe), screenshot.screenshot. Usage and the protocol: docs/dev/client-tools.md.
"""
from .client import BUTTONS, Probe, ProbeError, f32_bits, f64_bits
from .script import parse as parse_input

__all__ = ["BUTTONS", "Probe", "ProbeError", "f32_bits", "f64_bits", "parse_input"]
