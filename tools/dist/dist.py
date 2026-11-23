#!/usr/bin/env python3
"""Package a Lucy release for Windows (docs/dev/releasing.md).

    python tools/dist/dist.py 0.1.0              # or v0.1.0; `just dist 0.1.0` runs this
    python tools/dist/dist.py 0.1.0 --no-build   # package the release build already made

1. Builds `lucy.exe` and `owwfd_relay.dll` with the release profile
   (`cargo build --release --locked -p lucy -p lucy-relay`), with the version
   in `LUCY_VERSION`, which `lucy --version` prints (apps/lucy/src/main.rs).
   The workspace's Cargo version stays 0.0.0. The dashboard is already in
   `lucy.exe` (crates/server/build.rs embeds web/dashboard/dist).
2. Checks that `lucy.exe --version` prints `lucy <version>`.
3. Zips into `dist/lucy-<version>.zip` (the version without the tag's `v`;
   the README's download instructions name this file), every file at the
   zip's top level: the two builds side by side, `tools/relay/lucy_relay.py`,
   the start scripts and README.md of this folder, the user guide (docs/user,
   as docs/), LICENSE, CREDITS.md and SHA256SUMS.txt of those files.
4. Writes `dist/SHA256SUMS.txt` (the zip's SHA-256, `sha256sum -c` format) and
   `dist/release-notes.md`: the tag's annotation, or else the commits since
   the previous `v*` tag.

In GitHub Actions (`GITHUB_OUTPUT` set) it also writes the step outputs
`version`, `tag`, `zip` and `prerelease` for the release workflow.
Standard library only; Windows only (it runs the built `lucy.exe`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUT = REPO / "dist"

# Semantic version without the tag's `v`: 1.2.3 or 1.2.3-rc.1.
VERSION = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?$")

# Files from the target folder and from the repository, by their name in the zip.
BUILT = ["lucy.exe", "owwfd_relay.dll"]
COPIED = {
    "lucy_relay.py": REPO / "tools/relay/lucy_relay.py",
    "START.cmd": HERE / "START.cmd",
    "START_SERVER_LOCAL.cmd": HERE / "START_SERVER_LOCAL.cmd",
    "LICENSE": REPO / "LICENSE",
    "CREDITS.md": REPO / "CREDITS.md",
}
USER_DOCS = REPO / "docs/user"


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout.strip()


def git_ok(*args: str) -> str | None:
    """`git` that returns None instead of failing (a ref that doesn't exist)."""
    try:
        return git(*args)
    except subprocess.CalledProcessError:
        return None


def target_dir() -> Path:
    """The Cargo target folder, `CARGO_TARGET_DIR` included."""
    metadata = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=REPO,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return Path(json.loads(metadata)["target_directory"])


def build(version: str) -> None:
    env = dict(os.environ, LUCY_VERSION=version)
    command = ["cargo", "build", "--release", "--locked", "-p", "lucy", "-p", "lucy-relay"]
    print("+", " ".join(command), f"(LUCY_VERSION={version})", flush=True)
    subprocess.run(command, cwd=REPO, env=env, check=True)


def check_version(exe: Path, version: str) -> None:
    printed = subprocess.run([exe, "--version"], check=True, capture_output=True, text=True).stdout
    if printed.strip() != f"lucy {version}":
        sys.exit(
            f"{exe} --version prints {printed.strip()!r}, not 'lucy {version}': "
            "build it with LUCY_VERSION set (run without --no-build)"
        )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def user_doc(text: str) -> str:
    """A user guide page for the zip: the guide's commands run the build in the
    checkout's `target\\release`; in the zip, `lucy.exe` is in the top folder."""
    return text.replace("target\\release\\", "")


def stage(folder: Path, release: Path, version: str) -> None:
    """Put the zip's files in `folder`."""
    if folder.exists():
        shutil.rmtree(folder)
    (folder / "docs").mkdir(parents=True)
    for name in BUILT:
        shutil.copy2(release / name, folder / name)
    for name, source in COPIED.items():
        shutil.copy2(source, folder / name)
    readme = (HERE / "release-readme.md").read_text(encoding="utf-8")
    (folder / "README.md").write_text(readme.replace("{version}", version), encoding="utf-8", newline="\n")
    for page in sorted(USER_DOCS.glob("*.md")):
        text = user_doc(page.read_text(encoding="utf-8"))
        (folder / "docs" / page.name).write_text(text, encoding="utf-8", newline="\n")
    files = sorted(path for path in folder.rglob("*") if path.is_file())
    sums = "".join(f"{sha256(path)}  {path.relative_to(folder).as_posix()}\n" for path in files)
    (folder / "SHA256SUMS.txt").write_text(sums, encoding="utf-8", newline="\n")


def zip_folder(folder: Path, archive: Path) -> None:
    """Zip what is in `folder` at the zip's top level, files in name order, each
    dated at the commit's time, so the same commit and builds give the same zip."""
    stamp = time.gmtime(int(git("log", "-1", "--format=%ct")))[:6]
    archive.unlink(missing_ok=True)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
        for path in sorted(path for path in folder.rglob("*") if path.is_file()):
            name = path.relative_to(folder).as_posix()
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            zipped.writestr(info, path.read_bytes(), compresslevel=9)


def release_notes(tag: str, archive: Path, digest: str) -> str:
    """The tag's annotation; for a lightweight tag (or none yet, when run
    before tagging), the commits since the previous `v*` tag."""
    tagged = git_ok("rev-parse", "--verify", "--quiet", f"refs/tags/{tag}") is not None
    end = tag if tagged else "HEAD"
    previous = git_ok("describe", "--tags", "--abbrev=0", "--match", "v*", f"{end}^")

    notes = ""
    if tagged and git("cat-file", "-t", f"refs/tags/{tag}") == "tag":
        subject = git("tag", "--list", "--format=%(contents:subject)", tag)
        body = git("tag", "--list", "--format=%(contents:body)", tag)
        annotation = f"{subject}\n\n{body}".strip()
        if annotation and annotation != tag:
            notes = annotation + "\n"
    if not notes:
        if previous:
            log = git("log", "--no-merges", "--format=- %s (%h)", f"{previous}..{end}")
            notes = f"## Changes since {previous}\n\n{log or '- No changes.'}\n"
        else:
            notes = "The first release.\n"

    notes += (
        "\n## Download\n\n"
        f"`{archive.name}`: unzip it and read `README.md`. Lucy runs on Windows and needs your own "
        "Overwatch install of build 1.74.0.0.104319.\n\n"
        f"SHA-256 (`SHA256SUMS.txt`): `{digest}`\n"
    )
    repository = os.environ.get("GITHUB_REPOSITORY")
    if previous and repository:
        server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
        notes += f"\nAll changes: {server}/{repository}/compare/{previous}...{tag}\n"
    return notes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version", help="the release's version, 1.2.3 or v1.2.3 (the tag)")
    parser.add_argument(
        "--no-build", action="store_true", help="package the release build already in the target folder"
    )
    args = parser.parse_args()

    version = args.version.removeprefix("v")
    if not VERSION.match(version):
        sys.exit(f"{args.version!r} is not a version like 1.2.3 or 1.2.3-rc.1")
    if sys.platform != "win32":
        sys.exit("a release is built on Windows: it runs the built lucy.exe")
    tag = f"v{version}"

    if not args.no_build:
        build(version)
    release = target_dir() / "release"
    check_version(release / "lucy.exe", version)

    OUT.mkdir(exist_ok=True)
    name = f"lucy-{version}"
    folder = OUT / name
    stage(folder, release, version)
    archive = OUT / f"{name}.zip"
    zip_folder(folder, archive)
    shutil.rmtree(folder)

    digest = sha256(archive)
    (OUT / "SHA256SUMS.txt").write_text(f"{digest}  {archive.name}\n", encoding="utf-8", newline="\n")
    notes = release_notes(tag, archive, digest)
    (OUT / "release-notes.md").write_text(notes, encoding="utf-8", newline="\n")

    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as file:
            file.write(f"version={version}\ntag={tag}\nzip={archive.as_posix()}\n")
            file.write(f"prerelease={'true' if '-' in version else 'false'}\n")

    print(f"{archive}\n{OUT / 'SHA256SUMS.txt'}\n{OUT / 'release-notes.md'}")


if __name__ == "__main__":
    main()
