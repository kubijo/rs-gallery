"""Attribution is checked against a hand-built profile.

A real recording needs `perf_event_paranoid` lowered and a GPU, so the format is reproduced here
instead: three frames stacked gallery → component → egui, sampled at two depths.
"""

import gzip
import json

from gallery_perf.analyze import breakdown, busiest_thread, crate_of, load


def _thread(name: str, stacks: list[int]) -> dict:
    """One thread whose stack 0 → 1 → 2 is gallery → component → egui."""
    return {
        "name": name,
        "stringArray": ["gallery::shell", "app_gallery::animation::orbit", "egui::paint"],
        "funcTable": {"name": [0, 1, 2]},
        "frameTable": {"func": [0, 1, 2]},
        "stackTable": {"prefix": [None, 0, 1], "frame": [0, 1, 2]},
        "samples": {"stack": stacks, "length": len(stacks)},
    }


def test_crate_of_reads_the_leading_path_segment():
    assert crate_of("gallery::shell") == "gallery"
    assert crate_of("<gallery::Shell as egui::App>::update") == "gallery"
    assert crate_of("0x7ffde4") == "[unsymbolized]"
    assert crate_of("malloc") == "[system]"


def test_busiest_thread_wins_on_sample_count():
    profile = {"threads": [_thread("idle", [0]), _thread("main", [2, 2, 1])]}
    busiest = busiest_thread(profile)
    assert busiest is not None
    assert busiest["name"] == "main"


def test_busiest_thread_is_none_without_samples():
    assert busiest_thread({"threads": [_thread("idle", [])]}) is None


def test_inclusive_counts_every_crate_on_the_stack():
    report = breakdown(_thread("main", [2, 2, 1]))
    assert report.total == 3
    # Both deep samples pass through all three crates; the third stops at the component.
    assert report.crates["gallery"] == 3
    assert report.crates["app_gallery"] == 3
    assert report.crates["egui"] == 2


def test_self_time_lands_only_on_the_leaf():
    report = breakdown(_thread("main", [2, 2, 1]))
    assert report.own["egui::paint"] == 2
    assert report.own["app_gallery::animation::orbit"] == 1
    assert "gallery::shell" not in report.own


def test_load_reads_gzipped_and_plain(tmp_path):
    profile = {"threads": [_thread("main", [1])]}
    plain = tmp_path / "profile.json"
    plain.write_text(json.dumps(profile))
    gzipped = tmp_path / "profile.json.gz"
    with gzip.open(gzipped, "wt") as f:
        json.dump(profile, f)
    assert load(plain) == load(gzipped) == profile
