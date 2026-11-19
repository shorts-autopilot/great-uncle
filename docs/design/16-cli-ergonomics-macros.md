# 16 · CLI ergonomics, macros and undo grouping

**Rank** 16 · **Order** 16 · **Size** M · **Show** — · **Desk** d1 (additive: macros) · **Depends on** 04 (journal), 11 (reports)

## Problem

The binary is a good prototype CLI and a poor product. It has no `--version`, no
`--check`, no way to pass a command on the command line, no machine-readable
status, and its scripts can only run from a file. There is no way to package a
repeated sequence of commands ("blackout, wait, restore"), and undo is
statement-granular, so `Patch Import` of 24 fixtures is 24 undo steps.

## Behaviour when this is done

```
openlight --version
openlight --check "look.olshow"                    exit 0 or 1, problems on stdout
openlight --command "Fixture 1 At 50" --command "Store Cue 1" --frames 10
openlight --script - < examples/phase1-demo.txt
openlight --status --json | jq .universes
```

```
Macro 1 "Blackout cue" Blackout; Wait 2; Blackout Off
Macro 1 Run
Begin; Patch Import "rig.csv"; Report Patch "patch.csv"; End   one undo step
```

## Design

### Flags

| Flag | Purpose |
|---|---|
| `--version` | package version and show format version |
| `--command <line>` | one statement, repeatable, before `--script` |
| `--script -` | read the script from stdin |
| `--check [file]` | load, validate, print problems, exit 0/1; no output thread |
| `--status [--json]` | print the status once and exit |
| `--quiet` | suppress startup lines, keep command output |
| `--log <file>` | append every executed line and its result (a session log, distinct from the journal) |
| `--desk <file>` | the desk file to use (item 03) |
| `--autosave <secs>` / `--journal off` / `--no-recover` | item 04 |
| `--completions <shell>` | print a completion script (bash, zsh, fish) |

- `--command` and `--script` compose in order: every `--command` in the order
  given, then the script. `Wait` works in both (the console runner owns it).
- `--check` needs no `--output` and never starts threads; it is the CI entry
  point and uses `Show::load`'s validation plus `Patch Check` (item 11).
- `--json` status is a stable, documented shape (`serde_json` already in the
  tree through the show format): universes, rates, states, counters, desk
  markers, journal path.
- `--log` is line-oriented (`at result line`), appended and flushed, so a
  support case is one file.

### Macros (desk, additive)

```
Macro <n> "<name>" <statements…>     store/replace (statements separated by ;)
Macro <n> Add <statement>            append one
Macro <n> Delete                     remove
Macro [<n>]                          list
Macro <n> Run                        expand and execute
```

- A macro is stored as **statement text**, so it is inspected, edited and
  replayed exactly like typed input. Desk state: it is desk configuration, not
  programming, and belongs next to the handles.
- Expansion happens in `Engine::execute` before dispatch: a line that parses to
  `Macro <n> Run` is replaced by the macro's statements, each executed through
  the same loop. Depth is capped at 8 and a macro already on the expansion stack
  is refused with `macro 3 calls itself`, so recursion is impossible rather than
  merely unlikely.
- `Wait` inside a macro is rejected (`Wait is a script command; run the macro
  from a script`). The engine has no sleep, and giving it one would block the
  tick and the output — the right place for time is the runner.
- One `Run` is **one undo group**, so a macro that patches ten fixtures is one
  undo step.

### Undo grouping

```
Begin [<label>]     open a group
End                 close it
```

- `Engine` gains `group: Option<GroupState>`; `mutate` and the programmer
  writers record into the group instead of pushing a step, and `End` pushes one
  `Snapshot` taken at `Begin` with the label (default: the first statement in the
  group).
- A group closes on `End`, or on `Load`/`New` (which clear history outright).
  An unclosed group stays open across statements and across `execute` calls
  until `End`, so `Begin; a; b; End` works as one line, which is the main use;
  `Status` says `undo group open: 4 edits` while it is open.
- Nesting is refused (`End` without `Begin` and `Begin` while open are errors
  naming the open group), because a stack of groups is a feature nobody needs
  and every bug would be in it.
- `Patch Import`, `Macro … Run`, `Import Mvr`, `Import Type` open a group
  internally, so the common bulk operations are atomic without the user typing
  `Begin`.

### Completion

`--completions bash` prints a script that completes the command keywords, the
attribute names of the current selection, and the pool numbers — generated from
`HELP` plus a static keyword list, so it cannot drift far. It is a convenience,
not a parser: it completes words, never validates a line.

## Implementation plan

1. Add the flags, `--check`, `--status --json`, `--log` and `--quiet`; keep
   `Options::parse` a pure function with a test per flag and error.
2. `Command::{Begin, End}` + `GroupState` in `Engine`; `History::record` learns
   to append into an open group; tests for the group lifecycle.
3. `Macro` storage in `Desk`, parse/dispatch/expansion with depth and cycle
   guards; `Macro … Run` wrapped in a group.
4. Wrap the bulk imports in groups.
5. `--completions` generator and its smoke test (output parses as a shell
   script for `bash -n`).

## Tests

- **console** — every flag parses; `--version` contains both versions; `--check`
  on a valid show exits 0 and on a broken one exits 1 with the problem on
  stdout; `--script -` runs stdin; `--command` order is preserved before the
  script; `--json` is valid JSON with the documented keys.
- **engine** — `Begin; At 50; Store Cue 1; End` is one undo step labelled with
  the first statement; `Undo` inside an open group is refused with a hint;
  `End` without `Begin` errors; an unclosed group is reported in `Status`.
- **engine** — `Macro 1 "x" A; B` stores two statements; `Macro 1 Run` executes
  both; a self-calling macro is refused at depth 1 (`macro 1 calls itself`); a
  `Wait` in a macro is refused with the hint; `Macro 1 Run` is one undo step.
- **desk** — macros round-trip; a macro referencing a deleted pool still lists
  and fails at run time with the command's own error.
- **journal** — a group is journaled per statement (replay must reproduce the
  edits, not the grouping), and recovery restores the grouped result.

## Boundaries

- No variables, conditionals or loops in macros (that is a scripting language;
  `--script` plus a shell already exists).
- No key binding or MIDI/OSC macro triggers (item 12 maps addresses to
  commands; a macro is one of those commands).
- No history browser UI, no named undo checkpoints across sessions.

## Risks

- **Undo-group misuse.** A forgotten `End` makes every later edit part of the
  same step. `Status` reports the open group and the shell shows it in the
  prompt, so it is visible, and `Begin` while open is refused.
- **`--check` as a CI gate needs stable output.** The format is
  `path: problem` per line and nothing else; changing it is a breaking change
  that the docs list.
- **Completion drift.** Generated from `HELP` and a keyword table that lives
  next to the parser; a test asserts every command keyword appears in both.
