#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# ///
"""DHAT heap profile analyzer for epub-stream.

Parses dhat-rs JSON output and produces actionable allocation reports.

Usage:
    uv run scripts/analyze_heap.py summary [--phase PHASE] [--dir DIR]
    uv run scripts/analyze_heap.py hotspots [--phase PHASE] [-n N] [--dir DIR]
    uv run scripts/analyze_heap.py churn [--phase PHASE] [-n N] [--dir DIR]
    uv run scripts/analyze_heap.py peak [--phase PHASE] [-n N] [--dir DIR]
    uv run scripts/analyze_heap.py budget [--target SIZE] [--dir DIR]
    uv run scripts/analyze_heap.py compare FILE_A FILE_B [-n N]
    uv run scripts/analyze_heap.py report [--phase PHASE] [--target SIZE] [--dir DIR]
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import sys
from dataclasses import dataclass, field


# ---------------------------------------------------------------------------
# DHAT JSON parsing
# ---------------------------------------------------------------------------


@dataclass
class Profile:
    """Parsed DHAT profile."""

    path: str
    name: str
    phase: str
    total_bytes: int
    total_blocks: int
    peak_bytes: int  # gb sum (bytes alive at t-gmax)
    peak_blocks: int
    end_bytes: int
    end_blocks: int
    pps: list[dict]
    ftbl: list[str]


def parse_profile(path: str) -> Profile:
    with open(path) as f:
        d = json.load(f)
    pps = d["pps"]
    ftbl = d["ftbl"]
    basename = os.path.basename(path).replace(".json", "")
    # Extract phase and book name from dhat-<phase>-<name> or dhat-<phase>.
    # Match known phases by longest prefix so "session_once" doesn't bleed
    # into "session" reports.
    stem = basename.removeprefix("dhat-")
    known_phases = [
        "session_once",
        "session-once",
        "tokenize",
        "render",
        "cover",
        "open",
        "full",
        "session",
    ]
    phase = "unknown"
    name = "(aggregate)"
    for candidate in sorted(known_phases, key=len, reverse=True):
        if stem == candidate:
            phase = candidate
            name = "(aggregate)"
            break
        prefix = f"{candidate}-"
        if stem.startswith(prefix):
            phase = candidate
            name = stem[len(prefix) :]
            break
    if phase == "unknown":
        parts = basename.split("-", 2)  # ["dhat", phase, name?]
        phase = parts[1] if len(parts) >= 2 else "unknown"
        name = parts[2] if len(parts) >= 3 else "(aggregate)"
    return Profile(
        path=path,
        name=name,
        phase=phase,
        total_bytes=sum(p["tb"] for p in pps),
        total_blocks=sum(p["tbk"] for p in pps),
        peak_bytes=sum(p["gb"] for p in pps),
        peak_blocks=sum(p["gbk"] for p in pps),
        end_bytes=sum(p["eb"] for p in pps),
        end_blocks=sum(p["ebk"] for p in pps),
        pps=pps,
        ftbl=ftbl,
    )


def load_profiles(directory: str, phase: str | None = None) -> list[Profile]:
    pattern = os.path.join(directory, "dhat-*.json")
    profiles = []
    for path in sorted(glob.glob(pattern)):
        basename = os.path.basename(path)
        # Skip aggregate files (dhat-<phase>.json without book name)
        parts = basename.replace(".json", "").split("-")
        if len(parts) < 3:
            continue
        p = parse_profile(path)
        if phase and p.phase != phase:
            continue
        profiles.append(p)
    return profiles


# ---------------------------------------------------------------------------
# Function name utilities
# ---------------------------------------------------------------------------


def clean_fn(raw: str) -> str:
    """Clean a DHAT function table entry into a readable name."""
    # Strip address prefix: "0x10249dbc4: name (file:line)"
    if ": " in raw:
        raw = raw.split(": ", 1)[1]
    # Strip source location
    if " (" in raw:
        raw = raw.rsplit(" (", 1)[0]
    return raw


def shorten_fn(name: str, max_len: int = 72) -> str:
    """Shorten epub_stream namespaces for display."""
    s = name
    s = s.replace("epub_stream_render::render_layout::", "layout::")
    s = s.replace("epub_stream_render::render_ir::", "ir::")
    s = s.replace("epub_stream_render::render_engine::", "engine::")
    s = s.replace("epub_stream_embedded_graphics::", "eg::")
    s = s.replace("epub_stream::render_prep::", "prep::")
    s = s.replace("epub_stream::book::", "book::")
    s = s.replace("epub_stream::zip::", "zip::")
    s = s.replace("epub_stream::tokenizer::", "tokenizer::")
    s = s.replace("epub_stream::metadata::", "metadata::")
    s = s.replace("epub_stream::css::", "css::")
    s = s.replace("<", "").replace(">", "")
    if len(s) > max_len:
        s = s[: max_len - 3] + "..."
    return s


def owner_fn(pp: dict, ftbl: list[str]) -> str:
    """Find the first epub_stream function in a call stack."""
    for idx in pp["fs"][1:8]:
        name = clean_fn(ftbl[idx])
        if "epub_stream" in name or "heap_profile" in name:
            return name
    # Fallback: deepest non-root frame
    if len(pp["fs"]) > 1:
        return clean_fn(ftbl[pp["fs"][1]])
    return "(unknown)"


# ---------------------------------------------------------------------------
# Aggregation
# ---------------------------------------------------------------------------


@dataclass
class SiteStats:
    total_bytes: int = 0
    total_blocks: int = 0
    peak_bytes: int = 0  # max gb across pps
    peak_max: int = 0  # max mb (single pp max)


def aggregate_sites(profiles: list[Profile]) -> dict[str, SiteStats]:
    sites: dict[str, SiteStats] = {}
    for prof in profiles:
        for pp in prof.pps:
            fn = owner_fn(pp, prof.ftbl)
            if fn not in sites:
                sites[fn] = SiteStats()
            s = sites[fn]
            s.total_bytes += pp["tb"]
            s.total_blocks += pp["tbk"]
            s.peak_bytes += pp["gb"]
            s.peak_max = max(s.peak_max, pp["mb"])
    return sites


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def fmt_bytes(n: int) -> str:
    if n >= 1_048_576:
        return f"{n / 1_048_576:.1f} MB"
    if n >= 1_024:
        return f"{n / 1_024:.1f} KB"
    return f"{n} B"


def cmd_summary(profiles: list[Profile]) -> None:
    if not profiles:
        print("No profiles found.")
        return

    phases = sorted({p.phase for p in profiles})
    for phase in phases:
        phase_profiles = [p for p in profiles if p.phase == phase]
        print(f"\n{'=' * 80}")
        print(f"  Phase: {phase}")
        print(f"{'=' * 80}")
        print(
            f"  {'Book':<40} {'Total Alloc':>12} {'Peak Heap':>12} "
            f"{'Blocks':>10} {'Leaked':>10}"
        )
        print(f"  {'-' * 76}")
        for p in phase_profiles:
            leaked = f"{fmt_bytes(p.end_bytes)}" if p.end_bytes > 0 else "0"
            print(
                f"  {p.name:<40} {fmt_bytes(p.total_bytes):>12} "
                f"{fmt_bytes(p.peak_bytes):>12} {p.total_blocks:>10,} {leaked:>10}"
            )


def cmd_hotspots(profiles: list[Profile], n: int) -> None:
    if not profiles:
        print("No profiles found.")
        return
    sites = aggregate_sites(profiles)
    ranked = sorted(sites.items(), key=lambda x: x[1].total_bytes, reverse=True)[:n]

    phase = profiles[0].phase
    print(f"\nTop {n} allocation hotspots (phase={phase}, {len(profiles)} books)")
    print(f"{'─' * 110}")
    print(f"  {'Function':<65} {'Total':>12} {'Peak':>10} {'Blocks':>10}")
    print(f"  {'─' * 99}")
    for fn, s in ranked:
        print(
            f"  {shorten_fn(fn, 63):<65} {fmt_bytes(s.total_bytes):>12} "
            f"{fmt_bytes(s.peak_bytes):>10} {s.total_blocks:>10,}"
        )


def cmd_churn(profiles: list[Profile], n: int) -> None:
    if not profiles:
        print("No profiles found.")
        return
    sites = aggregate_sites(profiles)
    # Short-lived: high total, low peak (max mb < 1KB)
    churners = [
        (fn, s)
        for fn, s in sites.items()
        if s.total_bytes > 50_000 and s.peak_max < 1024
    ]
    churners.sort(key=lambda x: x[1].total_bytes, reverse=True)
    churners = churners[:n]

    phase = profiles[0].phase
    print(f"\nShort-lived allocations (phase={phase}) — high churn, near-zero peak")
    print("These alloc+free rapidly. On embedded, each hit costs allocator overhead.")
    print(f"{'─' * 110}")
    print(f"  {'Function':<65} {'Total':>12} {'Blocks':>10} {'MaxPeak':>10}")
    print(f"  {'─' * 99}")
    for fn, s in churners:
        print(
            f"  {shorten_fn(fn, 63):<65} {fmt_bytes(s.total_bytes):>12} "
            f"{s.total_blocks:>10,} {fmt_bytes(s.peak_max):>10}"
        )


def cmd_peak(profiles: list[Profile], n: int) -> None:
    if not profiles:
        print("No profiles found.")
        return
    sites = aggregate_sites(profiles)
    ranked = sorted(sites.items(), key=lambda x: x[1].peak_bytes, reverse=True)[:n]

    phase = profiles[0].phase
    total_peak = sum(p.peak_bytes for p in profiles)
    print(f"\nPeak heap breakdown (phase={phase}, total={fmt_bytes(total_peak)})")
    print("What's alive at the moment of maximum heap usage.")
    print(f"{'─' * 100}")
    print(f"  {'Function':<65} {'At Peak':>12} {'% of Peak':>10}")
    print(f"  {'─' * 89}")
    for fn, s in ranked:
        pct = (s.peak_bytes / total_peak * 100) if total_peak > 0 else 0
        if s.peak_bytes == 0:
            continue
        print(f"  {shorten_fn(fn, 63):<65} {fmt_bytes(s.peak_bytes):>12} {pct:>9.1f}%")


def parse_size(s: str) -> int:
    """Parse human-readable size like '512KB', '4MB', '1024'."""
    s = s.strip().upper()
    if s.endswith("KB"):
        return int(float(s[:-2]) * 1024)
    if s.endswith("MB"):
        return int(float(s[:-2]) * 1024 * 1024)
    if s.endswith("GB"):
        return int(float(s[:-2]) * 1024 * 1024 * 1024)
    return int(s)


def cmd_budget(profiles: list[Profile], target: int) -> None:
    if not profiles:
        print("No profiles found.")
        return
    phases = sorted({p.phase for p in profiles})
    print(f"\nBudget check: target = {fmt_bytes(target)}")
    print(f"{'─' * 80}")

    any_over = False
    for phase in phases:
        phase_profiles = [p for p in profiles if p.phase == phase]
        print(f"\n  Phase: {phase}")
        for p in phase_profiles:
            pct = p.peak_bytes / target * 100 if target > 0 else 0
            status = "PASS" if p.peak_bytes <= target else "OVER"
            if status == "OVER":
                any_over = True
            marker = "  " if status == "PASS" else ">>"
            print(
                f"  {marker} {p.name:<40} {fmt_bytes(p.peak_bytes):>10} "
                f"({pct:5.1f}%)  [{status}]"
            )

    print()
    if any_over:
        print(f"FAIL: Some profiles exceed {fmt_bytes(target)} budget.")
        sys.exit(1)
    else:
        print(f"OK: All profiles within {fmt_bytes(target)} budget.")


def cmd_compare(path_a: str, path_b: str, n: int) -> None:
    a = parse_profile(path_a)
    b = parse_profile(path_b)

    print(f"\nComparing:")
    print(f"  A: {os.path.basename(path_a)}")
    print(f"  B: {os.path.basename(path_b)}")
    print(f"{'─' * 60}")
    print(f"  {'Metric':<25} {'A':>14} {'B':>14} {'Delta':>14}")
    print(f"  {'─' * 50}")

    def row(label: str, va: int, vb: int) -> None:
        delta = vb - va
        sign = "+" if delta > 0 else ""
        print(
            f"  {label:<25} {fmt_bytes(va):>14} {fmt_bytes(vb):>14} "
            f"{sign}{fmt_bytes(abs(delta)):>13}"
        )

    row("Total allocated", a.total_bytes, b.total_bytes)
    row("Peak heap", a.peak_bytes, b.peak_bytes)
    row("Leaked at end", a.end_bytes, b.end_bytes)
    print(
        f"  {'Total blocks':<25} {a.total_blocks:>14,} {b.total_blocks:>14,} "
        f"{b.total_blocks - a.total_blocks:>+14,}"
    )

    # Per-function delta
    sites_a = {}
    for pp in a.pps:
        fn = owner_fn(pp, a.ftbl)
        sites_a[fn] = sites_a.get(fn, 0) + pp["tb"]
    sites_b = {}
    for pp in b.pps:
        fn = owner_fn(pp, b.ftbl)
        sites_b[fn] = sites_b.get(fn, 0) + pp["tb"]

    all_fns = set(sites_a) | set(sites_b)
    deltas = []
    for fn in all_fns:
        va = sites_a.get(fn, 0)
        vb = sites_b.get(fn, 0)
        deltas.append((fn, va, vb, vb - va))

    deltas.sort(key=lambda x: abs(x[3]), reverse=True)
    print(f"\n  Top {n} changed functions:")
    print(f"  {'Function':<55} {'A':>10} {'B':>10} {'Delta':>12}")
    print(f"  {'─' * 89}")
    for fn, va, vb, delta in deltas[:n]:
        sign = "+" if delta > 0 else ""
        print(
            f"  {shorten_fn(fn, 53):<55} {fmt_bytes(va):>10} "
            f"{fmt_bytes(vb):>10} {sign}{fmt_bytes(abs(delta)):>11}"
        )


def cmd_report(profiles: list[Profile], target: int, n: int) -> None:
    """Combined report: summary + hotspots + churn + budget."""
    cmd_summary(profiles)

    # Group by phase, show hotspots for each
    phases = sorted({p.phase for p in profiles})
    for phase in phases:
        phase_profiles = [p for p in profiles if p.phase == phase]
        print()
        cmd_hotspots(phase_profiles, n)
        print()
        cmd_churn(phase_profiles, n)
        print()
        cmd_peak(phase_profiles, n)

    print()
    cmd_budget(profiles, target)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze DHAT heap profiles for epub-stream.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = parser.add_subparsers(dest="command", required=True)

    # Common args
    def add_common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--dir", default="target/memory", help="Profile directory")
        p.add_argument("--phase", default=None, help="Filter by phase")

    # summary
    p = sub.add_parser("summary", help="Per-file allocation summary")
    add_common(p)

    # hotspots
    p = sub.add_parser("hotspots", help="Top allocation sites by total bytes")
    add_common(p)
    p.add_argument("-n", type=int, default=15, help="Number of entries")

    # churn
    p = sub.add_parser("churn", help="Short-lived (high-churn) allocations")
    add_common(p)
    p.add_argument("-n", type=int, default=15, help="Number of entries")

    # peak
    p = sub.add_parser("peak", help="Peak heap breakdown")
    add_common(p)
    p.add_argument("-n", type=int, default=15, help="Number of entries")

    # budget
    p = sub.add_parser("budget", help="Check peak heap against memory budget")
    add_common(p)
    p.add_argument("--target", default="512KB", help="Budget target (e.g. 512KB, 4MB)")

    # compare
    p = sub.add_parser("compare", help="Compare two profiles")
    p.add_argument("file_a", help="First profile JSON")
    p.add_argument("file_b", help="Second profile JSON")
    p.add_argument("-n", type=int, default=10, help="Number of changed functions")

    # report
    p = sub.add_parser("report", help="Full analysis report")
    add_common(p)
    p.add_argument("-n", type=int, default=15, help="Number of entries per section")
    p.add_argument("--target", default="512KB", help="Budget target (e.g. 512KB, 4MB)")

    args = parser.parse_args()

    if args.command == "compare":
        cmd_compare(args.file_a, args.file_b, args.n)
        return

    profiles = load_profiles(args.dir, args.phase)

    if args.command == "summary":
        cmd_summary(profiles)
    elif args.command == "hotspots":
        cmd_hotspots(profiles, args.n)
    elif args.command == "churn":
        cmd_churn(profiles, args.n)
    elif args.command == "peak":
        cmd_peak(profiles, args.n)
    elif args.command == "budget":
        cmd_budget(profiles, parse_size(args.target))
    elif args.command == "report":
        cmd_report(profiles, parse_size(args.target), args.n)


if __name__ == "__main__":
    main()
