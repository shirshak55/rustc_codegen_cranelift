// Adapted from https://github.com/sunfishcode/mir2cranelift/blob/master/rust-examples/nocore-hello-world.rs

#![feature(
    no_core, unboxed_closures, start, lang_items, box_syntax, never_type, linkage,
    extern_types, thread_local, register_attr,
)]
#![no_core]
#![allow(dead_code, non_camel_case_types)]
#![register_attr(do_not_trace)]

extern crate mini_core;

use mini_core::*;
use mini_core::libc::*;

unsafe extern "C" fn my_puts(s: *const i8) {
    puts(s);
}

#[lang = "termination"]
trait Termination {
    fn report(self) -> i32;
}

impl Termination for () {
    fn report(self) -> i32 {
        unsafe {
            NUM = 6 * 7 + 1 + (1u8 == 1u8) as u8; // 44
            *NUM_REF as i32
        }
    }
}

trait SomeTrait {
    fn object_safe(&self);
}

impl SomeTrait for &'static str {
    fn object_safe(&self) {
        unsafe {
            puts(*self as *const str as *const i8);
        }
    }
}

struct NoisyDrop {
    text: &'static str,
    inner: NoisyDropInner,
}

struct NoisyDropInner;

impl Drop for NoisyDrop {
    fn drop(&mut self) {
        unsafe {
            puts(self.text as *const str as *const i8);
        }
    }
}

impl Drop for NoisyDropInner {
    fn drop(&mut self) {
        unsafe {
            puts("Inner got dropped!\0" as *const str as *const i8);
        }
    }
}

impl SomeTrait for NoisyDrop {
    fn object_safe(&self) {}
}

enum Ordering {
    Less = -1,
    Equal = 0,
    Greater = 1,
}

#[lang = "start"]
fn start<T: Termination + 'static>(
    main: fn() -> T,
    argc: isize,
    argv: *const *const u8,
) -> isize {
    unsafe { yk_swt_start_tracing(); }

    if argc == 3 {
        //unsafe { puts(*argv as *const i8); }
        //unsafe { puts(*((argv as usize + intrinsics::size_of::<*const u8>()) as *const *const i8)); }
        //unsafe { puts(*((argv as usize + 2 * intrinsics::size_of::<*const u8>()) as *const *const i8)); }
    }

    main().report();

    unsafe {
        let mut ret_trace_len = 0;
        let mut buf = yk_swt_stop_tracing(&mut ret_trace_len);
        if buf as usize == 0 {
            intrinsics::abort();
        }
        libc::printf(
            "ret: %p %p\n\0" as *const str as *const i8,
            buf,
            ret_trace_len,
        );
        loop {
            if ret_trace_len == 0 {
                break;
            }
            let swt_loc = *buf;
            libc::printf(
                "trace: %x %d %d\n\0" as *const str as *const i8,
                swt_loc.crate_hash,
                swt_loc.def_idx,
                swt_loc.bb_idx,
            );
            buf = (buf as isize + intrinsics::size_of::<SwtLoc>() as isize) as *mut SwtLoc;
            ret_trace_len = ret_trace_len - 1;
        }
    }
    0
}

static mut NUM: u8 = 6 * 7;
static NUM_REF: &'static u8 = unsafe { &NUM };

macro_rules! assert {
    ($e:expr) => {
        if !$e {
            panic(stringify!(! $e));
        }
    };
}

macro_rules! assert_eq {
    ($l:expr, $r: expr) => {
        if $l != $r {
            panic(stringify!($l != $r));
        }
    }
}

struct Unique<T: ?Sized> {
    pointer: *const T,
    _marker: PhantomData<T>,
}

impl<T: ?Sized, U: ?Sized> CoerceUnsized<Unique<U>> for Unique<T> where T: Unsize<U> {}

unsafe fn zeroed<T>() -> T {
    let mut uninit = MaybeUninit { uninit: () };
    intrinsics::write_bytes(&mut uninit.value.value as *mut T, 0, 1);
    uninit.value.value
}

fn take_f32(_f: f32) {}
fn take_unique(_u: Unique<()>) {}

fn return_u128_pair() -> (u128, u128) {
    (0, 0)
}

fn call_return_u128_pair() {
    return_u128_pair();
}


// Rust translation of the C code removed in https://github.com/softdevteam/ykrustc/pull/121
#[repr(C)]
struct SwtLoc {
    crate_hash: u64,
    def_idx: u32,
    bb_idx: u32,
}

unsafe impl Copy for SwtLoc {}

const TL_TRACE_INIT_CAP: isize = 1024;
const TL_TRACE_REALLOC_CAP: isize = 1024;

/// The trace buffer.
#[thread_local]
static mut TRACE_BUF: *mut SwtLoc = 0 as *mut SwtLoc;

/// The number of elements in the trace buffer.
#[thread_local]
static mut TRACE_BUF_LEN: isize = 0;

/// The allocation capacity of the trace buffer (in elements).
#[thread_local]
static mut TRACE_BUF_CAP: isize = 0;

/// true = we are tracing, false = we are not tracing or an error occurred.
#[thread_local]
static mut TRACING: bool = false;

/// Start tracing on the current thread.
/// A new trace buffer is allocated and MIR locations will be written into it on
/// subsequent calls to `yk_swt_rec_loc`. If the current thread is already
/// tracing, calling this will lead to undefined behaviour.
#[do_not_trace]
unsafe fn yk_swt_start_tracing() {
    TRACE_BUF = calloc(TL_TRACE_INIT_CAP, intrinsics::size_of::<SwtLoc>() as isize) as *mut SwtLoc;
    if TRACE_BUF as usize == 0 {
        intrinsics::abort();
    }

    TRACE_BUF_CAP = TL_TRACE_INIT_CAP;
    TRACING = true;
}

/// Record a location into the trace buffer if tracing is enabled on the current thread.
#[do_not_trace]
#[no_mangle]
unsafe extern "C" fn __yk_swt_rec_loc(crate_hash: u64, def_idx: u32, bb_idx: u32) {
    if !TRACING {
        return;
    }
    //libc::printf("trace: %x %d %d\n\0" as *const str as *const i8, crate_hash, def_idx, bb_idx);

    // Check if we need more space and reallocate if necessary.
    if TRACE_BUF_LEN == TRACE_BUF_CAP {
        if TRACE_BUF_CAP >= isize::MAX_VALUE - TL_TRACE_REALLOC_CAP {
            // Trace capacity would overflow.
            TRACING = false;
            libc::puts("no trace" as *const str as *const i8);
            return;
        }
        let new_cap = TRACE_BUF_CAP + TL_TRACE_REALLOC_CAP;

        if new_cap > isize::MAX_VALUE / intrinsics::size_of::<SwtLoc>() as isize {
            // New buffer size would overflow.
            TRACING = false;
            libc::puts("no trace" as *const str as *const i8);
            return;
        }
        let new_size = new_cap * intrinsics::size_of::<SwtLoc>() as isize;

        TRACE_BUF = realloc(TRACE_BUF as *mut u8, new_size) as *mut SwtLoc;
        if TRACE_BUF as usize == 0 {
            TRACING = false;
            libc::puts("no trace" as *const str as *const i8);
            return;
        }

        TRACE_BUF_CAP = new_cap;
    }

    *((TRACE_BUF as isize + TRACE_BUF_LEN * intrinsics::size_of::<SwtLoc>() as isize) as *mut SwtLoc) = SwtLoc { crate_hash, def_idx, bb_idx };
    TRACE_BUF_LEN = TRACE_BUF_LEN + 1;
}

/// Stop tracing on the current thread.
/// On success the trace buffer is returned and the number of locations it
/// holds is written to `*ret_trace_len`. It is the responsibility of the caller
/// to free the returned trace buffer. A NULL pointer is returned on error.
/// Calling this function when tracing was not started with
/// `yk_swt_start_tracing_impl()` results in undefined behaviour.
#[do_not_trace]
unsafe fn yk_swt_stop_tracing(ret_trace_len: &mut isize) -> *mut SwtLoc {
    if !TRACING {
        libc::puts("no trace recorded\n\0" as *const str as *const i8);
        free(TRACE_BUF as *mut u8);
        TRACE_BUF = 0 as *mut SwtLoc;
        TRACE_BUF_LEN = 0;
        *ret_trace_len = 0;
        return 0 as *mut SwtLoc;
    }

    // We hand ownership of the trace to the caller. The caller is responsible
    // for freeing the trace.
    let ret_trace = TRACE_BUF;
    *ret_trace_len = TRACE_BUF_LEN;

    // Now reset all off the recorder's state.
    TRACE_BUF = 0 as *mut SwtLoc;
    TRACING = false;
    TRACE_BUF_LEN = 0;
    TRACE_BUF_CAP = 0;

    return ret_trace;
}

fn main() {
    //take_unique(Unique {
    //    pointer: 0 as *const (),
    //    _marker: PhantomData,
    //});
    take_f32(0.1);

    /*call_return_u128_pair();

    let slice = &[0, 1] as &[i32];
    let slice_ptr = slice as *const [i32] as *const i32;

    assert_eq!(slice_ptr as usize % 4, 0);

    //return;

    unsafe {
        printf("Hello %s\n\0" as *const str as *const i8, "printf\0" as *const str as *const i8);

        let hello: &[u8] = b"Hello\0" as &[u8; 6];
        let ptr: *const i8 = hello as *const [u8] as *const i8;
        puts(ptr);

        let world: Box<&str> = box "World!\0";
        puts(*world as *const str as *const i8);
        world as Box<dyn SomeTrait>;
    }*/
}
