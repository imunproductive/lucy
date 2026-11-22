"""The workspace crates a change can affect: the ones it touches and every crate that depends on them.

    python tools/affected.py                 # changes since main (commits + working tree)
    python tools/affected.py --base <rev>
    python tools/affected.py --cargo-args    # prints `-p a -p b ...` for cargo test / nextest

A change to the workspace's own build files (Cargo.toml, Cargo.lock, .cargo/, .config/,
rust-toolchain.toml) affects every crate; one to docs, tools or web/ affects none, except the
dashboard's build (web/dashboard/dist/), which lucy-server embeds.
Merges still run the whole suite; this is for an agent's own checks.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(
    subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
)
EVERYTHING = ("Cargo.toml", "Cargo.lock", ".cargo/", ".config/", "rust-toolchain.toml")
# Files outside a crate's folder that the crate embeds (its build.rs).
EMBEDDED = {"web/dashboard/dist/": "lucy-server"}


def run(*args: str) -> str:
    result = subprocess.run(args, cwd=REPO, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"{' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout


def changed_files(base: str) -> set[str]:
    files = set(run("git", "diff", "--name-only", f"{base}...HEAD").split())
    files |= set(run("git", "diff", "--name-only", "HEAD").split())
    files |= set(run("git", "ls-files", "--others", "--exclude-standard").split())
    return {f.replace("\\", "/") for f in files}


def workspace() -> tuple[dict[str, str], dict[str, set[str]]]:
    """Each member's folder (relative, `/`-separated) and its workspace dependents."""
    metadata = json.loads(run("cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"))
    folders, names = {}, set()
    for package in metadata["packages"]:
        folder = Path(package["manifest_path"]).parent.relative_to(REPO).as_posix()
        folders[package["name"]] = folder
        names.add(package["name"])
    dependents: dict[str, set[str]] = {name: set() for name in names}
    for package in metadata["packages"]:
        for dependency in package["dependencies"]:
            if dependency["name"] in names:
                dependents[dependency["name"]].add(package["name"])
    return folders, dependents


def affected(files: set[str], folders: dict[str, str], dependents: dict[str, set[str]]) -> set[str]:
    if any(f == e or f.startswith(e) for f in files for e in EVERYTHING):
        return set(folders)
    touched = {
        name
        for name, folder in folders.items()
        for f in files
        if f == folder or f.startswith(folder + "/")
    }
    touched |= {name for prefix, name in EMBEDDED.items() if any(f.startswith(prefix) for f in files)}
    result, queue = set(touched), list(touched)
    while queue:
        for dependent in dependents[queue.pop()]:
            if dependent not in result:
                result.add(dependent)
                queue.append(dependent)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base", default="main")
    parser.add_argument("--cargo-args", action="store_true")
    args = parser.parse_args()
    # LF even on Windows: bash callers split the output on newlines.
    sys.stdout.reconfigure(newline="\n")
    folders, dependents = workspace()
    crates = sorted(affected(changed_files(args.base), folders, dependents))
    if args.cargo_args:
        print(" ".join(f"-p {name}" for name in crates))
    else:
        print("\n".join(crates) if crates else "(no crate affected)")


if __name__ == "__main__":
    main()
