"""What the repo says about its own version, and the edits that move it.

Kept apart from the CLI so the parts worth testing can be, and so nothing here prints: a caller
decides how a fault is shown.
"""

import json
import re
import subprocess
import tomllib
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path

MEMBERS = ("gallery-macros", "gallery-build")
UNRELEASED = "## [Unreleased]"

# The one line that carries a version. Anchored at the start of a line, which no dependency's
# `version = ` is, so a miss means the manifest stopped declaring it there.
VERSION_LINE = re.compile(r'^version = "[^"]+"$', re.MULTILINE)


class ReleaseError(Exception):
    """Something the release cannot proceed past, phrased for whoever ran it."""


class Level(StrEnum):
    """Which part of the version a release moves."""

    major = "major"
    minor = "minor"
    patch = "patch"


def root() -> Path:
    """The repo this is being run against.

    Asked of git rather than derived from `__file__`: installed, this module lives in a nix store
    venv that knows nothing about where the checkout is.
    """
    found = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"], text=True, capture_output=True, check=False
    )
    if found.returncode != 0:
        raise ReleaseError("not inside a git checkout")
    return Path(found.stdout.strip())


def run(*command: str, cwd: Path, capture: bool = False) -> str:
    """A command that must succeed."""
    done = subprocess.run(command, cwd=cwd, text=True, capture_output=capture, check=False)
    if done.returncode != 0:
        raise ReleaseError(f"`{' '.join(command)}` failed")
    return done.stdout.strip() if capture else ""


def declared_version(root: Path) -> str:
    """The version every crate resolves to, which is one version or a fault.

    Read through cargo rather than the manifest: cargo resolves `version.workspace`, and the root
    manifest is not strict TOML anyway — `eframe`'s dependency is a multi-line inline table, which
    cargo accepts and `tomllib` rejects.
    """
    metadata = json.loads(
        run("cargo", "metadata", "--no-deps", "--format-version", "1", cwd=root, capture=True)
    )
    versions = {package["version"] for package in metadata["packages"]}
    if len(versions) != 1:
        raise ReleaseError(f"the crates disagree on a version: {', '.join(sorted(versions))}")
    return versions.pop()


def check_inheritance(root: Path) -> None:
    """That the members take that version rather than declaring one of their own."""
    for member in MEMBERS:
        manifest = tomllib.loads((root / member / "Cargo.toml").read_text())
        if manifest["package"].get("version") != {"workspace": True}:
            raise ReleaseError(f"{member} declares a version instead of inheriting it")


def next_version(current: str, level: Level) -> str:
    """`current` with `level` moved on, and everything below it reset."""
    major, minor, patch = (int(part) for part in current.split("."))
    if level is Level.major:
        return f"{major + 1}.0.0"
    if level is Level.minor:
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def unreleased_notes(changelog: str) -> list[str]:
    """The lines written under `## [Unreleased]`, which are what a release releases."""
    lines = changelog.splitlines()
    if UNRELEASED not in lines:
        raise ReleaseError(f"CHANGELOG.md has no '{UNRELEASED}' section")
    start = lines.index(UNRELEASED) + 1
    after = next(
        (at for at, line in enumerate(lines[start:], start) if line.startswith("## ")),
        len(lines),
    )
    return [line for line in lines[start:after] if line.strip()]


def documented(changelog: str, version: str) -> bool:
    """Whether the CHANGELOG carries a dated section for `version`."""
    heading = rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$"
    return re.search(heading, changelog, re.MULTILINE) is not None


def date_unreleased(path: Path, version: str, date: str) -> None:
    """Move the pending notes into a dated release, leaving a fresh section above them."""
    rewrite(
        path,
        re.compile(rf"^{re.escape(UNRELEASED)}$", re.MULTILINE),
        f"{UNRELEASED}\n\n## [{version}] - {date}",
    )


def already_tagged(root: Path, version: str) -> bool:
    return (
        subprocess.run(
            ["git", "rev-parse", "-q", "--verify", f"refs/tags/v{version}"],
            cwd=root,
            capture_output=True,
            check=False,
        ).returncode
        == 0
    )


def rewrite(path: Path, pattern: re.Pattern[str], replacement: str) -> None:
    """Exactly one substitution, or a fault.

    A pattern that missed would say nothing, and one that matched twice would mean the file had
    grown a second place to say the same thing.
    """
    before = path.read_text()
    found = len(pattern.findall(before))
    if found != 1:
        raise ReleaseError(f"{path.name}: {pattern.pattern!r} matched {found} times, want 1")
    path.write_text(pattern.sub(replacement, before))


def today() -> str:
    return datetime.now(UTC).date().isoformat()
