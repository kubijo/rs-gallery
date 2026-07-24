"""Resolve a recorded profile's addresses to symbols, without touching the recording.

samply defers symbolication to view time: it saves `meta.symbolicated: false` and leaves every frame
named `0x…`, then serves names to the front end from its own symbol server when you `samply load`.
Anything else reading that file — our analysis included — sees nothing but addresses.

This resolves them with addr2line into a `symbols.json` sidecar, leaving `profile.json.gz` as samply
wrote it. It has to run before the next build — afterwards the addresses point into new code.

Addresses are library-relative, so each is resolved against the library that owns it, reached
through `funcTable.resource` → `resourceTable.lib` → `libs`. Resolving everything against one
binary instead would mislabel rather than fail: a small offset exists in libc and in the app
alike, and the wrong name comes back looking entirely plausible.
"""

import subprocess
from collections import defaultdict


def addresses_by_lib(profile: dict) -> dict[str, set[str]]:
    """Every unresolved `0x…` name in the profile, grouped by the path of the library owning it.

    Frames samply could not place in a library carry `resource == -1` and are dropped: with no
    library there is nothing to resolve them against.
    """
    libs = profile.get("libs", [])
    grouped: dict[str, set[str]] = defaultdict(set)
    for thread in profile.get("threads", []):
        strings: list[str] = thread["stringArray"]
        func_names: list[int] = thread["funcTable"]["name"]
        func_resources: list[int] = thread["funcTable"]["resource"]
        resource_libs: list[int | None] = thread["resourceTable"]["lib"]
        for func, name_index in enumerate(func_names):
            name = strings[name_index]
            if not name.startswith("0x"):
                continue
            resource = func_resources[func]
            if resource is None or resource < 0:
                continue
            lib = resource_libs[resource]
            if lib is None or lib < 0:
                continue
            path = libs[lib].get("path")
            if path:
                grouped[path].add(name)
    return grouped


INLINED_BY = " (inlined by) "


def parse_blocks(stdout: str, addresses: list[str]) -> dict[str, str]:
    """Pair addr2line's `-i -p` output back to the addresses that produced it.

    Each address prints a block: the innermost frame, then a ` (inlined by) …` line per enclosing
    frame. The outermost is the one to report. Keeping the innermost put
    `<*const _>::is_null::runtime` at the top of a real recording at 42% — the address was really
    `rustix::time::timerfd::timerfd_settime`, and every hot instruction had been labelled with
    whatever one-liner the optimiser inlined there.
    """
    resolved: dict[str, str] = {}
    index = -1
    for line in stdout.splitlines():
        if line.startswith(INLINED_BY):
            name = line[len(INLINED_BY) :]
        else:
            index += 1
            name = line
        if not 0 <= index < len(addresses):
            continue
        # `name at /path/file.rs:38` — the path can't be split off from the left, a Rust symbol
        # holds spaces of its own (`<A as B>::f`).
        name = name.rpartition(" at ")[0] or name
        if name and name != "??":
            # Later lines overwrite earlier ones, so the outermost frame is what survives.
            resolved[addresses[index]] = name
    return resolved


def resolve(lib_path: str, addresses: list[str]) -> dict[str, str]:
    """Resolve addresses against one library, returning only the ones addr2line named.

    Addresses go in on stdin rather than argv — a busy thread carries tens of thousands, well past
    what a command line holds. `-C` demangles, `-f` prints function names, and `-i -p` unfolds the
    inline chain one line per frame.
    """
    if not addresses:
        return {}
    try:
        proc = subprocess.run(
            ["addr2line", "-f", "-C", "-i", "-p", "-e", lib_path],
            input="\n".join(addresses),
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as e:
        raise SystemExit("addr2line not found — it ships with binutils") from e
    if proc.returncode != 0:
        return {}
    return parse_blocks(proc.stdout, addresses)


def symbolicate(profile: dict) -> dict[str, dict[str, str]]:
    """Resolve the whole profile, as `{library path: {address: symbol}}`.

    Libraries that no longer exist on disk are skipped rather than failing the run — a profile spans
    system libraries whose debug info may simply not be installed.
    """
    symbols: dict[str, dict[str, str]] = {}
    for path, addresses in addresses_by_lib(profile).items():
        found = resolve(path, sorted(addresses))
        if found:
            symbols[path] = found
    return symbols


def apply_symbols(profile: dict, symbols: dict[str, dict[str, str]]) -> int:
    """Point every resolved func at its symbol, returning how many were rewritten.

    The name is appended to the thread's `stringArray` and the func re-pointed at it,
    rather than overwriting the existing string in place. One `0x23347` entry is shared by every
    func at that offset, in whichever library — overwriting it would spread one library's symbol
    to all of them.
    """
    libs = profile.get("libs", [])
    rewritten = 0
    for thread in profile.get("threads", []):
        strings: list[str] = thread["stringArray"]
        func_names: list[int] = thread["funcTable"]["name"]
        func_resources: list[int] = thread["funcTable"]["resource"]
        resource_libs: list[int | None] = thread["resourceTable"]["lib"]
        for func, name_index in enumerate(func_names):
            name = strings[name_index]
            if not name.startswith("0x"):
                continue
            resource = func_resources[func]
            if resource is None or resource < 0:
                continue
            lib = resource_libs[resource]
            if lib is None or lib < 0:
                continue
            path = libs[lib].get("path")
            resolved = symbols.get(path, {}).get(name) if path else None
            if resolved is None:
                continue
            strings.append(resolved)
            func_names[func] = len(strings) - 1
            rewritten += 1
    return rewritten
