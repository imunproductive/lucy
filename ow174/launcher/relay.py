"""The relay DLL that strips TLS from the game's Battle.net connection (source: crates/relay).

Nothing is downloaded: the launcher uses the DLL built with `cargo build --release -p orpheus-relay`.
"""

from pathlib import Path

from ow174.launcher import LaunchError
from ow174.paths import RELAY_DLL


def ensure_relay_dll(path: Path = RELAY_DLL) -> Path:
    if not path.is_file():
        raise LaunchError(
            f"The relay DLL {path} is missing. Build it with cargo build --release -p orpheus-relay."
        )
    return path
