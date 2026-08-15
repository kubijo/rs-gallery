"""`gallery-release` — cut a release, or check that a tag names one.

Square brackets are left out of anything shown: rich reads them as markup and swallows what is
inside, so `\\[workspace.package]` would arrive as an empty pair of backticks.

One long line per paragraph, too — rich wraps help text to the terminal, and hard breaks written
here survive into the middle of its lines.
"""

import re
from typing import Annotated

import typer
from rich.console import Console
from rich.markup import escape
from rich.panel import Panel
from rich.prompt import Confirm
from rich.table import Table

from .repo import (
    UNRELEASED,
    VERSION_LINE,
    Level,
    ReleaseError,
    already_tagged,
    check_inheritance,
    declared_version,
    documented,
    next_version,
    rewrite,
    root,
    run,
    today,
    unreleased_notes,
)

HELP = (
    "Cut a release, or check that a tag names one. `just tag major|minor|patch` moves the one "
    "version the workspace declares, dates the changelog's Unreleased section, and — once you "
    "have seen what that comes to and said yes — commits and tags it. `just release-check` asks "
    "the same question of a tag that already exists, which is what CI runs on a tag push.\n\n"
    "Pushing is left to a human: git push --follow-tags"
)

app = typer.Typer(help=HELP, add_completion=False, no_args_is_help=True)
out = Console()
err = Console(stderr=True)


def verify(tag: str) -> str:
    """That `tag` names the version the manifests carry and the CHANGELOG documents."""
    here = root()
    version = declared_version(here)
    check_inheritance(here)
    if tag != f"v{version}":
        raise ReleaseError(f"tag {tag} is not v{version}, which is what the manifests carry")
    if not documented((here / "CHANGELOG.md").read_text(), version):
        raise ReleaseError(f"CHANGELOG.md has no '## [{version}] - <date>' section")
    return version


@app.command()
def check(tag: Annotated[str, typer.Argument(help="the tag to hold the repo to, e.g. v0.1.0")]):
    """Does this tag name what the manifests and the CHANGELOG say?"""
    try:
        verify(tag)
    except ReleaseError as fault:
        # Escaped, not formatted: a message naming `## \[Unreleased]` or a version in brackets
        # would otherwise have it read as markup and dropped.
        err.print(f"[bold red]✗[/] {escape(str(fault))}")
        raise typer.Exit(1) from fault
    out.print(f"[bold green]✓[/] {tag} matches the manifests and the CHANGELOG")


@app.command()
def tag(level: Annotated[Level, typer.Argument(help="which part of the version moves")]):
    """Bump, show what that comes to, and — on confirmation — commit and tag."""
    try:
        _cut(level)
    except ReleaseError as fault:
        # Escaped, not formatted: a message naming `## \[Unreleased]` or a version in brackets
        # would otherwise have it read as markup and dropped.
        err.print(f"[bold red]✗[/] {escape(str(fault))}")
        raise typer.Exit(1) from fault


def _cut(level: Level) -> None:
    here = root()
    if run("git", "status", "--porcelain", cwd=here, capture=True):
        raise ReleaseError("the tree is dirty — a release commit carries only the bump")
    if run("git", "branch", "--show-current", cwd=here, capture=True) != "main":
        raise ReleaseError("releases are cut from main")

    current = declared_version(here)
    check_inheritance(here)
    version = next_version(current, level)
    if already_tagged(here, version):
        raise ReleaseError(f"v{version} is already tagged")

    changelog = here / "CHANGELOG.md"
    notes = unreleased_notes(changelog.read_text())
    if not notes:
        raise ReleaseError(f"'{UNRELEASED}' is empty — write the notes first")
    date = today()

    out.print(Panel.fit(_plan(current, version, date), title=f"Cutting v{version} ({level})"))
    out.print(f"{len(notes)} line(s) of notes are ready to go out.\n")
    if not Confirm.ask("Go ahead?", default=False):
        raise ReleaseError("nothing done")

    rewrite(here / "Cargo.toml", VERSION_LINE, f'version = "{version}"')
    rewrite(
        changelog,
        re.compile(rf"^{re.escape(UNRELEASED)}$", re.MULTILINE),
        f"## [{version}] - {date}",
    )
    run("cargo", "update", "--workspace", "--quiet", cwd=here)

    # Read back what the edits came to, then put the release through the gate it has to pass.
    out.rule("[bold]checking the release it came to")
    verify(f"v{version}")
    out.rule("[bold]validate")
    run("just", "validate", cwd=here)

    run("git", "add", "-A", cwd=here)
    run("git", "commit", "-q", "-m", f"release: v{version}", cwd=here)
    run("git", "tag", f"v{version}", cwd=here)
    out.print(
        Panel.fit(
            f"[bold green]v{version}[/] is committed and tagged.\n"
            "Push it with [bold]git push --follow-tags[/]",
            title="done",
        )
    )


def _plan(current: str, version: str, date: str) -> Table:
    """What the release is about to do, before it does any of it."""
    plan = Table.grid(padding=(0, 2))
    plan.add_column(style="bold")
    plan.add_column()
    plan.add_row("Cargo.toml", f"workspace version {current} [bold]→[/] {version}")
    plan.add_row("CHANGELOG.md", escape(f"{UNRELEASED} → ## [{version}] - {date}"))
    plan.add_row("Cargo.lock", "refreshed")
    plan.add_row("commit", f"release: v{version}")
    plan.add_row("tag", f"v{version}")
    return plan


def main() -> None:
    # Without a name, usage and errors quote the nix store path the venv runs this from.
    app(prog_name="gallery-release")
