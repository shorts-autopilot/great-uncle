# Phase 0 crawl report

- Source: https://help.malighting.com/grandMA3/2.2/
- Manual version: 2.2
- TOC nodes: 1223
- Unique topics: 1223
- Parsed: 1223
- Deferred by --limit: 0
- Failed/missing: 0
- Topics with content: 1223
- Matrix rows (topics): 1223
- Matrix sections: 4881

## Topics per phase

| Phase | Topics |
|---|---|
| 0 | 293 |
| 1 | 107 |
| 2 | 498 |
| 3 | 43 |
| 4 | 28 |
| 5 | 60 |
| 6 | 172 |
| 7 | 11 |
| 8 | 11 |

## Topics per kind

| Kind | Rows |
|---|---|
| `topic` | 646 |
| `keyword` | 347 |
| `lua-function` | 156 |
| `option-keyword` | 74 |

Extracted option/setting items: **21254**

## Topics per module

| Module | Topics |
|---|---|
| `cmd` | 427 |
| `hardware` | 256 |
| `script` | 164 |
| `docs` | 37 |
| `ui/shell` | 30 |
| `programmer` | 27 |
| `patch` | 21 |
| `playback` | 19 |
| `timecode` | 19 |
| `ui/windows` | 17 |
| `platform` | 16 |
| `session` | 16 |
| `io` | 14 |
| `ui/assets` | 13 |
| `pools` | 12 |
| `fixturetypes` | 12 |
| `showfile` | 10 |
| `system` | 9 |
| `cmd/macro` | 9 |
| `media` | 8 |
| `ui/layout` | 8 |
| `engine/parameters` | 7 |
| `matrix` | 7 |
| `playback/masters` | 7 |
| `showtools` | 7 |
| `docs/help` | 6 |
| `output` | 6 |
| `phaser` | 6 |
| `users` | 5 |
| `playback/exec` | 5 |
| `diagnostics` | 5 |
| `filter` | 4 |
| `ui/pools` | 4 |
| `session/remotedesk` | 4 |
| `io/media` | 3 |
| `playback/recipes` | 1 |
| `generator` | 1 |
| `io/rdm` | 1 |

## Topics per scope

| Scope | Topics |
|---|---|
| hardware | 256 |
| legal | 1 |
| meta | 36 |
| product | 930 |

## Files

- `spec/manual.json` — TOC tree + every parsed topic (headings, paragraphs, lists, tables, notes, links).
- `spec/topics.jsonl` — flat one-line-per-topic index for `jq`/grep tooling.
- `spec/matrix.json` — feature matrix rows (topic level + section level) with module, phase, status.
- `spec/matrix.md` — readable rendering of the matrix.
- `spec/REPORT.md` — this file.
- `.cache/manual/` — raw HTML cache (untracked), makes re-runs resumable.
