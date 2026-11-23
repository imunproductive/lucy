#!/usr/bin/env python3
"""Check that a fresh clone runs from nothing but the player's install.

    python tools/fresh_clone.py <install> [--rev HEAD] [--target-dir DIR] [--keep]

1. Exports the committed tree at --rev (`git archive`) into a new temporary folder: no build
   folder, no derived data, no state.
2. Checks the tree ships none of the files the server derives from the install
   (`lucy-assets`), such as the message layouts or the resource keys.
3. Builds `lucy` (release) into an empty target folder, unless --target-dir names one.
4. Derives every file from the install into a new state folder (`lucy --state <tmp>/state assets
   build --game <install>`) and checks each file the server reads is there.
5. Starts the lobby and game server (`lucy --state <tmp>/state server`) on free ports and waits for
   its start banner, then stops it.

Nothing outside the temporary folder is written (it is deleted unless --keep). It takes about the
time of a release build plus a first derivation (15-20 minutes on a 16-thread PC).
"""

import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Files the server derives from the install; the repository must not ship them.
DERIVED_ONLY = [
    "lobby_schemas.json",
    "game_schemas.json",
    "bnet_descriptors.pb",
    "lobby_state_blob.key",
    "resource_keys.json",
]
BANNER = "OVERWATCH 1.74 LOBBY SERVER"


def run(args, **kwargs):
    print("+", " ".join(str(arg) for arg in args), flush=True)
    subprocess.run(args, check=True, **kwargs)


def free_ports(count):
    sockets = [socket.socket() for _ in range(count)]
    for sock in sockets:
        sock.bind(("127.0.0.1", 0))
    ports = [sock.getsockname()[1] for sock in sockets]
    for sock in sockets:
        sock.close()
    return ports


def export(rev, tree):
    archive = subprocess.run(["git", "archive", "--format=tar", rev], cwd=ROOT, check=True, capture_output=True)
    tree.mkdir(parents=True)
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive.stdout, check=True)


def check_tree(tree):
    shipped = [
        path.relative_to(tree).as_posix()
        for path in tree.rglob("*")
        if path.is_file() and path.name in DERIVED_ONLY and "target" not in path.parts
    ]
    if shipped:
        sys.exit(f"the tree ships derived files: {shipped}")


def serve(lucy, state, install):
    lobby, stall, rpc, game, dashboard = free_ports(5)
    args = [
        lucy, "--state", state, "server", "--game", install, "--host", "127.0.0.1", "--lobby-port", lobby,
        "--stall-port", stall, "--rpc-port", rpc, "--game-port", game, "--dashboard-port", dashboard,
    ]
    print("+", " ".join(str(arg) for arg in args), flush=True)
    process = subprocess.Popen(
        [str(arg) for arg in args], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace"
    )
    started = threading.Event()

    def read():
        for line in process.stdout:
            print("  |", line.rstrip(), flush=True)
            if BANNER in line:
                started.set()

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    deadline = time.monotonic() + 600
    while not started.is_set() and process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.5)
    running = process.poll() is None
    process.kill()
    process.wait()
    if not started.is_set():
        sys.exit("the server did not start" + ("" if running else f" (exit code {process.returncode})"))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("install", type=Path, help="the Overwatch 1.74 install folder")
    parser.add_argument("--rev", default="HEAD", help="the commit to export (default HEAD)")
    parser.add_argument("--target-dir", type=Path, help="build here instead of an empty folder")
    parser.add_argument("--keep", action="store_true", help="keep the temporary folder")
    args = parser.parse_args()
    install = args.install.resolve()
    temp = Path(tempfile.mkdtemp(prefix="lucy-fresh-"))
    print(f"fresh clone in {temp}", flush=True)
    try:
        tree, state = temp / "lucy", temp / "state"
        export(args.rev, tree)
        check_tree(tree)
        target = args.target_dir or temp / "target"
        env = dict(os.environ, CARGO_TARGET_DIR=str(target))
        env.setdefault("CARGO_BUILD_JOBS", str(max(1, (os.cpu_count() or 4) // 4)))
        run(["cargo", "build", "--release", "--locked", "-p", "lucy"], cwd=tree, env=env)
        lucy = target / "release" / ("lucy.exe" if os.name == "nt" else "lucy")
        state.mkdir()
        run([lucy, "--state", state, "assets", "build", "--game", install], cwd=temp)
        pack = state / "cache" / "104319" / "data"
        missing = [name for name in DERIVED_ONLY if not (pack / name).is_file()]
        if missing:
            sys.exit(f"the derivation did not write {missing}")
        serve(lucy, state, install)
        print("fresh clone: builds, derives every file from the install and starts", flush=True)
    finally:
        if args.keep:
            print(f"kept {temp}")
        else:
            shutil.rmtree(temp, ignore_errors=True)


if __name__ == "__main__":
    main()
