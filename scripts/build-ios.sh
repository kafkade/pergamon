#!/usr/bin/env bash
#
# Build the pergamon-uniffi Rust facade and package it as an XCFramework that the
# PergamonKit Swift package (apps/ios/PergamonKit) — and, through it, the SwiftUI
# app (apps/ios) — consumes with no hand-written FFI glue.
#
# Steps:
#   1. ensure the iOS device + simulator + macOS host Rust targets are installed,
#   2. build release static libraries for each,
#   3. generate the Swift bindings + C module headers with uniffi-bindgen,
#   4. assemble PergamonFFI.xcframework (device + simulator + macOS slices),
#   5. drop the generated Swift into the package's bindings target.
#
# The macOS slice lets `swift test` run PergamonKit natively on the host (a fast
# inner loop, no Simulator); the iOS/simulator slices are what the app links.
#
# Outputs (all git-ignored, regenerated on demand):
#   apps/ios/PergamonKit/Frameworks/PergamonFFI.xcframework
#   apps/ios/PergamonKit/Sources/PergamonBindings/pergamon_uniffi.swift
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CRATE="pergamon-uniffi"
LIB="libpergamon_uniffi.a"
IOS_TARGET="aarch64-apple-ios"
SIM_TARGET="aarch64-apple-ios-sim"
MAC_TARGET="aarch64-apple-darwin"
APP_DIR="apps/ios"
PKG_DIR="$APP_DIR/PergamonKit"
GEN_DIR="$PKG_DIR/Sources/PergamonBindings"
XCF="$PKG_DIR/Frameworks/PergamonFFI.xcframework"

# Keep in sync with IPHONEOS_DEPLOYMENT_TARGET in apps/ios/project.yml.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

echo "==> Ensuring Rust targets are installed"
rustup target add "$IOS_TARGET" "$SIM_TARGET" "$MAC_TARGET"

echo "==> Building static libraries (release)"
cargo build -p "$CRATE" --release --target "$IOS_TARGET"
cargo build -p "$CRATE" --release --target "$SIM_TARGET"
cargo build -p "$CRATE" --release --target "$MAC_TARGET"

echo "==> Generating Swift bindings"
BIND="$(mktemp -d)"
HEADERS="$(mktemp -d)"
trap 'rm -rf "$BIND" "$HEADERS"' EXIT
cargo run -q --bin uniffi-bindgen -- generate \
  --library "target/$IOS_TARGET/release/$LIB" \
  --language swift --out-dir "$BIND"

echo "==> Staging generated Swift into the PergamonBindings target"
mkdir -p "$GEN_DIR"
rm -f "$GEN_DIR"/*.swift
cp "$BIND/pergamon_uniffi.swift" "$GEN_DIR/"

cp "$BIND/pergamon_uniffiFFI.h" "$HEADERS/"
# An XCFramework's Clang module must be named module.modulemap.
cp "$BIND/pergamon_uniffiFFI.modulemap" "$HEADERS/module.modulemap"

echo "==> Assembling XCFramework (iOS device + simulator + macOS host)"
mkdir -p "$PKG_DIR/Frameworks"
rm -rf "$XCF"
xcodebuild -create-xcframework \
  -library "target/$IOS_TARGET/release/$LIB" -headers "$HEADERS" \
  -library "target/$SIM_TARGET/release/$LIB" -headers "$HEADERS" \
  -library "target/$MAC_TARGET/release/$LIB" -headers "$HEADERS" \
  -output "$XCF"

echo "==> Done"
echo "  XCFramework: $XCF"
echo "  Bindings:    $GEN_DIR/pergamon_uniffi.swift"
du -sh "$XCF" 2>/dev/null || true
