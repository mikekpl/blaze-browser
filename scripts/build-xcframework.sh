#!/usr/bin/env bash
# Build the Rust core as an XCFramework + Swift bindings for the macOS shell (T017).
# Output: platforms/macos/Generated/{BlazeFFI.xcframework, blaze_ffi.swift, module map}
set -euo pipefail
cd "$(dirname "$0")/.."

CONFIG=${1:-debug}
OUT=platforms/macos/Generated
HEADERS="$OUT/headers"
PROFILE_FLAG=()
[[ "$CONFIG" == "release" ]] && PROFILE_FLAG=(--release)

TARGETS=(aarch64-apple-darwin)
# Universal binary for release/distribution builds.
[[ "$CONFIG" == "release" ]] && TARGETS+=(x86_64-apple-darwin)

for t in "${TARGETS[@]}"; do
  rustup target add "$t" >/dev/null 2>&1 || true
  cargo build -p blaze-ffi ${PROFILE_FLAG[@]+"${PROFILE_FLAG[@]}"} --target "$t"
done

rm -rf "$OUT"
mkdir -p "$HEADERS"

# Generate Swift bindings + C headers from the built library.
FIRST_LIB="target/${TARGETS[0]}/$CONFIG/libblaze_ffi.dylib"
cargo run -p blaze-ffi --bin uniffi-bindgen ${PROFILE_FLAG[@]+"${PROFILE_FLAG[@]}"} -- \
  generate --library "$FIRST_LIB" --language swift --out-dir "$OUT"

# Modulemap must be named module.modulemap inside the framework headers.
mv "$OUT"/*.h "$HEADERS"/
mv "$OUT"/*.modulemap "$HEADERS/module.modulemap"

if [[ ${#TARGETS[@]} -gt 1 ]]; then
  mkdir -p "target/universal/$CONFIG"
  lipo -create $(for t in "${TARGETS[@]}"; do echo "target/$t/$CONFIG/libblaze_ffi.a"; done) \
    -output "target/universal/$CONFIG/libblaze_ffi.a"
  STATIC_LIB="target/universal/$CONFIG/libblaze_ffi.a"
else
  STATIC_LIB="target/${TARGETS[0]}/$CONFIG/libblaze_ffi.a"
fi

xcodebuild -create-xcframework \
  -library "$STATIC_LIB" -headers "$HEADERS" \
  -output "$OUT/BlazeFFI.xcframework"

echo "XCFramework + bindings ready in $OUT"
