//! Serialised Intermideiate Representation (SIR).
//!
//! SIR is built in-memory during LLVM code-generation, and finally placed into an ELF section at
//! link time.

#![allow(dead_code, unused_imports)]

use rustc_data_structures::fx::FxHashMap;
use rustc_data_structures::small_c_str::SmallCStr;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_index::{
    newtype_index,
    vec::{Idx, IndexVec},
};
use rustc_middle::ty::TyCtxt;
use rustc_session::config::OutputType;
use std::default::Default;
use std::ffi::CString;

const SIR_SECTION: &str = ".yk_sir";
const SIR_GLOBAL_SYM_PREFIX: &str = ".yksir";

// FIXME add disambiguator for codegen units to crate_hash, section name and symbol name

/// Writes the SIR into a buffer which will be linked in into an ELF section via LLVM.
/// This is based on write_compressed_metadata().
pub fn write_sir<'tcx>(
    tcx: TyCtxt<'tcx>,
    yk_types: ykpack::Types,
    yk_packs: Vec<ykpack::Body>,
    module: &mut cranelift_module::Module<impl cranelift_module::Backend>,
) {
    let mut buf = Vec::new();
    let mut encoder = ykpack::Encoder::from(&mut buf);

    // First we serialise the types which will be referenced in the body packs that will follow.
    encoder.serialise(ykpack::Pack::Types(yk_types)).unwrap();

    for func in yk_packs {
        encoder.serialise(ykpack::Pack::Body(func)).unwrap();
    }

    encoder.done().unwrap();

    // Borrowed from exported_symbols::metadata_symbol_name().
    let sym_name = format!(
        "{}_{}_{}",
        SIR_GLOBAL_SYM_PREFIX,
        tcx.original_crate_name(LOCAL_CRATE),
        tcx.crate_disambiguator(LOCAL_CRATE).to_fingerprint().to_hex()
    );

    let section_name =
        format!("{}{}", ykpack::SIR_SECTION_PREFIX, &*tcx.crate_name(LOCAL_CRATE).as_str());

    let mut data_ctx = cranelift_module::DataContext::new();
    data_ctx.define(buf.into_boxed_slice());
    data_ctx.set_segment_section("", &section_name);

    let data_id = module.declare_data(&sym_name, cranelift_module::Linkage::Export, false, false, None).unwrap();
    module.define_data(data_id, &data_ctx).unwrap();
}
