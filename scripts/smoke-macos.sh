#!/usr/bin/env bash
#
# Host (macOS) smoke test for the pergamon UniFFI facade.
#
# Builds the macOS library, generates Swift bindings, then compiles and runs
# apps/ios/HostSmoke/main.swift against them. This is the fast inner-loop check
# that the FFI contract works, independent of Xcode / the iOS Simulator.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT

echo "==> Building pergamon-uniffi (release, host)"
cargo build -p pergamon-uniffi --release

DYLIB="target/release/libpergamon_uniffi.dylib"

echo "==> Generating Swift bindings"
cargo run -q --bin uniffi-bindgen -- generate \
  --library "$DYLIB" --language swift --out-dir "$GEN"

echo "==> Compiling Swift host smoke test"
swiftc -O \
  -I "$GEN" \
  -L target/release -lpergamon_uniffi \
  -Xcc -fmodule-map-file="$GEN/pergamon_uniffiFFI.modulemap" \
  "$GEN/pergamon_uniffi.swift" apps/ios/HostSmoke/main.swift \
  -o "$GEN/smoke"

echo "==> Running"
echo "------------------------------------------------------------"
DYLD_LIBRARY_PATH=target/release "$GEN/smoke"
echo "------------------------------------------------------------"
echo "==> OK"
