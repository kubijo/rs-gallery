import pytest

from gallery_release.repo import (
    UNRELEASED,
    VERSION_LINE,
    Level,
    ReleaseError,
    date_unreleased,
    documented,
    next_version,
    rewrite,
    unreleased_notes,
)

CHANGELOG = f"""# Changelog

{UNRELEASED}

- a fix nobody has released yet

## [0.1.0] - 2026-08-15

- the first one

## 2026-08-14

- from before any of this was tagged
"""


def test_a_level_moves_its_own_part_and_resets_what_is_under_it():
    assert next_version("1.4.2", Level.major) == "2.0.0"
    assert next_version("1.4.2", Level.minor) == "1.5.0"
    assert next_version("1.4.2", Level.patch) == "1.4.3"


def test_the_notes_are_the_lines_under_unreleased_and_stop_at_the_next_section():
    assert unreleased_notes(CHANGELOG) == ["- a fix nobody has released yet"]


def test_an_unreleased_section_with_nothing_under_it_releases_nothing():
    assert unreleased_notes(f"# Changelog\n\n{UNRELEASED}\n\n## [0.1.0] - 2026-08-15\n") == []


def test_a_changelog_without_the_section_is_a_fault_rather_than_an_empty_release():
    with pytest.raises(ReleaseError):
        unreleased_notes("# Changelog\n\n## [0.1.0] - 2026-08-15\n")


def test_a_version_counts_as_documented_only_with_a_dated_heading_of_its_own():
    assert documented(CHANGELOG, "0.1.0")
    assert not documented(CHANGELOG, "0.2.0")
    # Dates alone headed the sections before any of this was released.
    assert not documented(CHANGELOG, "2026-08-14")


def test_a_rewrite_that_matched_once_lands(tmp_path):
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text('[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n')

    rewrite(manifest, VERSION_LINE, 'version = "0.2.0"')

    assert 'version = "0.2.0"' in manifest.read_text()


@pytest.mark.parametrize(
    "manifest",
    [
        '[workspace.package]\nedition = "2024"\n',
        '[workspace.package]\nversion = "0.1.0"\n\n[other]\nversion = "0.1.0"\n',
    ],
    ids=["missing", "twice"],
)
def test_a_rewrite_that_did_not_match_exactly_once_is_a_fault(tmp_path, manifest):
    path = tmp_path / "Cargo.toml"
    path.write_text(manifest)

    with pytest.raises(ReleaseError):
        rewrite(path, VERSION_LINE, 'version = "0.2.0"')


def test_dating_unreleased_keeps_a_fresh_section_for_the_next_release(tmp_path):
    changelog = tmp_path / "CHANGELOG.md"
    changelog.write_text(CHANGELOG)

    date_unreleased(changelog, "0.2.0", "2026-09-02")

    released = changelog.read_text()
    assert f"{UNRELEASED}\n\n## [0.2.0] - 2026-09-02" in released
    assert unreleased_notes(released) == []
    assert documented(released, "0.2.0")
    assert "- a fix nobody has released yet" in released
