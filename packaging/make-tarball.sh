#!/usr/bin/env bash
# Build the binary and a release tarball: binary, README, docs, examples and the
# udev rules. The binary is still the same `openlight` the console crate builds;
# this only packages it.
#
#   packaging/make-tarball.sh                 # release build, host target
#   PROFILE=debug packaging/make-tarball.sh   # reuse a debug build
#   TARGET=aarch64-unknown-linux-gnu packaging/make-tarball.sh
set -euo pipefail

cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
target="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
profile="${PROFILE:-release}"
name="openlight-${version}-${target}"
out="dist/${name}"

cargo build -p openlight-console --profile "$profile" --target "$target"

rm -rf "$out"
mkdir -p "$out"
cp "target/${target}/${profile}/openlight" "$out/"
chmod 0755 "$out/openlight"
cp README.md CHANGELOG.md SECURITY.md CONTRIBUTING.md LICENSE "$out/"
mkdir -p "$out/docs" "$out/examples" "$out/packaging/udev"
cp -r docs/. "$out/docs/"
cp -r examples/. "$out/examples/"
cp packaging/udev/60-openlight.rules "$out/packaging/udev/"

tar -czf "dist/${name}.tar.gz" -C dist "$name"
echo "dist/${name}.tar.gz"
