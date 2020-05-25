//! Replaces 128-bit operators with lang item calls where necessary

use cranelift_codegen::ir::ArgumentPurpose;

use crate::prelude::*;

pub(crate) fn maybe_codegen<'tcx>(
    fx: &mut FunctionCx<'_, 'tcx, impl Module>,
    bin_op: BinOp,
    checked: bool,
    lhs: CValue<'tcx>,
    rhs: CValue<'tcx>,
) -> Option<CValue<'tcx>> {
    if lhs.layout().ty != fx.tcx.types.u128 && lhs.layout().ty != fx.tcx.types.i128 {
        return None;
    }

    let lhs_val = lhs.load_scalar(fx);
    let rhs_val = rhs.load_scalar(fx);

    let is_signed = type_sign(lhs.layout().ty);

    match bin_op {
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            assert!(!checked);
            None
        }
        BinOp::Add | BinOp::Sub if !checked => None,
        BinOp::Mul if !checked => {
            let val_ty = if is_signed {
                fx.tcx.types.i128
            } else {
                fx.tcx.types.u128
            };
            if fx.tcx.sess.target.is_like_windows {
                let ret_place = CPlace::new_stack_slot(fx, lhs.layout());
                let (lhs_ptr, lhs_extra) = lhs.force_stack(fx);
                let (rhs_ptr, rhs_extra) = rhs.force_stack(fx);
                assert!(lhs_extra.is_none());
                assert!(rhs_extra.is_none());
                let args = [
                    ret_place.to_ptr().get_addr(fx),
                    lhs_ptr.get_addr(fx),
                    rhs_ptr.get_addr(fx),
                ];
                fx.lib_call(
                    "__multi3",
                    vec![
                        AbiParam::special(pointer_ty(fx.tcx), ArgumentPurpose::StructReturn),
                        AbiParam::new(pointer_ty(fx.tcx)),
                        AbiParam::new(pointer_ty(fx.tcx)),
                    ],
                    vec![],
                    &args,
                );
                Some(ret_place.to_cvalue(fx))
            } else {
                Some(fx.easy_call("__multi3", &[lhs, rhs], val_ty))
            }
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul => {
            assert!(checked);
            let out_ty = fx.tcx.mk_tup([lhs.layout().ty, fx.tcx.types.bool].iter());
            let out_place = CPlace::new_stack_slot(fx, fx.layout_of(out_ty));
            let (param_types, args) = if fx.tcx.sess.target.is_like_windows {
                let (lhs_ptr, lhs_extra) = lhs.force_stack(fx);
                let (rhs_ptr, rhs_extra) = rhs.force_stack(fx);
                assert!(lhs_extra.is_none());
                assert!(rhs_extra.is_none());
                (
                    vec![
                        AbiParam::special(pointer_ty(fx.tcx), ArgumentPurpose::StructReturn),
                        AbiParam::new(pointer_ty(fx.tcx)),
                        AbiParam::new(pointer_ty(fx.tcx)),
                    ],
                    [
                        out_place.to_ptr().get_addr(fx),
                        lhs_ptr.get_addr(fx),
                        rhs_ptr.get_addr(fx),
                    ],
                )
            } else {
                (
                    vec![
                        AbiParam::special(pointer_ty(fx.tcx), ArgumentPurpose::StructReturn),
                        AbiParam::new(types::I128),
                        AbiParam::new(types::I128),
                    ],
                    [
                        out_place.to_ptr().get_addr(fx),
                        lhs.load_scalar(fx),
                        rhs.load_scalar(fx),
                    ],
                )
            };
            let name = match (bin_op, is_signed) {
                (BinOp::Add, false) => "__rust_u128_addo",
                (BinOp::Add, true) => "__rust_i128_addo",
                (BinOp::Sub, false) => "__rust_u128_subo",
                (BinOp::Sub, true) => "__rust_i128_subo",
                (BinOp::Mul, false) => "__rust_u128_mulo",
                (BinOp::Mul, true) => "__rust_i128_mulo",
                _ => unreachable!(),
            };
            fx.lib_call(name, param_types, vec![], &args);
            Some(out_place.to_cvalue(fx))
        }
        BinOp::Offset => unreachable!("offset should only be used on pointers, not 128bit ints"),
        BinOp::Div | BinOp::Rem => {
            assert!(!checked);
            let name = match (bin_op, is_signed) {
                (BinOp::Div, false) => "__udivti3",
                (BinOp::Div, true) => "__divti3",
                (BinOp::Rem, false) => "__umodti3",
                (BinOp::Rem, true) => "__modti3",
                _ => unreachable!(),
            };
            if fx.tcx.sess.target.is_like_windows {
                let (lhs_ptr, lhs_extra) = lhs.force_stack(fx);
                let (rhs_ptr, rhs_extra) = rhs.force_stack(fx);
                assert!(lhs_extra.is_none());
                assert!(rhs_extra.is_none());
                let args = [lhs_ptr.get_addr(fx), rhs_ptr.get_addr(fx)];
                let ret = fx.lib_call(
                    name,
                    vec![
                        AbiParam::new(pointer_ty(fx.tcx)),
                        AbiParam::new(pointer_ty(fx.tcx)),
                    ],
                    vec![AbiParam::new(types::I64X2)],
                    &args,
                )[0];
                // FIXME use bitcast instead of store to get from i64x2 to i128
                let ret_place = CPlace::new_stack_slot(fx, lhs.layout());
                ret_place.to_ptr().store(fx, ret, MemFlags::trusted());
                Some(ret_place.to_cvalue(fx))
            } else {
                Some(fx.easy_call(name, &[lhs, rhs], lhs.layout().ty))
            }
        }
        BinOp::Lt | BinOp::Le | BinOp::Eq | BinOp::Ge | BinOp::Gt | BinOp::Ne => {
            assert!(!checked);
            None
        }
        BinOp::Shl | BinOp::Shr => {
            let is_overflow = if checked {
                // rhs >= 128

                // FIXME support non 128bit rhs
                /*let (rhs_lsb, rhs_msb) = fx.bcx.ins().isplit(rhs_val);
                let rhs_msb_gt_0 = fx.bcx.ins().icmp_imm(IntCC::NotEqual, rhs_msb, 0);
                let rhs_lsb_ge_128 = fx.bcx.ins().icmp_imm(IntCC::SignedGreaterThan, rhs_lsb, 127);
                let is_overflow = fx.bcx.ins().bor(rhs_msb_gt_0, rhs_lsb_ge_128);*/
                let is_overflow = fx.bcx.ins().bconst(types::B1, false);

                Some(fx.bcx.ins().bint(types::I8, is_overflow))
            } else {
                None
            };

            // Optimize `val >> 64`, because compiler_builtins uses it to deconstruct an 128bit
            // integer into its lsb and msb.
            // https://github.com/rust-lang-nursery/compiler-builtins/blob/79a6a1603d5672cbb9187ff41ff4d9b5048ac1cb/src/int/mod.rs#L217
            if resolve_value_imm(fx.bcx.func, rhs_val) == Some(64) {
                let (lhs_lsb, lhs_msb) = fx.bcx.ins().isplit(lhs_val);
                let all_zeros = fx.bcx.ins().iconst(types::I64, 0);
                let val = match (bin_op, is_signed) {
                    (BinOp::Shr, false) => {
                        let val = fx.bcx.ins().iconcat(lhs_msb, all_zeros);
                        Some(CValue::by_val(val, fx.layout_of(fx.tcx.types.u128)))
                    }
                    (BinOp::Shr, true) => {
                        let sign = fx.bcx.ins().icmp_imm(IntCC::SignedLessThan, lhs_msb, 0);
                        let all_ones = fx.bcx.ins().iconst(types::I64, u64::MAX as i64);
                        let all_sign_bits = fx.bcx.ins().select(sign, all_zeros, all_ones);

                        let val = fx.bcx.ins().iconcat(lhs_msb, all_sign_bits);
                        Some(CValue::by_val(val, fx.layout_of(fx.tcx.types.i128)))
                    }
                    (BinOp::Shl, _) => {
                        let val_ty = if is_signed {
                            fx.tcx.types.i128
                        } else {
                            fx.tcx.types.u128
                        };
                        let val = fx.bcx.ins().iconcat(all_zeros, lhs_lsb);
                        Some(CValue::by_val(val, fx.layout_of(val_ty)))
                    }
                    _ => None,
                };
                if let Some(val) = val {
                    if let Some(is_overflow) = is_overflow {
                        let out_ty = fx.tcx.mk_tup([lhs.layout().ty, fx.tcx.types.bool].iter());
                        let val = val.load_scalar(fx);
                        return Some(CValue::by_val_pair(val, is_overflow, fx.layout_of(out_ty)));
                    } else {
                        return Some(val);
                    }
                }
            }

            let truncated_rhs = clif_intcast(fx, rhs_val, types::I32, false);
            let truncated_rhs = CValue::by_val(truncated_rhs, fx.layout_of(fx.tcx.types.u32));
            let name = match (bin_op, is_signed) {
                (BinOp::Shl, false) => "__ashlti3",
                (BinOp::Shl, true) => "__ashlti3",
                (BinOp::Shr, false) => "__lshrti3",
                (BinOp::Shr, true) => "__ashrti3",
                _ => unreachable!(),
            };
            let val = if fx.tcx.sess.target.is_like_windows {
                crate::trap::trap_unimplemented_ret_value(
                    fx,
                    lhs.layout(),
                    &format!(
                        "{:?} is not implemented on Windows for 128bit ints.",
                        bin_op,
                    ),
                )
            } else {
                fx.easy_call(name, &[lhs, truncated_rhs], lhs.layout().ty)
            };
            if let Some(is_overflow) = is_overflow {
                let out_ty = fx.tcx.mk_tup([lhs.layout().ty, fx.tcx.types.bool].iter());
                let val = val.load_scalar(fx);
                Some(CValue::by_val_pair(val, is_overflow, fx.layout_of(out_ty)))
            } else {
                Some(val)
            }
        }
    }
}
