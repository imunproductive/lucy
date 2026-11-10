"""Application paths and the optional external server runtime directory."""

import os
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_runtime_directory = os.environ.get("ORPHEUS_RUNTIME_DIR")
RUNTIME_DIR = Path(_runtime_directory).resolve() if _runtime_directory else None
DATA_DIR = (RUNTIME_DIR or ROOT) / "data"
COLLISION_CACHE_DIR = RUNTIME_DIR / "collision" if RUNTIME_DIR is not None else ROOT / "cache" / "collision"
# Fixed build data the install does not contain, committed with the Rust crate that embeds it.
FIXED_DIR = ROOT / "crates" / "fixed" / "data"
LOBBY_SCHEMAS = FIXED_DIR / "lobby_schemas.json"
GAME_SCHEMAS = FIXED_DIR / "game_schemas.json"
STATESCRIPT_CODECS = FIXED_DIR / "statescript_codecs.json"
BNET_DESCRIPTORS = FIXED_DIR / "bnet_descriptors.pb"
LOBBY_STATE_BLOB_KEY = FIXED_DIR / "lobby_state_blob.key"
RETAIL_TEMPLATES = FIXED_DIR / "retail_templates.json"
RESOURCE_KEYS = FIXED_DIR / "resource_keys.json"
WEB_DIR = ROOT / "web" / "dashboard"
LOGS_DIR = ROOT / "logs"
LOG_FILE = LOGS_DIR / "ow174.log"
GAME_LOG_FILE = LOGS_DIR / "game.log"
RELAY_DLL = ROOT / "target" / "release" / "owwfd_relay.dll"
GAME_PATH_FILE = ROOT / "game_path.txt"  # the Overwatch.exe picked on the first start
SERVER_ADDRESS_FILE = ROOT / "server_address.txt"  # the server joined last time
PLAYER_NAME_FILE = ROOT / "player_name.txt"  # the name used to join last time
PUBLIC_ADDRESS_FILE = ROOT / "public_address.txt"  # the host's public address, asked when hosting
REQUIREMENTS = ROOT / "requirements.txt"

LOBBY_RUNTIME_DATA = (
    "extracted_items.json", "extracted_general_unlocks.json", "extracted_heroes.json", "extracted_maps.json",
    "arcade_cards_174.json", "game_queues_174.json",
)
MATCH_RUNTIME_DATA = (
    "ai_spawners_174.json", "collision_keys_174.json", "collision_props_174.json", "game_graphs_174.json.gz",
    "game_heroes_174.json", "game_maps_174.json", "game_movers_174.json", "game_stats_174.json",
)


def missing_runtime_files(*, matches: bool) -> list[str]:
    """Report missing external pack members before the service binds its ports."""
    if RUNTIME_DIR is None:
        return []
    files = [DATA_DIR / name for name in LOBBY_RUNTIME_DATA]
    if matches:
        files.extend(DATA_DIR / name for name in MATCH_RUNTIME_DATA)
    return [path.relative_to(RUNTIME_DIR).as_posix() for path in files if not path.is_file()]


@dataclass(frozen=True)
class Paths:
    """Files the lobby server reads and writes. Tests point these at a temporary folder."""

    profiles: Path = ROOT / "profiles"
    template: Path = ROOT / "profile.json"
    default_account: Path = ROOT / "default_account.txt"  # the account a started game logs in as
    matchmaking: Path = ROOT / "matchmaking.json"  # the dashboard's settings per queue card
    client_log: Path = ROOT / "client_msgs.log"
    inject_file: Path = ROOT / "inject.jsonl"
    log_file: Path = LOG_FILE
