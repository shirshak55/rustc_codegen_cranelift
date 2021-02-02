#!/bin/bash
set -e

# Settings
export CHANNEL="release"
build_sysroot="clif"
target_dir='build'
oldbe=''
while [[ $# != 0 ]]; do
    case $1 in
        "--debug")
            export CHANNEL="debug"
            ;;
        "--sysroot")
            build_sysroot=$2
            shift
            ;;
        "--target-dir")
            target_dir=$2
            shift
            ;;
        "--oldbe")
            oldbe='--features oldbe'
            ;;
        *)
            echo "Unknown flag '$1'"
            echo "Usage: ./build.sh [--debug] [--sysroot none|clif|llvm] [--target-dir DIR] [--oldbe]"
            exit 1
            ;;
    esac
    shift
done

# Build cg_clif
unset CARGO_TARGET_DIR
unamestr=$(uname)
HOST_TRIPLE=$(rustc -vV | grep host | cut -d: -f2 | tr -d " ")
if [[ "$unamestr" == 'Linux' ]]; then
   export RUSTFLAGS="-Clink-arg=-Wl,-rpath=\$ORIGIN/../lib/rustlib/$HOST_TRIPLE/codegen-backends "$RUSTFLAGS
elif [[ "$unamestr" == 'Darwin' ]]; then
   export RUSTFLAGS="-Csplit-debuginfo=unpacked -Clink-arg=-Wl,-rpath,@loader_path/../lib/rustlib/$HOST_TRIPLE/codegen-backends -Zosx-rpath-install-name "$RUSTFLAGS
   dylib_ext='dylib'
else
   echo "Unsupported os"
   exit 1
fi
unset HOST_TRIPLE
if [[ "$CHANNEL" == "release" ]]; then
    cargo build $oldbe --release
else
    cargo build $oldbe
fi

source scripts/ext_config.sh

rm -rf "$target_dir"
mkdir "$target_dir"
mkdir "$target_dir"/bin "$target_dir"/lib
mkdir -p "$target_dir/lib/rustlib/$HOST_TRIPLE/codegen-backends"
mkdir -p "$target_dir/lib/rustlib/$TARGET_TRIPLE/lib/"
ln "$(rustc --print sysroot)/bin/rustc" "$target_dir/bin"
ln target/$CHANNEL/cg_clif{,_build_sysroot} "$target_dir"/bin
ln target/$CHANNEL/librustc_codegen_cranelift.so "$target_dir/lib/rustlib/$HOST_TRIPLE/codegen-backends/librustc_codegen_cranelift.so"
ln target/$CHANNEL/librustc_codegen_cranelift.so "$target_dir/lib/rustlib/$HOST_TRIPLE/codegen-backends/librustc_codegen_cranelift-$(rustc -vV | grep release | cut -d: -f2 |tr -d " ").so"
ln rust-toolchain scripts/config.sh scripts/cargo.sh "$target_dir"

if [[ "$TARGET_TRIPLE" == "x86_64-pc-windows-gnu" ]]; then
    cp $(rustc --print sysroot)/lib/rustlib/$TARGET_TRIPLE/lib/*.o "$target_dir/lib/rustlib/$TARGET_TRIPLE/lib/"
fi

case "$build_sysroot" in
    "none")
        ;;
    "llvm")
        cp -r $(rustc --print sysroot)/lib/rustlib/$TARGET_TRIPLE/lib "$target_dir/lib/rustlib/$TARGET_TRIPLE/"
        ;;
    "clif")
        echo "[BUILD] sysroot"
        dir=$(pwd)
        cd "$target_dir"
        time "$dir/build_sysroot/build_sysroot.sh"
        cp lib/rustlib/*/lib/libstd-* lib/
        ;;
    *)
        echo "Unknown sysroot kind \`$build_sysroot\`."
        echo "The allowed values are:"
        echo "    none A sysroot that doesn't contain the standard library"
        echo "    llvm Copy the sysroot from rustc compiled by cg_llvm"
        echo "    clif Build a new sysroot using cg_clif"
        exit 1
esac
