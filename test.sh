#!/bin/bash
set -e

# Build cg_clif
export RUSTFLAGS="-Zrun_dsymutil=no"
if [[ "$1" == "--release" ]]; then
    export CHANNEL='release'
    cargo build --release
else
    export CHANNEL='debug'
    cargo build --bin cg_clif
fi

# Config
source scripts/config.sh
export CG_CLIF_INCR_CACHE_DISABLED=1
MY_RUSTC=$RUSTC" "$RUSTFLAGS" -L crate=target/out --out-dir target/out -Cdebuginfo=2"

# Cleanup
rm -r target/out || true

# Perform all tests
echo "[BUILD] mini_core"
$MY_RUSTC example/mini_core.rs --crate-name mini_core --crate-type lib,dylib --target $TARGET_TRIPLE

echo "[BUILD] example"
$MY_RUSTC example/example.rs --crate-type lib --target $TARGET_TRIPLE

if [[ "$JIT_SUPPORTED" = "1" ]]; then
    echo "[JIT] mini_core_hello_world"
    CG_CLIF_JIT_ARGS="abc bcd" $MY_RUSTC --jit example/mini_core_hello_world.rs --cfg jit --target $HOST_TRIPLE
else
    echo "[JIT] mini_core_hello_world (skipped)"
fi

echo "[AOT] mini_core_hello_world"
$MY_RUSTC example/mini_core_hello_world.rs --crate-name mini_core_hello_world --crate-type bin -g --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/mini_core_hello_world abc bcd
# (echo "break set -n main"; echo "run"; sleep 1; echo "si -c 10"; sleep 1; echo "frame variable") | lldb -- ./target/out/mini_core_hello_world abc bcd

echo "[AOT] arbitrary_self_types_pointers_and_wrappers"
$MY_RUSTC example/arbitrary_self_types_pointers_and_wrappers.rs --crate-name arbitrary_self_types_pointers_and_wrappers --crate-type bin --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/arbitrary_self_types_pointers_and_wrappers

echo "[BUILD] sysroot"
time ./build_sysroot/build_sysroot.sh --release

echo "[AOT] alloc_example"
$MY_RUSTC example/alloc_example.rs --crate-type bin --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/alloc_example

if [[ "$JIT_SUPPORTED" = "1" ]]; then
    echo "[JIT] std_example"
    $MY_RUSTC --jit example/std_example.rs --target $HOST_TRIPLE -Z force-overflow-checks=off
else
    echo "[JIT] std_example (skipped)"
fi

echo "[AOT] dst_field_align"
# FIXME Re-add -Zmir-opt-level=2 once rust-lang/rust#67529 is fixed.
$MY_RUSTC example/dst-field-align.rs --crate-name dst_field_align --crate-type bin --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/dst_field_align || (echo $?; false)

echo "[AOT] std_example"
$MY_RUSTC example/std_example.rs --crate-type bin --target $TARGET_TRIPLE -Z force-overflow-checks=off
$RUN_WRAPPER ./target/out/std_example arg

echo "[AOT] subslice-patterns-const-eval"
$MY_RUSTC example/subslice-patterns-const-eval.rs --crate-type bin -Cpanic=abort --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/subslice-patterns-const-eval

echo "[AOT] track-caller-attribute"
$MY_RUSTC example/track-caller-attribute.rs --crate-type bin -Cpanic=abort --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/track-caller-attribute

echo "[AOT] mod_bench"
$MY_RUSTC example/mod_bench.rs --crate-type bin --target $TARGET_TRIPLE
$RUN_WRAPPER ./target/out/mod_bench

git clone https://github.com/rust-lang/rust.git --single-branch || true
cd rust
git fetch
git checkout -f $(rustc -V | cut -d' ' -f3 | tr -d '(') src/test
export RUSTFLAGS=
export CG_CLIF_DISPLAY_CG_TIME=

rm config.toml || true

cat > config.toml <<EOF
changelog-seen = 2
[rust]
codegen-backends = []
deny-warnings = false
[build]
local-rebuild = true
rustc = "${RUSTC}"
EOF

cargo install ripgrep

git checkout $(rustc -V | cut -d' ' -f3 | tr -d '(') src/test
rm -r src/test/ui/{abi/,extern/,panics/,unsized-locals/,thinlto/,simd*,*lto*.rs,linkage*,unwind-*.rs,duplicate/} || true
for test in $(rg --files-with-matches "asm!|catch_unwind|should_panic|lto" src/test/ui); do
  rm $test
done

for test in $(rg --files-with-matches "//~.*ERROR|//~.*NOTE|// error-pattern:|// build-fail" src/test/ui); do
  rm $test
done

git checkout -- src/test/ui/issues/auxiliary/issue-3136-a.rs # contains //~ERROR, but shouldn't be removed

# these all depend on unwinding support
rm src/test/ui/backtrace.rs
rm src/test/ui/intrinsics/intrinsic-move-val-cleanups.rs
rm src/test/ui/array-slice-vec/box-of-array-of-drop-*.rs
rm src/test/ui/array-slice-vec/slice-panic-*.rs
rm src/test/ui/array-slice-vec/nested-vec-3.rs
rm src/test/ui/cleanup-rvalue-temp-during-incomplete-alloc.rs
rm src/test/ui/issues/issue-26655.rs
rm src/test/ui/issues/issue-29485.rs
rm src/test/ui/issues/issue-30018-panic.rs
rm src/test/ui/multi-panic.rs
rm src/test/ui/sepcomp/sepcomp-unwind.rs
rm src/test/ui/structs-enums/unit-like-struct-drop-run.rs
rm src/test/ui/terminate-in-initializer.rs
rm src/test/ui/threads-sendsync/task-stderr.rs
rm src/test/ui/numbers-arithmetic/int-abs-overflow.rs
rm src/test/ui/drop/drop-trait-enum.rs
rm src/test/ui/issues/issue-8460.rs

# these all use ByScalarPair type as extern "C" function parameter => warning
rm src/test/ui/rust-2018/proc-macro-crate-in-paths.rs
rm src/test/ui/proc-macro/crt-static.rs
rm src/test/ui/proc-macro/no-missing-docs.rs
rm src/test/ui/mir/mir_codegen_calls.rs

rm src/test/ui/issues/issue-28950.rs # depends on stack size optimizations
rm src/test/ui/init-large-type.rs # same
rm src/test/ui/sse2.rs # cpuid not supported, so sse2 not detected
rm src/test/ui/issues/issue-33992.rs # unsupported linkages
rm src/test/ui/issues/issue-51947.rs # same
rm src/test/ui/impl-trait/impl-generic-mismatch.rs # same
rm src/test/ui/issues/issue-21160.rs # same
rm src/test/ui/numbers-arithmetic/saturating-float-casts.rs # intrinsic gives different but valid result
rm src/test/ui/mir/mir_misc_casts.rs # depends on deduplication of constants
rm src/test/ui/mir/mir_raw_fat_ptr.rs # same
rm src/test/ui/consts/const-str-ptr.rs # same
rm src/test/ui/async-await/async-fn-size-moved-locals.rs # -Cpanic=abort shrinks some generator by one byte
rm src/test/ui/async-await/async-fn-size-uninit-locals.rs # same
rm src/test/ui/generator/size-moved-locals.rs # same
rm src/test/ui/fn/dyn-fn-alignment.rs # wants a 256 byte alignment
rm src/test/ui/consts/const_in_pattern/issue-73431.rs # gives warning for MY_RUSTC_LOG=warn
rm src/test/ui/test-attrs/test-fn-signature-verification-for-explicit-return-type.rs # "Cannot run dynamic test fn out-of-process"
rm src/test/ui/intrinsics/intrinsic-nearby.rs # unimplemented nearbyintf32 and nearbyintf64 intrinsics

rm src/test/incremental/hashes/inline_asm.rs # inline asm
rm src/test/incremental/issue-72386.rs # same
rm src/test/incremental/change_crate_dep_kind.rs # requires -Cpanic=unwind
rm src/test/incremental/issue-49482.rs # same
rm src/test/incremental/issue-54059.rs # same
rm src/test/incremental/hashes/statics.rs # unsupported linkages
rm src/test/incremental/hashes/function_interfaces.rs # same
rm src/test/incremental/lto.rs # requires lto

rm src/test/pretty/asm.rs # inline asm
rm src/test/pretty/raw-str-nonexpr.rs # same

rm -r src/test/run-pass-valgrind/unsized-locals

echo "[TEST] rustc test suite"
COMPILETEST_FORCE_STAGE0=1 ./x.py test --stage 0 src/test/{codegen-units,run-make,run-pass-valgrind,ui} 2>&1 | tee log.txt
