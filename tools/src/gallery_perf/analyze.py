"""Read a samply profile and attribute its samples.

samply writes the Firefox Profiler format: per thread, a `stringArray` of names, and
`funcTable` / `frameTable` / `stackTable` indices that thread a sample's stack together.
Walking a stack to its root gives inclusive time; the leaf alone gives self time.

Crate attribution is what makes a profile answer "gallery or my component?" — every Rust
symbol carries its crate as the first path segment, so the split falls out of the names
without instrumenting the code being measured.
"""

import gzip
import json
import re
from collections import Counter
from pathlib import Path


def load(path: Path) -> dict:
    """Read a profile, gzipped or not."""
    if path.suffix == ".gz":
        with gzip.open(path, "rt") as f:
            return json.load(f)
    with path.open() as f:
        return json.load(f)


def busiest_thread(profile: dict, process: str | None = None) -> dict | None:
    """The busiest thread of the recorded process — gallery renders on one thread.

    Restricted to one process because a recording holds every process the run spawned. Picking the
    busiest thread outright once selected a rustc thread out of a run that rebuilt while recording,
    and reported the compiler's samples as if they were the app's. `meta.product` is the command
    samply recorded, and names the process to keep.
    """
    threads = [t for t in profile.get("threads", []) if t.get("samples", {}).get("length", 0) > 0]
    if not threads:
        return None
    process = process or profile.get("meta", {}).get("product")
    ours = [t for t in threads if t.get("processName") == process]
    return max(ours or threads, key=lambda t: t["samples"]["length"])


def crate_of(symbol: str) -> str:
    """The crate a symbol belongs to — the first path segment of a Rust symbol."""
    if symbol.startswith("0x"):
        return "[unsymbolized]"
    match = re.match(r"<?(\w+)::", symbol)
    if match:
        return match.group(1)
    if "::" not in symbol and "<" not in symbol:
        return "[system]"
    return "[other]"


#: Attributions that name no code — worth replacing with the library a frame came from.
NO_CRATE = frozenset({"[system]", "[other]", "[unsymbolized]"})


"""Leaf symbols where a parked thread's samples pile up, rather than drawing.

600 frames at 60 fps is ten seconds of wall clock over well under one second of work, and a
wall-clock sampler records that faithfully — three quarters of a run is the event loop. Matched on
the leaf, since that is where the CPU is; anywhere on the stack would exclude everything under the
event loop, which is everything.

Empirical, and stuck that way: samply categorises only as Other / JIT / User / Kernel, so a blocking
`<polling::Poller>::wait_impl` arrives tagged "User" exactly like tessellation. Nothing flags a
missing entry — a suspiciously busy wait function in the work tables is the signal to add one, and
the printed excluded share is what makes that visible.
"""
WAIT_MARKERS = (
    "epoll",
    "timerfd",
    "ppoll",
    "Poll>::poll",
    "syscall_cancel",
    "Poller>::wait",
)


def is_waiting(symbol: str) -> bool:
    return any(marker in symbol for marker in WAIT_MARKERS)


class Breakdown:
    """Sample counts for one thread, over the samples that were doing work.

    `crates` splits self time per crate, falling back to the library a frame came from where the
    symbol names no crate — driver and libc time is a large share of a frame, and lumping it under
    one `[system]` says nothing about which of them. It is self time because inclusive cannot
    discriminate: every sample runs through `main → gallery → eframe → winit`, so each of those
    scores ~93% no matter what the frame did.
    """

    def __init__(
        self,
        crates: Counter[str],
        inclusive: Counter[str],
        own: Counter[str],
        total: int,
        waiting: int = 0,
    ):
        self.crates = crates
        self.inclusive = inclusive
        self.own = own
        self.total = total
        self.waiting = waiting


def breakdown(thread: dict, libs: list[dict] | None = None) -> Breakdown:
    """Attribute every sample in `thread`, deduplicating per stack.

    A crate or function recursing through one stack counts once for that stack, so an
    inclusive share reads as "this fraction of samples had it somewhere on the stack"
    rather than double-counting depth.

    `libs` is the profile's library list, used to name frames whose symbol carries no crate.
    """
    try:
        strings: list[str] = thread["stringArray"]
        func_names: list[int] = thread["funcTable"]["name"]
        frame_funcs: list[int] = thread["frameTable"]["func"]
        prefixes: list[int | None] = thread["stackTable"]["prefix"]
        stack_frames: list[int] = thread["stackTable"]["frame"]
        sample_stacks: list[int | None] = thread["samples"]["stack"]
    except KeyError as e:
        # Names the missing key, so format drift reads as a diagnosis rather than a traceback.
        raise SystemExit(f"unexpected profile shape: missing {e}") from e

    func_resources: list[int] = thread["funcTable"].get("resource", [])
    resource_libs: list[int | None] = thread.get("resourceTable", {}).get("lib", [])

    def symbol(stack_index: int) -> str:
        return strings[func_names[frame_funcs[stack_frames[stack_index]]]]

    def library(stack_index: int) -> str | None:
        """The library a frame came from, or `None` where samply could not place the address."""
        if not libs:
            return None
        func = frame_funcs[stack_frames[stack_index]]
        resource = func_resources[func] if func < len(func_resources) else -1
        if resource is None or not 0 <= resource < len(resource_libs):
            return None
        index = resource_libs[resource]
        if index is None or not 0 <= index < len(libs):
            return None
        return libs[index].get("name")

    def origin(stack_index: int, name: str) -> str:
        crate = crate_of(name)
        if crate not in NO_CRATE:
            return crate
        lib = library(stack_index)
        # Marked, never bare: the main binary's library name is the crate's own, and it statically
        # links every other Rust crate too — an unparseable epaint frame reported as plain `gallery`
        # would read as the shell's own cost.
        return f"lib:{lib}" if lib else crate

    crates: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    own: Counter[str] = Counter()
    total = 0
    waiting = 0

    for stack_index, count in Counter(sample_stacks).items():
        if stack_index is None:
            continue
        leaf = symbol(stack_index)
        if is_waiting(leaf):
            waiting += count
            continue
        total += count
        own[leaf] += count
        crates[origin(stack_index, leaf)] += count

        seen_symbols: set[str] = set()
        walk: int | None = stack_index
        while walk is not None:
            name = symbol(walk)
            if name not in seen_symbols:
                inclusive[name] += count
                seen_symbols.add(name)
            walk = prefixes[walk]

    return Breakdown(crates, inclusive, own, total, waiting)
