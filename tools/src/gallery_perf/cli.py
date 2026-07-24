"""`gallery-perf` — read a recorded profile and say where the frames went."""

import argparse
from pathlib import Path

from .analyze import Breakdown, breakdown, busiest_thread, load


def _truncate(text: str, width: int = 88) -> str:
    return text if len(text) <= width else text[: width - 1] + "…"


def _rows(counts: dict[str, int], total: int, limit: int) -> list[tuple[str, int, float]]:
    ranked = sorted(counts.items(), key=lambda kv: kv[1], reverse=True)[:limit]
    return [(name, n, 100.0 * n / total) for name, n in ranked]


def _print_analysis(report: Breakdown, top: int) -> None:
    print(f"=== Crate breakdown — inclusive ({report.total} samples) ===\n")
    for name, _, pct in _rows(report.crates, report.total, 15):
        print(f"{pct:5.1f}%  {name}")

    print(f"\n=== Top functions — inclusive ({report.total} samples) ===\n")
    for name, n, pct in _rows(report.inclusive, report.total, top):
        print(f"{pct:5.1f}% ({n:5d})  {_truncate(name)}")

    print(f"\n=== Top functions — self ({report.total} samples) ===\n")
    for name, n, pct in _rows(report.own, report.total, top):
        print(f"{pct:5.1f}% ({n:5d})  {_truncate(name)}")


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="gallery-perf",
        description="Analyse the profiles `just profile` records.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    analyze = sub.add_parser("analyze", help="crate and function breakdown of one profile")
    analyze.add_argument("profile", type=Path, help="reports/<name>/profile.json.gz")
    analyze.add_argument("-n", "--top", type=int, default=20, help="functions per table")

    args = parser.parse_args()

    if not args.profile.exists():
        raise SystemExit(f"no such profile: {args.profile}")

    thread = busiest_thread(load(args.profile))
    if thread is None:
        raise SystemExit("profile has no samples — was the run too short to sample?")

    report = breakdown(thread)
    if report.total == 0:
        raise SystemExit("profile has no attributable samples")

    _print_analysis(report, args.top)
    return 0
