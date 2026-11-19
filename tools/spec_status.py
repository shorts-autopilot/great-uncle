#!/usr/bin/env python3
"""Record implementation status in the Phase 0 feature matrix.

`status` is the only field in `spec/matrix.json` that implementation work may
rewrite; everything else is derived from the manual by `tools/crawl/crawl.py`.
This script owns that field, so the review is explicit, reviewable and can be
re-applied after the crawler regenerates the matrix.

Every Phase 1 row must be classified, and the script fails if a row appears or
disappears, so the matrix and the implementation cannot drift silently. Later
phases are classified only for the chapters each phase implements (Phase 2:
presets, cues and sequences, executors; Phase 3: groups, worlds and filters,
masters, phasers); the remaining rows stay `not-started`.

Statuses mean:
  implemented   the feature works and is covered by tests;
  in-progress   part of the feature exists, the rest is deliberately deferred;
  not-started   not built yet;
  out-of-scope  not a feature of this product (OS tuning, vendor hardware).

Usage:
  python3 tools/spec_status.py            # write matrix.json and matrix.md
  python3 tools/spec_status.py --check    # exit 1 when they are out of date
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SPEC = Path(__file__).resolve().parent.parent / "spec"


def expand(prefix: str, table: list[tuple[str, str]]) -> dict[str, str]:
    """`("implemented", "1 2")` -> {"CX-PREFIX-0001": "implemented", ...}."""
    statuses: dict[str, str] = {}
    for status, numbers in table:
        for number in numbers.split():
            row_id = f"CX-{prefix}-{int(number):04d}"
            if row_id in statuses:
                raise ValueError(f"{row_id} is classified twice")
            statuses[row_id] = status
    return statuses


# Phase 1 — engine, show file, command line, DMX output (docs/PHASE1.md).
PHASE1 = {
    # Single process, no network: a standalone desk is what this build is.
    **expand(
        "SYSTEM",
        [
            ("implemented", "1 2"),
            ("not-started", "3 4 5 6 7"),  # networking, world server, parameters
        ],
    ),
    # The headless engine is the onPC equivalent; installers and OS tuning are
    # not product features, and there is no onPC terminal app.
    **expand(
        "ONPC",
        [
            ("in-progress", "1"),
            ("not-started", "2 3 5 8"),
            ("out-of-scope", "4 6 7"),
        ],
    ),
    # Save, load and new are real; backups, demo shows and file organisation
    # are still to come.
    **expand(
        "SHOW_FILE_MANAGEMENT",
        [
            ("implemented", "1 2 3 4"),
            ("not-started", "5 6"),
        ],
    ),
    # The console has a command line; every other row in this chapter is a
    # window, bar or gesture that Phase 1 deliberately does not draw.
    **expand(
        "WORKSPACE",
        [
            ("implemented", "6"),
            ("not-started", "1 2 3 4 5 7 8 9 10 11 14 15 16 17 19 20 21 22 23 24 26 27 28 29 30"),
            ("out-of-scope", "12 13 18 25"),
        ],
    ),
    # No windows yet. Pool objects can be created and deleted by command.
    **expand(
        "WVM",
        [
            ("in-progress", "11 16"),
            ("not-started", "1 2 3 4 5 6 7 8 9 10 12 13 14 15"),
        ],
    ),
    # DMX leaves through a serial port by design, so the Ethernet rows are out
    # of scope. DMX In is not built.
    **expand(
        "DMX",
        [
            ("implemented", "2"),
            ("in-progress", "1"),
            ("out-of-scope", "3 4 5 6"),
        ],
    ),
    **expand(
        "PATCH",
        [
            ("implemented", "1 2 3 6 9 16"),
            ("in-progress", "7 8 18"),  # DMX sheet, universe command, feature groups
            ("not-started", "4 5 10 11 12 13 14 15 17 19 20 21"),
        ],
    ),
    **expand("WVM_POOL_LABEL", [("in-progress", "1")]),
    **expand(
        "FIXTURE_TYPES",
        [
            ("implemented", "1 2 3"),
            ("in-progress", "4"),
            ("not-started", "5 6 7 8 9 10 11 12"),
        ],
    ),
}

# Phase 2 — palettes, cues, sequences and executor faders (docs/PHASE2.md).
PHASE2 = {
    **expand(
        "PRESETS",
        [
            ("implemented", "1 2 3 5"),
            ("in-progress", "6"),  # re-store replaces; no update/merge semantics
            ("not-started", "4"),  # recipe presets
        ],
    ),
    **expand(
        "CUE_SEQUENCE",
        [
            ("implemented", "1 2 10 11 15 17 19"),
            ("in-progress", "7"),  # text listing, no sequence sheet window
            ("not-started", "3 4 5 6 8 9 12 13 14 16 18"),
        ],
    ),
    **expand(
        "EXECUTOR",
        [
            ("implemented", "1 4"),
            ("in-progress", "2 3"),  # fader and sequence assignment only
            ("not-started", "5"),
        ],
    ),
}

# Phase 3 — groups, worlds, filters, masters and effects (docs/PHASE3.md).
PHASE3 = {
    **expand(
        "GROUP",
        [
            ("implemented", "1 2 4 5"),
            ("in-progress", "3"),  # re-store edits; no add/remove member command
        ],
    ),
    **expand(
        "WORLDFILTER",
        [
            ("implemented", "1 2 3"),
            ("not-started", "4"),  # the Filter window is a UI feature
        ],
    ),
    **expand(
        "MASTERS",
        [
            ("implemented", "1 3 5"),
            ("in-progress", "6"),  # executor faders are playback masters
            ("not-started", "2 4 7"),
        ],
    ),
    **expand(
        "PHASER",
        [
            ("implemented", "1 3"),
            ("in-progress", "2"),  # command-line definition, no phaser editor
            ("not-started", "4 5 6"),
        ],
    ),
    **expand("GENERATOR", [("not-started", "1")]),
}

STATUS = {**PHASE1, **PHASE2, **PHASE3}


def main(argv: list[str]) -> int:
    check = "--check" in argv
    matrix_path = SPEC / "matrix.json"
    markdown_path = SPEC / "matrix.md"
    matrix = json.loads(matrix_path.read_text())

    rows = matrix["rows"]
    by_id = {row["id"]: row for row in rows}
    vocabulary = set(matrix["meta"]["status_vocabulary"])

    unknown = sorted(set(STATUS) - set(by_id))
    missing = sorted(row["id"] for row in rows if row["phase"] == 1 and row["id"] not in STATUS)
    invalid = sorted({status for status in STATUS.values() if status not in vocabulary})
    if unknown:
        print(f"unknown row ids in the status table: {', '.join(unknown)}", file=sys.stderr)
        return 1
    if missing:
        print(f"Phase 1 rows without a status: {', '.join(missing)}", file=sys.stderr)
        return 1
    if invalid:
        print(f"invalid statuses: {', '.join(invalid)}", file=sys.stderr)
        return 1

    changed = 0
    for row in rows:
        status = STATUS.get(row["id"])
        if status is None or row["status"] == status:
            continue
        row["status"] = status
        changed += 1

    json_text = json.dumps(matrix, ensure_ascii=False, indent=1)
    markdown_text = render_markdown(matrix)

    if check:
        stale = [
            str(path)
            for path, text in ((matrix_path, json_text), (markdown_path, markdown_text))
            if path.read_text() != text
        ]
        if stale:
            print(f"out of date: {', '.join(stale)}", file=sys.stderr)
            print("run `python3 tools/spec_status.py`", file=sys.stderr)
            return 1
        print(f"statuses are up to date ({len(STATUS)} classified rows)")
        return 0

    matrix_path.write_text(json_text)
    markdown_path.write_text(markdown_text)
    print(f"updated {changed} statuses ({len(STATUS)} classified rows)")
    return 0


def render_markdown(matrix: dict) -> str:
    """Use the crawler's renderer so matrix.md keeps one layout."""
    crawler = Path(__file__).resolve().parent / "crawl"
    sys.path.insert(0, str(crawler))
    try:
        import crawl  # noqa: PLC0415
    finally:
        sys.path.remove(str(crawler))
    return crawl.render_matrix_md(matrix)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
