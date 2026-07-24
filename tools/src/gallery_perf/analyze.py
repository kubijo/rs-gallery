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


def busiest_thread(profile: dict) -> dict | None:
    """The thread that did the work. gallery renders on one thread, so the rest are noise."""
    threads = [t for t in profile.get("threads", []) if t.get("samples", {}).get("length", 0) > 0]
    if not threads:
        return None
    return max(threads, key=lambda t: t["samples"]["length"])


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


class Breakdown:
    """Sample counts for one thread: inclusive per crate and function, plus self per function."""

    def __init__(
        self, crates: Counter[str], inclusive: Counter[str], own: Counter[str], total: int
    ):
        self.crates = crates
        self.inclusive = inclusive
        self.own = own
        self.total = total


def breakdown(thread: dict) -> Breakdown:
    """Attribute every sample in `thread`, deduplicating per stack.

    A crate or function recursing through one stack counts once for that stack, so an
    inclusive share reads as "this fraction of samples had it somewhere on the stack"
    rather than double-counting depth.
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

    def symbol(stack_index: int) -> str:
        return strings[func_names[frame_funcs[stack_frames[stack_index]]]]

    crates: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    own: Counter[str] = Counter()
    total = 0

    for stack_index, count in Counter(sample_stacks).items():
        if stack_index is None:
            continue
        total += count
        own[symbol(stack_index)] += count

        seen_crates: set[str] = set()
        seen_symbols: set[str] = set()
        walk: int | None = stack_index
        while walk is not None:
            name = symbol(walk)
            if name not in seen_symbols:
                inclusive[name] += count
                seen_symbols.add(name)
            crate = crate_of(name)
            if crate not in seen_crates:
                crates[crate] += count
                seen_crates.add(crate)
            walk = prefixes[walk]

    return Breakdown(crates, inclusive, own, total)
