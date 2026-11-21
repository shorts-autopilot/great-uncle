#!/usr/bin/env bash
# Run the phase demos and compare the last DMX frame to a recorded trace.
#
#   tools/demo_trace.sh check    # fail on any byte change (CI and pre-commit)
#   tools/demo_trace.sh record   # rewrite examples/expected/*.trace
#
# The demos write their shows under target/, so a run never touches the
# examples. Output is quiet apart from the trace itself and a diff.
set -euo pipefail

cd "$(dirname "$0")/.."
mode="${1:-check}"
case "$mode" in
    check|record) ;;
    *)
        echo "usage: $0 [check|record]" >&2
        exit 2
        ;;
esac

mkdir -p target examples/expected
for demo in phase1:30 phase2:120 phase3:120 phase4:120; do
    name="${demo%%:*}"
    frames="${demo##*:}"
    output="$(cargo run -q -p openlight-console -- \
        --no-remote \
        --output 1=virtual \
        --script "examples/$name-demo.txt" \
        --frames "$frames" --trace 2>&1)"
    # The final trace line has the frame count (`Universe 1: 30 frames…`); the
    # startup line (`Universe 1 -> virtual`) carries live timings and must not
    # enter the expected file.
    # The frame count depends on how long startup took; the bytes do not. Drop
    # the count so the expected file is about the rendered look only.
    printf '%s\n' "$output" \
        | sed -n '/^Universe [0-9][0-9]*:/,$p' \
        | sed -E 's/^(Universe [0-9]+): [0-9]+ frames, /\1: /' \
        > "target/$name.trace"
    if ! grep -q '^Universe [0-9][0-9]*:' "target/$name.trace"; then
        echo "$name: the demo printed no trace" >&2
        printf '%s\n' "$output" | tail -20 >&2
        exit 1
    fi
    if [ "$mode" = record ]; then
        cp "target/$name.trace" "examples/expected/$name.trace"
        echo "recorded examples/expected/$name.trace"
    else
        diff -u "examples/expected/$name.trace" "target/$name.trace"
        echo "$name: trace matches"
    fi
done
