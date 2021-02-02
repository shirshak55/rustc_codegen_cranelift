#!/bin/bash

# Requires the CHANNEL env var to be set to `debug` or `release.`

set -e

source ./config.sh

dir=$(pwd)

unamestr=$(uname)
if [[ "$unamestr" == 'Linux' ]]; then
   dylib_ext='so'
elif [[ "$unamestr" == 'Darwin' ]]; then
   dylib_ext='dylib'
else
   echo "Unsupported os"
   exit 1
fi

cd "$(dirname "$0")"

# Cleanup for previous run
#     v Clean target dir except for build scripts and incremental cache
rm -r target/*/{debug,release}/{build,deps,examples,libsysroot*,native} 2>/dev/null || true

# We expect the target dir in the default location. Guard against the user changing it.
export CARGO_TARGET_DIR=target

# Build libs
export RUSTC="$dir/bin/rustc"
export RUSTFLAGS="-Zforce-unstable-if-unmarked -Cpanic=abort --sysroot $dir -Zcodegen-backend=cranelift"
export __CARGO_DEFAULT_LIB_METADATA="cg_clif"
if [[ "$1" != "--debug" ]]; then
    sysroot_channel='release'
    # FIXME Enable incremental again once rust-lang/rust#74946 is fixed
    CARGO_INCREMENTAL=0 RUSTFLAGS="$RUSTFLAGS -Zmir-opt-level=2" cargo build --target "$TARGET_TRIPLE" --release
else
    sysroot_channel='debug'
    cargo build --target "$TARGET_TRIPLE"
fi

# Copy files to sysroot
ln "target/$TARGET_TRIPLE/$sysroot_channel/deps/"* "$dir/lib/rustlib/$TARGET_TRIPLE/lib/"
rm "$dir/lib/rustlib/$TARGET_TRIPLE/lib/"*.{rmeta,d}
