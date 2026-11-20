#!/usr/bin/env python3
"""Record implementation status and scope in the Phase 0 feature matrix.

`status` is the only field in `spec/matrix.json` that implementation work may
rewrite; everything else is derived from the manual by `tools/crawl/crawl.py`.
This script owns that field, so the review is explicit, reviewable and can be
re-applied after the crawler regenerates the matrix.

Every row is classified, not just the chapters a phase happened to touch:

- `STATUS` is the row-level audit from the phase docs and the completeness
  review. It always wins.
- `CHAPTER_STATUS` is the deliberate default for rows not named in `STATUS`.
  There is no implicit "not-started" fallback: a chapter without a default is
  an error, so a newly crawled chapter cannot slip in unclassified.
- the command-keyword chapter gets a conservative second pass: a keyword the
  parser recognises is `in-progress` (the word exists, the manual semantics are
  not row-audited); every other keyword is `not-started`.
- every `out-of-scope` row must have a reason, either a row reason or the
  chapter's reason. `--check` fails when one is missing.

The script writes `spec/matrix.json`, `spec/matrix.md` and
`docs/COMPLETENESS.md`. The ledger is generated from the same tables as the
statuses, so the scope review cannot drift from the matrix.

Statuses mean:
  implemented   the feature works and is covered by tests;
  in-progress   part of the feature exists, the rest is deliberately deferred;
  not-started   not built yet, or not designed yet;
  out-of-scope  not a feature of this product, with a recorded reason;
  verified      implemented and proven on hardware (no row uses this yet).

Usage:
  python3 tools/spec_status.py            # write matrix.json, matrix.md, ledger
  python3 tools/spec_status.py --check    # exit 1 when any of them is stale
"""

from __future__ import annotations

import json
import sys
from collections import Counter, defaultdict
from pathlib import Path

SPEC = Path(__file__).resolve().parent.parent / "spec"
LEDGER = Path(__file__).resolve().parent.parent / "docs" / "COMPLETENESS.md"


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
    # Networked world servers and parameter distribution are not a single-desk
    # feature; see the completeness ledger.
    **expand(
        "SYSTEM",
        [
            ("implemented", "1 2"),
            ("out-of-scope", "3 4 5 6 7"),
        ],
    ),
    # The headless engine is the onPC equivalent; installers, OS tuning and the
    # vendor terminal app are not product features.
    **expand(
        "ONPC",
        [
            ("in-progress", "1"),
            ("out-of-scope", "2 3 4 5 6 7 8"),
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
            ("not-started", "3 4 5 6 8 9 12 13 14 18"),
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

# Phase 4 — timecode, move in black, undo and the first UI shell (docs/PHASE4.md).
# Undo has no matrix row of its own; it is a console behaviour, not a chapter.
PHASE4 = {
    **expand(
        "TIMECODE",
        [
            ("implemented", "1 9"),  # the pool and creating a show
            # text viewer, tracks/markers, cue and effect events, sources; no
            # track groups, time ranges or OSC ingestion yet
            ("in-progress", "2 4 5 7 8 10 11"),
            ("not-started", "3 6"),
        ],
    ),
    **expand("CUE_SEQUENCE", [("implemented", "16")]),  # move in black
    **expand("OPERATE_FIXTURES", [("in-progress", "3")]),  # text fixture sheet
    **expand("WORKSPACE", [("in-progress", "2")]),  # the terminal UI shell
    **expand(
        "COMMAND_SYNTAX_KEYWORDS",
        [("implemented", "322")],  # the Timecode keyword
    ),
}

# Row-level refinements found by the completeness review. These are intentionally
# after the phase tables: the phase tables record what a phase claimed, this table
# records what the full-manual audit found.
EXTRA = {
    # The workspace's command line is implemented; desk lock landed in item 06;
    # the command area and master/playback bars exist in the terminal shell.
    "CX-WORKSPACE-0001": "in-progress",
    "CX-WORKSPACE-0004": "implemented",
    "CX-WORKSPACE-0014": "in-progress",
    "CX-WORKSPACE-0016": "in-progress",
    "CX-WORKSPACE-0017": "in-progress",
    # 3D visualisation is explicitly outside the design set.
    "CX-PATCH-0010": "out-of-scope",
    "CX-PATCH-0011": "out-of-scope",
    "CX-PATCH-0012": "out-of-scope",
    "CX-PATCH-0013": "out-of-scope",
    "CX-PATCH-0014": "out-of-scope",
    "CX-PATCH-0015": "out-of-scope",
    # Operating fixtures exists; the fixture sheet is partial and the
    # programmer is the command-line programmer.
    "CX-OPERATE_FIXTURES-0001": "in-progress",
    "CX-OPERATE_FIXTURES-0002": "implemented",
    "CX-OPERATE_FIXTURES-0007": "implemented",
    # Environmental fixture types depend on 3D/media, which are out of scope.
    "CX-FIXTURE_TYPES-0011": "out-of-scope",
    # The Status command reports system information; desk lights are hardware.
    "CX-SYSTEM_INFORMATION-0001": "in-progress",
    "CX-SYSTEM_INFORMATION-0004": "out-of-scope",
    "CX-SYSTEM_INFORMATION-0005": "in-progress",
    # Networked position tracking and MVR exchange are outside the design set.
    "CX-REMOTE_INPUTS-0013": "out-of-scope",
    "CX-REMOTE_INPUTS-0014": "out-of-scope",
    # SFTP to a vendor console and its folder layout are vendor features.
    "CX-FILE_MANAGEMENT-0002": "out-of-scope",
    "CX-FILE_MANAGEMENT-0003": "out-of-scope",
    # The syntax itself exists; these topic rows are not individual features.
    "CX-COMMAND_SYNTAX_KEYWORDS-0001": "in-progress",
    "CX-COMMAND_SYNTAX_KEYWORDS-0002": "implemented",
    "CX-COMMAND_SYNTAX_KEYWORDS-0003": "not-started",
    "CX-COMMAND_SYNTAX_KEYWORDS-0004": "in-progress",
    "CX-COMMAND_SYNTAX_KEYWORDS-0005": "in-progress",
    "CX-COMMAND_SYNTAX_KEYWORDS-0006": "in-progress",
}

STATUS = {**PHASE1, **PHASE2, **PHASE3, **PHASE4, **EXTRA}

# Keywords the parser in crates/engine/src/command.rs recognises. A keyword here
# is not a claim that the manual's full behaviour is implemented; it moves the
# row from "not-started" to "in-progress" so the remaining audit is visible.
COMMAND_KEYWORDS = {
    "add", "all", "assign", "at", "attribute", "back", "black", "blackout",
    "clear", "cue", "curve", "default", "delay", "delete", "desk", "effect",
    "effects", "executor", "executors", "fade", "filter", "filters", "fixture",
    "fixtures", "flash", "follow", "fps", "full", "gdtf", "go", "grand", "group",
    "groups", "help", "home", "import", "in", "load", "lock", "master", "masters",
    "mib", "mode", "new", "off", "on", "output", "palette", "palettes", "patch",
    "pause", "percent", "rate", "redo", "release", "remove", "save", "sequence",
    "sequences", "sheet", "show", "size", "solo", "spread", "stagger", "start",
    "status", "stop", "store", "thru", "time", "timecode", "timecodes", "timing",
    "to", "trigger", "type", "undo", "unlock", "unpatch", "values", "waveform",
    "world", "worlds",
}

# Default for every row a chapter does not classify explicitly. A chapter with
# no entry here is an error: new chapters must be classified, not defaulted.
CHAPTER_STATUS = {
    "help": "out-of-scope",
    "news": "out-of-scope",
    "about_the_manual": "out-of-scope",
    "device_overview": "out-of-scope",
    "system": "out-of-scope",
    "first_steps": "out-of-scope",
    "onpc": "out-of-scope",
    "show_file_management": "not-started",
    "workspace": "not-started",
    "command_syntax_keywords": "not-started",
    "wvm": "not-started",
    "network": "out-of-scope",
    "dmx": "not-started",
    "user": "out-of-scope",
    "patch": "not-started",
    "operate_fixtures": "not-started",
    "scribbles": "out-of-scope",
    "images": "out-of-scope",
    "screenshot": "out-of-scope",
    "file_formats": "out-of-scope",
    "videos": "out-of-scope",
    "meshes": "out-of-scope",
    "gobos": "out-of-scope",
    "symbols": "out-of-scope",
    "appear": "not-started",
    "notes": "not-started",
    "wvm_pool_label": "not-started",
    "group": "not-started",
    "presets": "not-started",
    "worldfilter": "not-started",
    "matricks": "not-started",
    "cue_sequence": "not-started",
    "executor": "not-started",
    "masters": "not-started",
    "recipes": "not-started",
    "phaser": "not-started",
    "generator": "not-started",
    "bitmap": "out-of-scope",
    "xyz": "not-started",
    "tags": "not-started",
    "macros": "not-started",
    "agenda": "not-started",
    "timers": "not-started",
    "timecode": "not-started",
    "layouts": "not-started",
    "plugins": "not-started",
    "quickeys": "not-started",
    "datapool": "not-started",
    "system_information": "not-started",
    "remote_inputs": "not-started",
    "sound": "out-of-scope",
    "rdm": "not-started",
    "local_settings": "not-started",
    "update": "out-of-scope",
    "fixture_types": "not-started",
    "file_management": "not-started",
    "show-creator": "not-started",
    "control_other_ma_devices": "out-of-scope",
    "troubleshooting": "out-of-scope",
    "glossary": "out-of-scope",
    "key_grandma3_listoftrademarks": "out-of-scope",
    "qsg": "out-of-scope",
    "grandma3_quick_manual_consoles": "out-of-scope",
    "grandma3_quick_manual_processing_units": "out-of-scope",
    "grandma3_quick_manual_nodes": "out-of-scope",
    "grandma3_quick_manual_nodes_din-rail": "out-of-scope",
    "grandma3_quick_manual_onpc_command_wing_xt": "out-of-scope",
    "grandma3_quick_manual_onpc_command_wing": "out-of-scope",
    "grandma3_quick_manual_onpc_fader_wing": "out-of-scope",
    "grandma3_quick_manual_viz-key": "out-of-scope",
    "grandma3_quick_manual_onpc_rack-unit": "out-of-scope",
    "grandma3_quick_manual_i_o_nodes": "out-of-scope",
    "release_notes": "out-of-scope",
}

# Every out-of-scope row needs one of these, selected by row id first. A chapter
# reason applies to every out-of-scope row in that chapter.
CHAPTER_REASONS = {
    "help": "Vendor manual entry point; the project ships its own README, docs and `Help` command.",
    "news": "Vendor release notes; the project keeps its own changelog.",
    "about_the_manual": "Vendor documentation browser; the project ships its own docs.",
    "device_overview": "Describes vendor consoles, wings, nodes and hardware; this is an independent software desk.",
    "first_steps": "Vendor onboarding for consoles; the project's quick start is in README.md.",
    "system": "Networked sessions, world server and parameter distribution are outside the single-desk design.",
    "onpc": "Vendor onPC installer, OS tuning, terminal app and local settings; the project ships its own Rust binary.",
    "network": "Networked sessions and multi-user collaboration are outside the single-operator design.",
    "user": "User accounts, ownership and screen configuration are outside the single-operator design.",
    "dmx": "Ethernet DMX (Art-Net/sACN) is excluded by the serial-DMX constraint.",
    "scribbles": "Bitmap scribble editor is outside the design set; text labels are supported.",
    "images": "Image assets are outside a serial-DMX desk.",
    "screenshot": "OS screenshot capture is not a desk feature.",
    "file_formats": "Vendor file-format reference; the project documents its own formats.",
    "videos": "Video playback is outside a serial-DMX desk.",
    "meshes": "3D meshes are outside the design set.",
    "gobos": "Gobo image assets are outside a serial-DMX desk.",
    "symbols": "Symbol assets are outside a serial-DMX desk.",
    "bitmap": "Bitmap/pixel mapping is outside the design set.",
    "sound": "Sound playback is outside a serial-DMX desk.",
    "update": "Vendor console/node update; the project has no auto-update.",
    "control_other_ma_devices": "Controlling vendor devices and RemoteHID is outside the independent software scope.",
    "troubleshooting": "Vendor troubleshooting and panic macro; the project has `Status` and docs.",
    "glossary": "Vendor glossary; the project keeps its own documentation.",
    "key_grandma3_listoftrademarks": "Vendor trademark page.",
    "qsg": "Vendor quick start guide; the project's quick start is in README.md.",
    "grandma3_quick_manual_consoles": "Vendor hardware manual.",
    "grandma3_quick_manual_processing_units": "Vendor hardware manual.",
    "grandma3_quick_manual_nodes": "Vendor hardware manual.",
    "grandma3_quick_manual_nodes_din-rail": "Vendor hardware manual.",
    "grandma3_quick_manual_onpc_command_wing_xt": "Vendor hardware manual.",
    "grandma3_quick_manual_onpc_command_wing": "Vendor hardware manual.",
    "grandma3_quick_manual_onpc_fader_wing": "Vendor hardware manual.",
    "grandma3_quick_manual_viz-key": "Vendor hardware manual.",
    "grandma3_quick_manual_onpc_rack-unit": "Vendor hardware manual.",
    "grandma3_quick_manual_i_o_nodes": "Vendor hardware manual.",
    "release_notes": "Vendor release notes; the project keeps its own changelog.",
    "patch": "3D visualisation, stages, cameras and render quality are outside the design set.",
    "workspace": "Trackpad, gestures, vendor displays and wing bar are hardware.",
    "file_management": "SFTP to a vendor console and its folder layout are outside the independent software scope.",
    "system_information": "Desk lights are hardware.",
    "remote_inputs": "PosiStageNet tracking and MVR-xchange are networked integrations outside the design set.",
    "fixture_types": "Environmental fixture types depend on 3D/media, outside the design set.",
}

# What a chapter with remaining work needs. The ledger prints this next to the
# not-started/in-progress counts, so every gap has a named next step.
CHAPTER_NOTES = {
    "system": "The standalone desk is built; networked parameter distribution is out of scope.",
    "onpc": "The headless `openlight` binary is the onPC equivalent; vendor installer/OS rows are out of scope.",
    "show_file_management": "Backup/template shows and file organisation need a small design.",
    "workspace": "Terminal shell exists; windows, panels and themes are item 18.",
    "command_syntax_keywords": "Parser-recognized keywords are in-progress; the rest need a row-by-row audit.",
    "wvm": "Windows/views/menus are item 18.",
    "dmx": "Output exists; DMX In and merge need a bidirectional transport design.",
    "patch": "Item 11 adds patch tools; item 13 adds MVR; 3D rows are out of scope.",
    "operate_fixtures": "Selection/programmer exist; the sheet is partial; colour picker, gels and Align need designs.",
    "wvm_pool_label": "Pool labels are partial; editing labels needs a command/UI.",
    "group": "Add/remove member editing needs a command.",
    "presets": "Recipe presets need a design.",
    "worldfilter": "At Filter window is a GUI feature.",
    "matricks": "Needs a design (blocks, groups, wings, widths, shuffle, transform).",
    "cue_sequence": "Item 10 covers blocking/shield/replace/move; content sheet, settings and cue recipes need designs.",
    "executor": "Special executors and executor pages need designs.",
    "masters": "Selected, time and timing masters need designs.",
    "recipes": "Needs a design.",
    "phaser": "Item 14 supplies multi-attribute/phase effects; the phaser editor and circle/rainbow presets still need a design.",
    "generator": "Needs a design.",
    "xyz": "Needs a design (XYZ attribute space and marker fixtures).",
    "tags": "Needs a small design.",
    "macros": "Item 16 adds macros.",
    "agenda": "Needs a design.",
    "timers": "Needs a design.",
    "timecode": "Item 15 adds tracks, markers and MTC; OSC ingestion and track groups still need designs.",
    "layouts": "Item 18 GUI; layouts have no design yet.",
    "plugins": "Largest undecided chapter: embedded Lua host vs native plugin API; needs a scope decision and design.",
    "quickeys": "Needs a design (GUI/CLI shortcuts).",
    "datapool": "Needs a small design.",
    "system_information": "Status covers part; date/time, clock and monitor windows need designs.",
    "remote_inputs": "Item 12 covers OSC/web; MIDI, DMX and DC remotes need designs.",
    "rdm": "Needs a design; requires a read path in the DMX transport.",
    "local_settings": "Needs a small settings design.",
    "fixture_types": "Item 13 adds the fixture builder/export; environmental types are out of scope.",
    "file_management": "A file browser needs a design; SFTP/folder layout are out of scope.",
    "show-creator": "Needs a design.",
    "appear": "Pool appearance metadata needs a small design.",
    "notes": "Notes on objects and cues need a small design.",
}

# Designs that are larger than one manual chapter and therefore cannot be read
# off the chapter table. Ordered by value to a working desk.
CROSS_CUTTING = [
    ("Colour model", "`ColorSub_*`/CMY, HSB, colour wheels, CTO and gels. Additive RGB only is wrong for a large class of real profiles; affects palettes, cues, effects and the GUI swatch."),
    ("DMX In and merge", "The `dmx` chapter's first row is only half built: the transport is write-only, so there is no backup desk, house console or DMX-triggered snapshot."),
    ("RDM", "Discovery and remote patching; shares the bidirectional transport work with DMX In."),
    ("MAtricks, Shuffle and Align", "Programming speed for multi-fixture edits; the whole `matricks` chapter and `Align` are unbuilt."),
    ("Recipes and Generator", "Look generation beyond a single waveform; `recipes`, `generator` and recipe presets are unbuilt."),
    ("Executor pages and special executors", "The executor list is flat; pages, assignments and special executors are missing."),
    ("Timecode depth", "Tracks, markers, MTC and an OSC feed hook landed with item 15; OSC ingestion and track groups remain."),
    ("Macros and CLI ergonomics", "Macros, variables, transaction undo and machine-readable status are item 16."),
    ("Layouts, Quickeys and windows", "Layouts/Quickeys have no design; the five-pane GUI is item 18."),
    ("Agenda and timers", "Scheduled events, stopwatch and countdown are unbuilt."),
    ("Plugins/Lua", "164 rows, the largest undecided chapter. Decide between an embedded Lua host and a native plugin API before claiming completeness."),
    ("Show Creator", "Import/export and preset/group creation from fixture types; no design yet."),
    ("File management and backup", "Backup/template shows, a file browser and show organisation need a design."),
    ("Remote inputs", "MIDI, DMX and DC remotes; item 12 only covers OSC/web and item 15 only covers MTC."),
    ("Small metadata features", "Appearances, notes, tags, data pools and XYZ are small designs that still need to exist for a full manual replica."),
]


def chapter_ext_ids(matrix: dict) -> dict[str, str]:
    return {chapter["chapter"]: chapter["chapter_ext_id"] for chapter in matrix["chapters"]}


def classify(matrix: dict) -> dict[str, int]:
    """Write a status and (for out-of-scope rows) a reason onto every row."""
    vocabulary = set(matrix["meta"]["status_vocabulary"])
    chapter_by_name = chapter_ext_ids(matrix)
    rows = matrix["rows"]

    unknown_chapters = sorted(set(chapter_by_name.values()) - set(CHAPTER_STATUS))
    if unknown_chapters:
        raise ValueError(f"chapters without a default status: {', '.join(unknown_chapters)}")

    unknown_reasons = sorted(set(CHAPTER_REASONS) - set(chapter_by_name.values()))
    if unknown_reasons:
        raise ValueError(f"reasons for unknown chapters: {', '.join(unknown_reasons)}")

    unknown_notes = sorted(set(CHAPTER_NOTES) - set(chapter_by_name.values()))
    if unknown_notes:
        raise ValueError(f"notes for unknown chapters: {', '.join(unknown_notes)}")

    ids = {row["id"] for row in rows}
    unknown_rows = sorted(set(STATUS) - ids)
    if unknown_rows:
        raise ValueError(f"unknown row ids in the status table: {', '.join(unknown_rows)}")

    counts: Counter[str] = Counter()
    for row in rows:
        chapter = chapter_by_name.get(row["chapter"])
        if chapter is None:
            raise ValueError(f"{row['id']}: chapter `{row['chapter']}` is not in the matrix")

        status = STATUS.get(row["id"])
        if status is None and chapter == "command_syntax_keywords":
            token = (row.get("token") or row["name"]).strip().lstrip("/").lower()
            if token in COMMAND_KEYWORDS:
                status = "in-progress"
        if status is None:
            status = CHAPTER_STATUS[chapter]

        if status not in vocabulary:
            raise ValueError(f"{row['id']}: invalid status `{status}`")
        if status == "out-of-scope" and not CHAPTER_REASONS.get(chapter):
            raise ValueError(f"{row['id']}: out-of-scope without a reason")

        row["status"] = status
        counts[status] += 1

    remaining = {
        chapter_by_name[row["chapter"]]
        for row in rows
        if row["status"] in ("not-started", "in-progress")
    }
    missing_notes = sorted(remaining - set(CHAPTER_NOTES))
    if missing_notes:
        raise ValueError(
            "chapters with remaining work but no next-step note: "
            + ", ".join(missing_notes)
        )

    return dict(counts)


def status_summary(counts: Counter[str], vocabulary: list[str]) -> str:
    parts = [f"{counts[status]} {status}" for status in vocabulary if counts.get(status)]
    return ", ".join(parts) if parts else "—"


def render_ledger(matrix: dict) -> str:
    rows = matrix["rows"]
    chapter_by_name = chapter_ext_ids(matrix)
    vocabulary = matrix["meta"]["status_vocabulary"]
    by_chapter: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        by_chapter[row["chapter"]].append(row)

    total = Counter(row["status"] for row in rows)
    lines = [
        "# Completeness ledger",
        "",
        "Generated by `python3 tools/spec_status.py`; edit the classification tables "
        "in that script, not this file. The same tables write `spec/matrix.json`.",
        "",
        "## What \"complete\" means",
        "",
        "A row is classified when it has one of the statuses below. An "
        "`out-of-scope` row additionally needs a reason. Every chapter has a "
        "deliberate default, so a newly crawled chapter cannot appear as an "
        "implicit `not-started` row.",
        "",
        "This ledger records scope and implementation evidence; it does not claim "
        "that a `not-started` or `in-progress` row works. `verified` is reserved "
        "for behaviour proven on real DMX hardware, and no row uses it yet.",
        "",
        "| Status | Rows | Meaning |",
        "|---|---|---|",
        f"| implemented | {total.get('implemented', 0)} | Works and is covered by tests. |",
        f"| in-progress | {total.get('in-progress', 0)} | Part of the feature exists; the rest is deferred. |",
        f"| not-started | {total.get('not-started', 0)} | Not built, or not designed yet. |",
        f"| out-of-scope | {total.get('out-of-scope', 0)} | Not a feature of this product; reason recorded below. |",
        f"| verified | {total.get('verified', 0)} | Implemented and proven on hardware. |",
        "",
        f"Total: **{len(rows)} rows** from the Phase 0 crawl of the manual.",
        "",
        "## Chapters",
        "",
        "| Phase | Chapter | Module | Rows | Status | Disposition |",
        "|---|---|---|---|---|---|",
    ]

    for chapter in matrix["chapters"]:
        chapter_rows = by_chapter[chapter["chapter"]]
        counts = Counter(row["status"] for row in chapter_rows)
        disposition = CHAPTER_NOTES.get(chapter["chapter_ext_id"], "")
        if not disposition and counts.get("out-of-scope") == len(chapter_rows):
            disposition = CHAPTER_REASONS[chapter["chapter_ext_id"]]
        elif not disposition and counts.get("implemented") == len(chapter_rows):
            disposition = "Built."
        lines.append(
            "| {phase} | {name} | {module} | {rows} | {status} | {disposition} |".format(
                phase=chapter["phase"],
                name=chapter["chapter"],
                module=chapter["module"],
                rows=len(chapter_rows),
                status=status_summary(counts, vocabulary),
                disposition=disposition or "—",
            )
        )

    lines += [
        "",
        "## Out-of-scope rows, with reasons",
        "",
        "| Chapter | Rows | Reason |",
        "|---|---|---|",
    ]

    for chapter in matrix["chapters"]:
        chapter_rows = by_chapter[chapter["chapter"]]
        ext = chapter["chapter_ext_id"]
        groups: dict[str, list[str]] = defaultdict(list)
        for row in chapter_rows:
            if row["status"] == "out-of-scope":
                groups[CHAPTER_REASONS[ext]].append(row["id"])
        for reason, ids in groups.items():
            if len(ids) <= 8:
                label = ", ".join(f"`{row_id}`" for row_id in ids)
            else:
                label = f"{len(ids)} rows"
            lines.append(f"| {chapter['chapter']} | {label} | {reason} |")

    lines += [
        "",
        "## Chapters needing new or extended designs",
        "",
        "| Phase | Chapter | Not started | In progress | Next step |",
        "|---|---|---|---|---|",
    ]

    for chapter in matrix["chapters"]:
        chapter_rows = by_chapter[chapter["chapter"]]
        counts = Counter(row["status"] for row in chapter_rows)
        if not counts.get("not-started") and not counts.get("in-progress"):
            continue
        note = CHAPTER_NOTES.get(chapter["chapter_ext_id"], "No disposition recorded.")
        lines.append(
            f"| {chapter['phase']} | {chapter['chapter']} | "
            f"{counts.get('not-started', 0)} | {counts.get('in-progress', 0)} | {note} |"
        )

    lines += [
        "",
        "## Cross-cutting designs not owned by one chapter",
        "",
        "These are the gaps that a chapter-by-chapter audit would otherwise hide. "
        "Ranked by value to a working desk.",
        "",
    ]
    for index, (name, reason) in enumerate(CROSS_CUTTING, start=1):
        lines.append(f"{index}. **{name}** — {reason}")

    lines += [
        "",
        "## How rows were classified",
        "",
        "- **Phase tables** — the explicit audit in `tools/spec_status.py` for the "
        "chapters Phases 1–4 implemented.",
        "- **Chapter defaults** — the deliberate disposition for every row not named "
        "individually. A chapter without a default fails the check.",
        "- **Command keywords** — a keyword the parser recognises is `in-progress`; "
        "it means the word exists, not that the manual's full behaviour is audited.",
        "- **Out-of-scope** — every such row carries the chapter reason printed above; "
        "`--check` fails if one is missing.",
        "- **Row refinements** — `EXTRA` records the cases where the full-manual audit "
        "found a status the phase table did not anticipate.",
        "",
        "## Verification",
        "",
        "```bash",
        "python3 tools/spec_status.py --check",
        "```",
        "",
        "The check fails when `spec/matrix.json`, `spec/matrix.md` or this ledger is "
        "stale, when a row has no classification, or when an out-of-scope row has no "
        "reason.",
        "",
    ]
    return "\n".join(lines)


def render_markdown(matrix: dict) -> str:
    """Use the crawler's renderer so matrix.md keeps one layout."""
    crawler = Path(__file__).resolve().parent / "crawl"
    sys.path.insert(0, str(crawler))
    try:
        import crawl  # noqa: PLC0415
    finally:
        sys.path.remove(str(crawler))
    return crawl.render_matrix_md(matrix)


def main(argv: list[str]) -> int:
    check = "--check" in argv
    matrix_path = SPEC / "matrix.json"
    markdown_path = SPEC / "matrix.md"
    matrix = json.loads(matrix_path.read_text())

    try:
        counts = classify(matrix)
        json_text = json.dumps(matrix, ensure_ascii=False, indent=1)
        markdown_text = render_markdown(matrix)
        ledger_text = render_ledger(matrix)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1

    if check:
        stale = [
            str(path)
            for path, text in (
                (matrix_path, json_text),
                (markdown_path, markdown_text),
                (LEDGER, ledger_text),
            )
            if not path.exists() or path.read_text() != text
        ]
        if stale:
            print(f"out of date: {', '.join(stale)}", file=sys.stderr)
            print("run `python3 tools/spec_status.py`", file=sys.stderr)
            return 1
        summary = ", ".join(f"{counts[status]} {status}" for status in matrix["meta"]["status_vocabulary"] if counts.get(status))
        print(f"statuses are up to date ({len(matrix['rows'])} rows: {summary})")
        return 0

    matrix_path.write_text(json_text)
    markdown_path.write_text(markdown_text)
    LEDGER.parent.mkdir(parents=True, exist_ok=True)
    LEDGER.write_text(ledger_text)
    summary = ", ".join(f"{counts[status]} {status}" for status in matrix["meta"]["status_vocabulary"] if counts.get(status))
    print(f"updated {len(matrix['rows'])} rows ({summary})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
