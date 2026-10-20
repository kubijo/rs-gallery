import json
import subprocess
from pathlib import Path

import pytest

from gallery_release import outdated

REASON = "Keep scene dependencies compatible."
EXCEPTIONS: list[outdated.ExceptionPolicy] = [
    {
        "packages": ["egui"],
        "project": "0.36.2",
        "latest": "0.37.0",
        "reason": REASON,
    }
]


def dependency(
    *, name: str = "egui", project: str = "0.36.2", latest: str = "0.37.0", compat: str = "---"
) -> outdated.Dependency:
    return {"name": name, "project": project, "latest": latest, "compat": compat}


def test_only_the_documented_upgrade_is_accepted():
    notes, errors = outdated.check_updates([dependency()], EXCEPTIONS)
    assert not errors
    assert REASON in notes[0]


@pytest.mark.parametrize(
    "changes",
    [
        {"compat": "0.36.3"},
        {"latest": "0.37.1"},
        {"project": "0.36.3"},
        {"name": "another-crate"},
    ],
)
def test_new_updates_still_fail(changes: dict[str, str]) -> None:
    notes, errors = outdated.check_updates([dependency(**changes)], EXCEPTIONS)
    assert not notes
    assert errors


def test_obsolete_exceptions_fail():
    _, errors = outdated.check_updates([], EXCEPTIONS)
    assert errors == ["unused outdated exception: egui 0.36.2 -> 0.37.0"]


def test_updates_without_exceptions_fail():
    _, errors = outdated.check_updates([dependency()], [])
    assert errors == ["egui: 0.36.2 -> 0.37.0 (compatible: ---)"]


def test_no_updates_and_no_exceptions_pass():
    assert outdated.check_updates([], []) == ([], [])


def test_exceptions_require_reasons():
    with pytest.raises(ValueError, match="require a reason"):
        exception = EXCEPTIONS[0].copy()
        exception["reason"] = " "
        outdated.check_updates([], [exception])


@pytest.mark.parametrize("returncode", [0, 1])
def test_command_preserves_cargo_errors_and_applies_repo_policy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, returncode: int
) -> None:
    (tmp_path / "outdated.toml").write_text(
        '[[exceptions]]\npackages = ["egui"]\nproject = "0.36.2"\nlatest = "0.37.0"\n'
        'reason = "Keep scenes compatible."\n'
    )
    monkeypatch.setattr(outdated, "root", lambda: tmp_path)

    def run(
        command: list[str], *, cwd: Path, env: dict[str, str], text: bool, stdout: int, check: bool
    ) -> subprocess.CompletedProcess[str]:
        assert command == [
            "cargo",
            "outdated",
            "--root-deps-only",
            "--ignore",
            "glow",
            "--format",
            "json",
        ]
        assert cwd == tmp_path
        assert env["TMPDIR"] == str(tmp_path / ".tmp")
        return subprocess.CompletedProcess(
            command, returncode, stdout=json.dumps({"dependencies": [dependency()]})
        )

    monkeypatch.setattr(outdated.subprocess, "run", run)
    with pytest.raises(SystemExit) as stopped:
        outdated.main()
    assert stopped.value.code == returncode
