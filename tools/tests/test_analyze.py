"""Attribution is checked against a hand-built profile.

A real recording needs `perf_event_paranoid` lowered and a GPU, so the format is reproduced here
instead: three frames stacked gallery → component → egui, sampled at two depths.
"""

import gzip
import json

from gallery_perf.analyze import breakdown, busiest_thread, crate_of, is_waiting, load


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


def test_busiest_thread_stays_inside_the_recorded_process():
    # A run that rebuilt while recording put rustc's threads in the profile, out-sampling the app.
    compiler = _thread("rustc", [2, 2, 2, 1, 1]) | {"processName": "rustc"}
    app = _thread("gallery", [2, 1]) | {"processName": "gallery"}
    busiest = busiest_thread({"meta": {"product": "gallery"}, "threads": [compiler, app]})
    assert busiest is not None
    assert busiest["name"] == "gallery"


def test_busiest_thread_falls_back_when_no_thread_matches_the_process():
    threads = [_thread("worker", [1]) | {"processName": "other"}]
    busiest = busiest_thread({"meta": {"product": "gallery"}, "threads": threads})
    assert busiest is not None
    assert busiest["name"] == "worker"


def test_busiest_thread_is_none_without_samples():
    assert busiest_thread({"threads": [_thread("idle", [])]}) is None


def test_inclusive_counts_every_function_on_the_stack():
    report = breakdown(_thread("main", [2, 2, 1]))
    assert report.total == 3
    # Both deep samples pass through all three frames; the third stops at the component.
    assert report.inclusive["gallery::shell"] == 3
    assert report.inclusive["app_gallery::animation::orbit"] == 3
    assert report.inclusive["egui::paint"] == 2


def test_samples_parked_on_the_event_loop_are_counted_apart():
    thread = _thread("main", [2, 2, 1])
    thread["stringArray"][2] = "rustix::time::timerfd::timerfd_settime"
    report = breakdown(thread)
    # The two deep samples sit in the timer syscall; only the third was drawing.
    assert report.waiting == 2
    assert report.total == 1
    assert report.own == {"app_gallery::animation::orbit": 1}


def test_a_c_symbol_is_charged_to_the_library_it_came_from():
    # `__memcpy_avx512` names no crate; without this it lands in one anonymous `[system]` bucket
    # alongside every driver and libc frame, which was 45% of a real recording.
    thread = _thread("main", [0]) | {
        "funcTable": {"name": [0, 1, 2], "resource": [0, -1, -1]},
        "resourceTable": {"lib": [0]},
    }
    thread["stringArray"] = ["__memcpy_avx512", "app_gallery::orbit", "egui::paint"]
    report = breakdown(thread, [{"name": "libc.so.6"}])
    assert report.crates == {"lib:libc.so.6": 1}


def test_a_frame_samply_could_not_place_keeps_its_bucket():
    thread = _thread("main", [0]) | {
        "funcTable": {"name": [0, 1, 2], "resource": [-1, -1, -1]},
        "resourceTable": {"lib": []},
    }
    thread["stringArray"] = ["__memcpy_avx512", "app_gallery::orbit", "egui::paint"]
    report = breakdown(thread, [{"name": "libc.so.6"}])
    assert report.crates == {"[system]": 1}


def test_a_future_being_polled_is_work_not_waiting():
    # The marker matches calloop's `<calloop::sys::Poll>::poll`; a bare `::poll` also caught every
    # `Future::poll`, quietly deleting the async work of any consumer that has some.
    assert is_waiting("<calloop::sys::Poll>::poll")
    assert not is_waiting("<core::pin::Pin<&mut F> as core::future::future::Future>::poll")


def test_crate_split_is_self_time_so_the_call_spine_does_not_dominate():
    report = breakdown(_thread("main", [2, 2, 1]))
    # Every sample runs through gallery, but only the leaf's crate is charged for the work.
    assert report.crates["egui"] == 2
    assert report.crates["app_gallery"] == 1
    assert "gallery" not in report.crates


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
