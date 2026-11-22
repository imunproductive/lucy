"""Agent worktrees: create, list and remove them safely.

    python tools/wt.py new <name> [--base main]   # ../lucy-wt/<name> on branch day/<name>
    python tools/wt.py list
    python tools/wt.py remove <name> [--force]    # refuses unmerged branches and linked folders

`new` prints the environment the agent must use (its own target folder, the
build and test caps). `remove` deletes the worktree and its target folder, but
first refuses when any folder inside the worktree is a junction or symlink:
`git worktree remove` follows a junction to `research/out` and wiped the main
checkout's copy twice (2026-11-18, 2026-11-19).
"""

from __future__ import annotations

import argparse
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WORKTREES = REPO.parent / "lucy-wt"


def git(*args: str, cwd: Path = REPO, check: bool = True) -> str:
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
    if check and result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout.strip()


def target_dir(name: str) -> Path:
    return WORKTREES / f"target-{name}"


def is_link(path: Path) -> bool:
    """A symlink or a junction (any reparse point)."""
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return False
    attributes = getattr(info, "st_file_attributes", 0)
    return stat.S_ISLNK(info.st_mode) or bool(attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT)


def links_inside(root: Path) -> list[Path]:
    """Every link below `root`, without descending into links or `.git`."""
    found = []
    for folder, dirs, _files in os.walk(root, followlinks=False):
        for name in list(dirs):
            path = Path(folder) / name
            if is_link(path):
                found.append(path)
                dirs.remove(name)
            elif name in (".git", "node_modules"):
                dirs.remove(name)
    return found


def new(args: argparse.Namespace) -> None:
    path = WORKTREES / args.name
    branch = args.branch or f"day/{args.name}"
    if path.exists():
        sys.exit(f"{path} exists already")
    git("worktree", "add", "-b", branch, str(path), args.base)
    print(f"worktree {path} on {branch} from {args.base}")
    print("use, in bash:")
    print(
        f"  export CARGO_TARGET_DIR={target_dir(args.name).as_posix()} "
        "CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4"
    )


def merged(branch: str) -> bool:
    return (
        subprocess.run(
            ["git", "merge-base", "--is-ancestor", branch, "main"], cwd=REPO
        ).returncode
        == 0
    )


def worktrees() -> list[tuple[Path, str]]:
    """(path, branch or detached head) of each worktree under lucy-wt."""
    entries, path = [], None
    for line in git("worktree", "list", "--porcelain").splitlines():
        if line.startswith("worktree "):
            path = Path(line.removeprefix("worktree "))
        elif line.startswith("branch ") and path:
            entries.append((path, line.removeprefix("branch refs/heads/")))
        elif line == "detached" and path:
            entries.append((path, "(detached)"))
    return [(p, b) for p, b in entries if p.resolve().parent == WORKTREES.resolve()]


def size_gb(path: Path) -> float:
    total = 0
    for folder, _dirs, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(Path(folder) / name).st_size
            except OSError:
                pass
    return total / 1e9


def list_(args: argparse.Namespace) -> None:
    for path, branch in worktrees():
        state = "merged" if branch != "(detached)" and merged(branch) else "open"
        dirty = "dirty" if git("status", "--porcelain", "--untracked-files=no", cwd=path) else "clean"
        target = target_dir(path.name)
        size = f"{size_gb(target):.1f} GB" if args.sizes and target.exists() else ""
        print(f"{path.name:20} {branch:28} {state:7} {dirty:6} {size}")


def remove(args: argparse.Namespace) -> None:
    path = WORKTREES / args.name
    if not path.exists():
        sys.exit(f"no worktree {path}")
    links = links_inside(path)
    if links:
        sys.exit(
            "refusing: these folders are links; remove the links themselves first "
            "(rmdir, not a recursive delete):\n  " + "\n  ".join(map(str, links))
        )
    branch = dict((p.name, b) for p, b in worktrees()).get(args.name)
    if not args.force:
        if git("status", "--porcelain", "--untracked-files=no", cwd=path):
            sys.exit("refusing: uncommitted changes (use --force to drop them)")
        if branch == "(detached)":
            sys.exit("refusing: a detached worktree (benchmarks, integration) may be in use (use --force)")
        if branch and not merged(branch):
            sys.exit(f"refusing: {branch} is not merged into main (use --force)")
    git("worktree", "remove", "--force", str(path))
    target = target_dir(args.name)
    if target.exists():
        shutil.rmtree(target, ignore_errors=True)
    print(f"removed {path} and {target}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(required=True)
    command = commands.add_parser("new", help="create a worktree for an agent")
    command.add_argument("name")
    command.add_argument("--base", default="main")
    command.add_argument("--branch")
    command.set_defaults(run=new)
    command = commands.add_parser("list", help="worktrees, their branches and state")
    command.add_argument("--sizes", action="store_true", help="also the target folder sizes (slow)")
    command.set_defaults(run=list_)
    command = commands.add_parser("remove", help="remove a worktree and its target folder")
    command.add_argument("name")
    command.add_argument("--force", action="store_true")
    command.set_defaults(run=remove)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
