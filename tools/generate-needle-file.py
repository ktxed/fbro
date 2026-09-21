#!/usr/bin/env python3
"""Generate a large text file with a marker at selected percentage offsets."""

from __future__ import annotations

import argparse
import re
from decimal import Decimal, InvalidOperation
from pathlib import Path


DEFAULT_NEEDLE = "THIS_IS_THE_NEEDLE"
DEFAULT_POSITIONS = (Decimal("10"), Decimal("50"), Decimal("80"))
WRITE_CHUNK_SIZE = 1024 * 1024
FILLER_LINE = b"0123456789abcdef" * 7 + b"0123456789abc\n"  # 128 bytes

SIZE_UNITS = {
    "": 1,
    "b": 1,
    "kb": 1_000,
    "mb": 1_000_000,
    "gb": 1_000_000_000,
    "tb": 1_000_000_000_000,
    "kib": 1024,
    "mib": 1024**2,
    "gib": 1024**3,
    "tib": 1024**4,
}


def parse_size(value: str) -> int:
    match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*([a-zA-Z]*)\s*", value)
    if not match or match.group(2).lower() not in SIZE_UNITS:
        raise argparse.ArgumentTypeError(
            "size must be bytes or use KB, MB, GB, TB, KiB, MiB, GiB, or TiB"
        )
    size = Decimal(match.group(1)) * SIZE_UNITS[match.group(2).lower()]
    if size != size.to_integral_value() or size <= 0:
        raise argparse.ArgumentTypeError("size must resolve to a positive whole number of bytes")
    return int(size)


def parse_percentage(value: str) -> Decimal:
    value = value.strip().removesuffix("%").strip()
    try:
        percentage = Decimal(value)
    except InvalidOperation as error:
        raise argparse.ArgumentTypeError(f"invalid percentage: {value}") from error
    if not Decimal(0) <= percentage <= Decimal(100):
        raise argparse.ArgumentTypeError("percentages must be between 0 and 100")
    return percentage


def marker_offsets(file_size: int, marker_size: int, positions: list[Decimal]) -> list[int]:
    if marker_size > file_size:
        raise ValueError("needle is larger than the requested file")

    # At 100%, place the marker as late as possible while preserving exact size.
    last_start = file_size - marker_size
    offsets = [min(int(file_size * position / 100), last_start) for position in positions]
    ordered = sorted(offsets)
    for left, right in zip(ordered, ordered[1:]):
        if right < left + marker_size:
            raise ValueError("needle positions overlap; use a larger file or wider spacing")
    return offsets


def write_filler(output: Path, size: int) -> None:
    chunk = (FILLER_LINE * (WRITE_CHUNK_SIZE // len(FILLER_LINE) + 1))[:WRITE_CHUNK_SIZE]
    remaining = size
    with output.open("wb") as file:
        while remaining:
            amount = min(remaining, len(chunk))
            file.write(chunk[:amount])
            remaining -= amount


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate an exact-size text file containing a needle at percentage offsets."
    )
    parser.add_argument("output", type=Path, help="output file path")
    parser.add_argument("--size", required=True, type=parse_size, help="e.g. 4MiB, 2.5GB, or 4194304")
    parser.add_argument("--needle", default=DEFAULT_NEEDLE, help=f"marker text (default: {DEFAULT_NEEDLE})")
    parser.add_argument(
        "--positions",
        nargs="+",
        type=parse_percentage,
        default=list(DEFAULT_POSITIONS),
        metavar="PERCENT",
        help="marker positions (default: 10 50 80; a trailing %% is optional)",
    )
    parser.add_argument("--force", action="store_true", help="overwrite an existing output file")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    marker = args.needle.encode("utf-8")
    if not marker:
        raise SystemExit("error: needle must not be empty")
    if args.output.exists() and not args.force:
        raise SystemExit(f"error: {args.output} already exists (use --force to overwrite it)")

    try:
        offsets = marker_offsets(args.size, len(marker), args.positions)
    except ValueError as error:
        raise SystemExit(f"error: {error}") from error

    args.output.parent.mkdir(parents=True, exist_ok=True)
    write_filler(args.output, args.size)
    with args.output.open("r+b") as file:
        for offset in offsets:
            file.seek(offset)
            file.write(marker)

    print(f"Created {args.output} ({args.size} bytes)")
    for position, offset in zip(args.positions, offsets):
        print(f"  {position}%: byte offset {offset}, needle {args.needle!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
