"""Apply documented, version-scoped exceptions to cargo-outdated's JSON report."""

import json
import os
import subprocess
import tomllib
from typing import TypedDict, cast

from gallery_release.repo import root


class Dependency(TypedDict):
    name: str
    project: str
    latest: str
    compat: str


class ExceptionPolicy(TypedDict):
    packages: list[str]
    project: str
    latest: str
    reason: str


class Policy(TypedDict, total=False):
    exceptions: list[ExceptionPolicy]


class Report(TypedDict):
    dependencies: list[Dependency]


def check_updates(
    dependencies: list[Dependency], exceptions: list[ExceptionPolicy]
) -> tuple[list[str], list[str]]:
    """Return accepted updates and errors, keeping compatible updates and stale policy visible."""
    allowed: dict[tuple[str, str, str], str] = {}
    for exception in exceptions:
        reason = exception["reason"].strip()
        if not reason:
            raise ValueError("outdated exceptions require a reason")
        for package in exception["packages"]:
            allowed[package, exception["project"], exception["latest"]] = reason

    used: set[tuple[str, str, str]] = set()
    notes: list[str] = []
    errors: list[str] = []
    for dependency in dependencies:
        key = dependency["name"], dependency["project"], dependency["latest"]
        name, project, latest = key
        compat = dependency["compat"]
        update = f"{name}: {project} -> {latest} (compatible: {compat})"
        if key in allowed and compat == "---":
            used.add(key)
            notes.append(f"{update}\n  {allowed[key]}")
        else:
            errors.append(update)
    for name, project, latest in sorted(allowed.keys() - used):
        errors.append(f"unused outdated exception: {name} {project} -> {latest}")
    return notes, errors


def main() -> None:
    repo = root()
    policy = cast(Policy, tomllib.loads((repo / "outdated.toml").read_text()))
    temporary = repo / ".tmp"
    temporary.mkdir(exist_ok=True)
    # The distinct glow test dependency still mirrors the femtovg demo's renderer.
    done = subprocess.run(
        ["cargo", "outdated", "--root-deps-only", "--ignore", "glow", "--format", "json"],
        cwd=repo,
        env=os.environ | {"TMPDIR": str(temporary)},
        text=True,
        stdout=subprocess.PIPE,
        check=False,
    )
    if done.returncode:
        raise SystemExit(done.returncode)
    report = cast(Report, json.loads(done.stdout))
    notes, errors = check_updates(report["dependencies"], policy.get("exceptions", []))
    for note in notes:
        print(f"note: {note}")
    for error in errors:
        print(f"error: {error}")
    raise SystemExit(bool(errors))
