#!/bin/sh
# Meson <-> cargo bridge: build the crate into the Meson build tree and copy the
# resulting binary to the path Meson expects as the custom_target output.
#
#   $1  meson build root
#   $2  meson source root
#   $3  output path Meson wants the binary at (@OUTPUT@)
#   $4  rust profile directory: "debug" or "release"
#   $5  binary name produced by cargo
#   $@  remaining args are passed straight through to `cargo build`
set -eu

BUILD_ROOT="$1"
SOURCE_ROOT="$2"
OUTPUT="$3"
PROFILE="$4"
BIN_NAME="$5"
shift 5

CARGO_TARGET_DIR="$BUILD_ROOT/target"
export CARGO_TARGET_DIR
: "${CARGO_HOME:="$BUILD_ROOT/cargo-home"}"
export CARGO_HOME

cd "$SOURCE_ROOT"
cargo build "$@"
cp -f "$CARGO_TARGET_DIR/$PROFILE/$BIN_NAME" "$OUTPUT"
