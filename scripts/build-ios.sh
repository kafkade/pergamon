#!/usr/bin/env bash
#
# Build the pergamon-uniffi Rust facade for iOS and package it as an
# XCFramework that the SwiftUI sample app (apps/ios) consumes.
#
# Steps:
#   1. ensure the iOS device + simulator Rust targets are installed,
#   2. build release static libraries for each,
#   3. generate the Swift bindings + C module headers with uniffi-bindgen,
#   4. assemble PergamonFFI.xcframework (device + simulator slices),
#   5. drop the generated Swift into the app's Generated/ folder.
#
# Outputs (both git-ignored, regenerated on demand):
#   apps/ios/Frameworks/PergamonFFI.xcframework
#   apps/ios/PergamonSpike/Generated/pergamon_uniffi.swift
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CRATE="pergamon-uniffi"
LIB="libpergamon_uniffi.a"
IOS_TARGET="aarch64-apple-ios"
SIM_TARGET="aarch64-apple-ios-sim"
APP_DIR="apps/ios"
GEN_DIR="$APP_DIR/PergamonSpike/Generated"
XCF="$APP_DIR/Frameworks/PergamonFFI.xcframework"

# Keep in sync with IPHONEOS_DEPLOYMENT_TARGET in apps/ios/project.yml.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

echo "==> Ensuring Rust iOS targets are installed"
rustup target add "$IOS_TARGET" "$SIM_TARGET"

echo "==> Building static libraries (release)"
cargo build -p "$CRATE" --release --target "$IOS_TARGET"
cargo build -p "$CRATE" --release --target "$SIM_TARGET"

echo "==> Generating Swift bindings"
BIND="$(mktemp -d)"
HEADERS="$(mktemp -d)"
trap 'rm -rf "$BIND" "$HEADERS"' EXIT
cargo run -q --bin uniffi-bindgen -- generate \
  --library "target/$IOS_TARGET/release/$LIB" \
  --language swift --out-dir "$BIND"

echo "==> Staging generated Swift + Clang module headers"
mkdir -p "$GEN_DIR"
rm -f "$GEN_DIR"/*.swift
cp "$BIND/pergamon_uniffi.swift" "$GEN_DIR/"

cp "$BIND/pergamon_uniffiFFI.h" "$HEADERS/"
# An XCFramework's Clang module must be named module.modulemap.
cp "$BIND/pergamon_uniffiFFI.modulemap" "$HEADERS/module.modulemap"

echo "==> Assembling XCFramework"
mkdir -p "$APP_DIR/Frameworks"
rm -rf "$XCF"
xcodebuild -create-xcframework \
  -library "target/$IOS_TARGET/release/$LIB" -headers "$HEADERS" \
  -library "target/$SIM_TARGET/release/$LIB" -headers "$HEADERS" \
  -output "$XCF"

echo "==> Done"
echo "  XCFramework: $XCF"
echo "  Bindings:    $GEN_DIR/pergamon_uniffi.swift"
du -sh "$XCF" 2>/dev/null || true
