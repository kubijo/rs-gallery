"""Symbolication is checked against a hand-built profile.

The shape mirrors what samply saves: library-relative addresses as frame names, tied to a library
through `funcTable.resource` → `resourceTable.lib` → `libs`.
"""

from gallery_perf.symbolicate import (
    addresses_by_lib,
    apply_symbols,
    parse_blocks,
    resolve,
)

LIBC = "/lib/libc.so.6"
APP = "/app/gallery"


def _profile() -> dict:
    """Two libraries whose frames sit at the same offset, plus one already-named func.

    Offset `0x10` exists in both, which is the whole reason addresses are kept per library: the
    third func carries `resource: -1`, as samply writes for a frame it cannot place.
    """
    return {
        "libs": [{"path": LIBC}, {"path": APP}],
        "threads": [
            {
                "stringArray": ["0x10", "already::named", "0x7ffd0000"],
                "funcTable": {"name": [0, 0, 1, 2], "resource": [0, 1, 1, -1]},
                "resourceTable": {"lib": [0, 1]},
            }
        ],
    }


def test_addresses_group_under_the_library_that_owns_them():
    assert addresses_by_lib(_profile()) == {LIBC: {"0x10"}, APP: {"0x10"}}


def test_frames_without_a_library_are_dropped():
    # `0x7ffd0000` is the `resource: -1` func — nothing to resolve it against.
    assert "0x7ffd0000" not in {a for addrs in addresses_by_lib(_profile()).values() for a in addrs}


def test_each_library_keeps_its_own_symbol_for_a_shared_offset():
    profile = _profile()
    rewritten = apply_symbols(
        profile, {LIBC: {"0x10": "malloc"}, APP: {"0x10": "gallery::shell::ui"}}
    )
    thread = profile["threads"][0]
    strings, names = thread["stringArray"], thread["funcTable"]["name"]
    assert rewritten == 2
    assert strings[names[0]] == "malloc"
    assert strings[names[1]] == "gallery::shell::ui"
    # The shared entry both funcs pointed at is left alone, so neither symbol reached the other.
    assert strings[0] == "0x10"


def test_unresolved_addresses_keep_their_address_name():
    profile = _profile()
    assert apply_symbols(profile, {APP: {"0x10": "gallery::shell::ui"}}) == 1
    thread = profile["threads"][0]
    assert thread["stringArray"][thread["funcTable"]["name"][0]] == "0x10"


def test_already_named_funcs_are_left_alone():
    profile = _profile()
    apply_symbols(profile, {APP: {"0x10": "gallery::shell::ui"}})
    thread = profile["threads"][0]
    assert thread["stringArray"][thread["funcTable"]["name"][2]] == "already::named"


def test_resolve_without_addresses_never_runs_addr2line():
    assert resolve("/nonexistent/binary", []) == {}


# Verbatim addr2line `-i -p` output for one real address, trimmed to three frames.
INLINE_OUTPUT = """\
<*const _>::is_null::runtime at /rust/library/core/src/ptr/const_ptr.rs:38
 (inlined by) rustix::backend::conv::ret at /rustix-1.1.4/src/backend/conv.rs:886
 (inlined by) rustix::time::timerfd::timerfd_settime at /rustix-1.1.4/src/time/timerfd.rs:59
plain::function at /src/main.rs:10"""


def test_the_outermost_inlined_frame_wins():
    resolved = parse_blocks(INLINE_OUTPUT, ["0xd528c1", "0xabc"])
    # Not `is_null`, which is the one-line helper inlined at that instruction.
    assert resolved["0xd528c1"] == "rustix::time::timerfd::timerfd_settime"


def test_a_block_without_inlining_keeps_its_own_name():
    resolved = parse_blocks(INLINE_OUTPUT, ["0xd528c1", "0xabc"])
    assert resolved["0xabc"] == "plain::function"


def test_symbols_keep_the_spaces_inside_them():
    resolved = parse_blocks("<A as B>::f at /src/x.rs:1", ["0x1"])
    assert resolved["0x1"] == "<A as B>::f"


def test_unresolvable_blocks_are_skipped_without_shifting_the_rest():
    resolved = parse_blocks("?? at ??:0\nreal::function at /src/x.rs:1", ["0x1", "0x2"])
    assert "0x1" not in resolved
    assert resolved["0x2"] == "real::function"
