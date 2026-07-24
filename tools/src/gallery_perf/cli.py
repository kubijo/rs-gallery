"""`gallery-perf` — read a recorded profile and say where the frames went."""

import argparse
import json
from pathlib import Path

from .analyze import Breakdown, breakdown, busiest_thread, load
from .symbolicate import apply_symbols, symbolicate


def _truncate(text: str, width: int = 88) -> str:
    return text if len(text) <= width else text[: width - 1] + "…"


def _rows(counts: dict[str, int], total: int, limit: int) -> list[tuple[str, int, float]]:
    ranked = sorted(counts.items(), key=lambda kv: kv[1], reverse=True)[:limit]
    return [(name, n, 100.0 * n / total) for name, n in ranked]


def _print_analysis(report: Breakdown, top: int) -> None:
    sampled = report.total + report.waiting
    if report.waiting:
        share = 100.0 * report.waiting / sampled
        print(
            f"{report.waiting} of {sampled} samples ({share:.1f}%) were parked on the event loop, "
            f"not drawing — excluded below.\n"
        )

    print(f"=== Crate / library breakdown — self ({report.total} samples of work) ===\n")
    for name, _, pct in _rows(report.crates, report.total, 15):
        print(f"{pct:5.1f}%  {name}")

    print(f"\n=== Top functions — inclusive ({report.total} samples) ===\n")
    for name, n, pct in _rows(report.inclusive, report.total, top):
        print(f"{pct:5.1f}% ({n:5d})  {_truncate(name)}")

    print(f"\n=== Top functions — self ({report.total} samples) ===\n")
    for name, n, pct in _rows(report.own, report.total, top):
        print(f"{pct:5.1f}% ({n:5d})  {_truncate(name)}")


def _default_symbols(profile: Path) -> Path:
    return profile.parent / "symbols.json"


def _run_symbolicate(args: argparse.Namespace) -> int:
    profile = load(args.profile)
    symbols = symbolicate(profile)
    out = args.output or _default_symbols(args.profile)
    out.write_text(json.dumps(symbols, separators=(",", ":")))
    resolved = sum(len(v) for v in symbols.values())
    print(f"resolved {resolved} addresses across {len(symbols)} libraries → {out}")
    return 0


def _run_analyze(args: argparse.Namespace) -> int:
    profile = load(args.profile)

    symbols_path = args.symbols or _default_symbols(args.profile)
    if symbols_path.exists():
        rewritten = apply_symbols(profile, json.loads(symbols_path.read_text()))
        print(f"({rewritten} symbols from {symbols_path.name})\n")

    thread = busiest_thread(profile, args.process)
    if thread is None:
        raise SystemExit("profile has no samples — was the run too short to sample?")

    report = breakdown(thread, profile.get("libs"))
    if report.total == 0:
        raise SystemExit("profile has no attributable samples")

    _print_analysis(report, args.top)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="gallery-perf",
        description="Analyse the profiles `just profile` records.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    analyze = sub.add_parser("analyze", help="crate and function breakdown of one profile")
    analyze.add_argument("profile", type=Path, help="reports/<name>/profile.json.gz")
    analyze.add_argument("-n", "--top", type=int, default=20, help="functions per table")
    analyze.add_argument("--symbols", type=Path, help="symbols.json (default: beside the profile)")
    analyze.add_argument("--process", help="process to analyse (default: the recorded command)")
    analyze.set_defaults(run=_run_analyze)

    symbols = sub.add_parser(
        "symbolicate",
        help="resolve the profile's addresses into a symbols.json sidecar",
        description="Run this before rebuilding: it reads the binaries the recording points at.",
    )
    symbols.add_argument("profile", type=Path, help="reports/<name>/profile.json.gz")
    symbols.add_argument("-o", "--output", type=Path, help="where to write (default: symbols.json)")
    symbols.set_defaults(run=_run_symbolicate)

    args = parser.parse_args()
    if not args.profile.exists():
        raise SystemExit(f"no such profile: {args.profile}")
    return int(args.run(args))
