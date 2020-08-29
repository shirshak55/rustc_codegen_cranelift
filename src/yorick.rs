use std::convert::TryFrom;
use std::collections::HashMap;

use rustc_middle::ty::{TyCtxt, Instance};

use cranelift_codegen::entity::{EntitySet, SecondaryMap};
use cranelift_codegen::ir::{
    self, types, Block, Function, Inst, InstructionData, Opcode, Type, Value, ExternalName
};
use cranelift_module::FuncId;

pub(crate) struct ExtraInfo {
    pub(crate) sw_trace_insts: EntitySet<Inst>,
    pub(crate) func_names: HashMap<FuncId, String>,
}

impl Default for ExtraInfo {
    fn default() -> Self {
        Self {
            sw_trace_insts: EntitySet::new(),
            func_names: HashMap::new(),
        }
    }
}

fn pack_ty_for_stack_slot(types: &mut ykpack::Types, size: u32) -> (u64, u32) {
    // FIXME re-use types
    let ty_index = types.types.len() as u32;
    types.types.push(ykpack::Ty::Struct(ykpack::StructTy {
        fields: ykpack::Fields {
            offsets: vec![],
            tys: vec![],
        },
        size_align: ykpack::SizeAndAlign {
            size: i32::try_from(size).unwrap(),
            align: 8,
        },
    }));
    (types.crate_hash, ty_index)
}

pub fn preallocate_clif_types(types: &mut ykpack::Types) {
    types.types.push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U8));
    types.types.push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U16));
    types.types.push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U32));
    types.types.push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U64));
    types.types.push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U128));
}

fn pack_ty_for_clif_ty(types: &mut ykpack::Types, clif_ty: Type) -> (u64, u32) {
    match clif_ty {
        types::B1 => (types.crate_hash, 0),
        types::I8 => (types.crate_hash, 0),
        types::I16 => (types.crate_hash, 1),
        types::I32 => (types.crate_hash, 2),
        types::I64 => (types.crate_hash, 3),
        types::I128 => (types.crate_hash, 4),
        _ => todo!("{}", clif_ty),
    }
}

struct SirBuilder<'a> {
    types: &'a mut ykpack::Types,
    body: ykpack::Body,
    current_block: Option<ykpack::BasicBlockIndex>,
    block_map: SecondaryMap<Block, ykpack::BasicBlockIndex>,
    value_map: SecondaryMap<Value, u32>,
}

impl SirBuilder<'_> {
    fn create_block(&mut self) -> ykpack::BasicBlockIndex {
        let idx = self.body.blocks.len() as u32;
        self.body.blocks.push(ykpack::BasicBlock {
            stmts: vec![],
            term: ykpack::Terminator::Unreachable,
        });
        idx
    }

    fn switch_to_clif_block(&mut self, block: Block) {
        assert!(self.current_block.is_none());
        let i = self.block_map[block];
        assert!(i != 0);
        assert_eq!(self.body.blocks[(i - 1) as usize].term, ykpack::Terminator::Unreachable);
        assert!(self.body.blocks[(i - 1) as usize].stmts.is_empty());
        self.current_block = Some(i - 1);
    }

    fn switch_to_block(&mut self, block: ykpack::BasicBlockIndex) {
        assert!(self.current_block.is_none());
        assert_eq!(self.body.blocks[block as usize].term, ykpack::Terminator::Unreachable);
        assert!(self.body.blocks[block as usize].stmts.is_empty());
        self.current_block = Some(block);
    }

    fn bb_for_block(&self, block: Block) -> ykpack::BasicBlockIndex {
        let i = self.block_map[block];
        assert!(i != 0);
        i - 1
    }

    fn local_for_value(&mut self, val: Value, ty: Type) -> ykpack::Local {
        let i = self.value_map[val];
        if i != 0 {
            ykpack::Local(i - 1)
        } else {
            self.body.local_decls.push(ykpack::LocalDecl {
                ty: pack_ty_for_clif_ty(self.types, ty),
            });
            let i = self.body.local_decls.len() as u32;
            self.value_map[val] = i;
            ykpack::Local(i - 1)
        }
    }

    fn add_stmt(&mut self, stmt: ykpack::Statement) {
        self.body.blocks[self.current_block.expect("No current block") as usize].stmts.push(stmt);
    }

    fn terminate_block(&mut self, term: ykpack::Terminator) {
        self.body.blocks[self.current_block.expect("No current block") as usize].term = term;
        self.current_block = None;
    }

    fn finalize(self) -> ykpack::Body {
        assert!(self.current_block.is_none());
        self.body
    }
}

pub(crate) fn encode_sir<'tcx>(
    tcx: TyCtxt<'tcx>,
    instance: Instance<'tcx>,
    types: &mut ykpack::Types,
    symbol_name: &str,
    func: &Function,
    extra_info: ExtraInfo,
) -> ykpack::Body {
    //println!("====================================\n");
    //println!("{}", func);

    let mut body = ykpack::Body {
        symbol_name: symbol_name.to_string(),
        flags: 0,                 // FIXME
        trace_inputs_local: None, // FIXME
        local_decls: vec![],
        blocks: vec![],
    };

    let trace_head = rustc_span::Symbol::intern("trace_head");
    for attr in tcx.get_attrs(instance.def_id()).iter() {
        if tcx.sess.check_name(attr, trace_head) {
            println!("trace head");
            body.flags |= ykpack::bodyflags::TRACE_HEAD;
        }
    }

    let trace_tail = rustc_span::Symbol::intern("trace_tail");
    for attr in tcx.get_attrs(instance.def_id()).iter() {
        if tcx.sess.check_name(attr, trace_tail) {
            body.flags |= ykpack::bodyflags::TRACE_TAIL;
        }
    }

    // Return place
    body.local_decls.push(ykpack::LocalDecl {
        ty: pack_ty_for_clif_ty(types, types::I64),
    });

    let mut stack_slot_map =
        cranelift_codegen::entity::SecondaryMap::with_capacity(func.stack_slots.keys().count());
    for stack_slot in func.stack_slots.keys() {
        stack_slot_map[stack_slot] = body.local_decls.len() as u32 + 1;
        body.local_decls.push(ykpack::LocalDecl {
            ty: pack_ty_for_stack_slot(types, func.stack_slots[stack_slot].size),
        });
    }
    let local_for_stack_slot = |stack_slot| {
        let i = stack_slot_map[stack_slot];
        assert!(i != 0);
        i - 1
    };

    let mut sir_builder = SirBuilder {
        types,
        body,
        current_block: None,
        block_map: SecondaryMap::new(),
        value_map: SecondaryMap::new(),
    };

    for block in func.layout.blocks() {
        sir_builder.block_map[block] = sir_builder.create_block() + 1;
    }

    for block in func.layout.blocks() {
        sir_builder.switch_to_clif_block(block);
        for inst in func.layout.block_insts(block) {
            if extra_info.sw_trace_insts.contains(inst) {
                continue; // skip software tracing calls
            }

            if func.dfg.inst_args(inst).iter().chain(func.dfg.inst_results(inst)).any(|&arg| func.dfg.value_type(arg).is_float() || func.dfg.value_type(arg).is_vector()) {
                // floats and vector types not yet supported by Yorick
                if func.dfg[inst].opcode().is_terminator() {
                    sir_builder.terminate_block(ykpack::Terminator::Unimplemented(format!("{:?}", func.dfg[inst])));
                } else {
                    sir_builder.add_stmt(ykpack::Statement::Unimplemented(format!("{:?}", func.dfg[inst])));
                }
                continue;
            }

            match &func.dfg[inst] {
                InstructionData::NullAry {
                    opcode: Opcode::Nop,
                } => {}
                InstructionData::UnaryBool {
                    opcode: Opcode::Bconst,
                    imm,
                } => {
                    let local = sir_builder.local_for_value(
                        func.dfg.first_result(inst),
                        func.dfg.ctrl_typevar(inst),
                    );
                    sir_builder.add_stmt(ykpack::Statement::Assign(
                        ykpack::Place {
                            local,
                            projection: vec![],
                        },
                        ykpack::Rvalue::Use(ykpack::Operand::Constant(ykpack::Constant::Bool(
                            *imm,
                        ))),
                    ));
                }
                InstructionData::UnaryImm {
                    opcode: Opcode::Iconst,
                    imm,
                } => {
                    let local = sir_builder.local_for_value(
                        func.dfg.first_result(inst),
                        func.dfg.ctrl_typevar(inst),
                    );
                    sir_builder.add_stmt(ykpack::Statement::Assign(
                        ykpack::Place {
                            local,
                            projection: vec![],
                        },
                        ykpack::Rvalue::Use(ykpack::Operand::Constant(ykpack::Constant::Int(
                            ykpack::ConstantInt::UnsignedInt(match func.dfg.ctrl_typevar(inst) {
                                types::I8 => ykpack::UnsignedInt::U8(imm.bits() as u8),
                                types::I16 => ykpack::UnsignedInt::U16(imm.bits() as u16),
                                types::I32 => ykpack::UnsignedInt::U32(imm.bits() as u32),
                                types::I64 => ykpack::UnsignedInt::U64(imm.bits() as u64),
                                ty => todo!("{}", ty),
                            }),
                        ))),
                    ))
                }
                InstructionData::Unary {
                    opcode: Opcode::Bint,
                    arg,
                } => {
                    let local = sir_builder.local_for_value(
                        func.dfg.first_result(inst),
                        func.dfg.ctrl_typevar(inst),
                    );
                    let arg = sir_builder.local_for_value(
                        func.dfg.resolve_aliases(*arg),
                        func.dfg.value_type(*arg),
                    );
                    sir_builder.add_stmt(ykpack::Statement::Assign(
                        ykpack::Place {
                            local,
                            projection: vec![],
                        },
                        ykpack::Rvalue::Use(ykpack::Operand::Place(ykpack::Place {
                            local: arg,
                            projection: vec![],
                        })),
                    ))
                }
                InstructionData::Unary {
                    opcode: Opcode::Uextend,
                    arg,
                } => {
                    let local = sir_builder.local_for_value(
                        func.dfg.first_result(inst),
                        func.dfg.ctrl_typevar(inst),
                    );
                    let arg = sir_builder.local_for_value(
                        func.dfg.resolve_aliases(*arg),
                        func.dfg.value_type(*arg),
                    );
                    sir_builder.add_stmt(ykpack::Statement::Assign(
                        ykpack::Place {
                            local,
                            projection: vec![],
                        },
                        ykpack::Rvalue::Use(ykpack::Operand::Place(ykpack::Place {
                            local: arg,
                            projection: vec![],
                        })),
                    ))
                }
                InstructionData::IntCompare {
                    opcode: Opcode::Icmp,
                    args: [x, y],
                    cond,
                } => {
                    let local = sir_builder.local_for_value(
                        func.dfg.first_result(inst),
                        func.dfg.ctrl_typevar(inst),
                    );
                    let x = sir_builder.local_for_value(
                        func.dfg.resolve_aliases(*x),
                        func.dfg.value_type(*x),
                    );
                    let y = sir_builder.local_for_value(
                        func.dfg.resolve_aliases(*y),
                        func.dfg.value_type(*y),
                    );

                    sir_builder.add_stmt(ykpack::Statement::Assign(
                        ykpack::Place {
                            local,
                            projection: vec![],
                        },
                        ykpack::Rvalue::BinaryOp(
                            match *cond {
                                ir::condcodes::IntCC::Equal => ykpack::BinOp::Eq,
                                ir::condcodes::IntCC::UnsignedLessThan => ykpack::BinOp::Lt,
                                _ => ykpack::BinOp::BitXor, // FIXME
                            },
                            ykpack::Operand::Place(ykpack::Place {
                                local: x,
                                projection: vec![],
                            }),
                            ykpack::Operand::Place(ykpack::Place {
                                local: y,
                                projection: vec![],
                            }),
                        ),
                    ));
                }

                inst_data
                @
                InstructionData::Call {
                    opcode: Opcode::Call,
                    args,
                    func_ref,
                } => {
                    let call_operand = match func.dfg.ext_funcs[*func_ref].name {
                        ExternalName::TestCase { length, ascii } => unreachable!(),
                        ExternalName::User { namespace, index } => {
                            assert_eq!(namespace, 0);
                            extra_info
                                .func_names
                                .get(&FuncId::from_u32(index))
                                .cloned()
                                .map(ykpack::CallOperand::Fn)
                                .unwrap_or(ykpack::CallOperand::Unknown) // FIXME
                        }
                        ExternalName::LibCall(_) => ykpack::CallOperand::Unknown, // FIXME
                    };

                    let next_block = sir_builder.create_block();
                    sir_builder.terminate_block(ykpack::Terminator::Call {
                        operand: call_operand,
                        args: vec![],
                        destination: Some((ykpack::Place {
                            local: ykpack::Local(1), // FIXME
                            projection: vec![],
                        }, next_block)),
                    });
                    sir_builder.switch_to_block(next_block);
                    // FIXME
                    for ret_val in func.dfg.inst_results(inst) {
                        let ret_val_local = sir_builder.local_for_value(
                            *ret_val,
                            func.dfg.value_type(*ret_val),
                        );
                        sir_builder.add_stmt(ykpack::Statement::Assign(ykpack::Place {
                            local: ret_val_local,
                            projection: vec![],
                        }, ykpack::Rvalue::Use(ykpack::Operand::Constant(ykpack::Constant::Int(
                            ykpack::ConstantInt::UnsignedInt(match func.dfg.value_type(*ret_val) {
                                types::I8 => ykpack::UnsignedInt::U8(0),
                                types::I16 => ykpack::UnsignedInt::U16(0),
                                types::I32 => ykpack::UnsignedInt::U32(0),
                                types::I64 => ykpack::UnsignedInt::U64(0),
                                ty => continue,
                            }),
                        )))));
                    }
                    /*if args.len(&func.dfg.value_lists) != 0 {
                        sir_builder
                            .add_stmt(ykpack::Statement::Unimplemented(format!("{:?}", inst_data)))
                    } else {
                        let dest = match func.dfg.inst_results(inst) {
                            [] => None,
                            [ret_val] => {
                                let ret_val = sir_builder.local_for_value(
                                    *ret_val,
                                    func.dfg.value_type(*ret_val),
                                );
                                Some(ykpack::Place {
                                    local: ret_val,
                                    projection: vec![],
                                })
                            },
                            ret_vals => {
                                sir_builder.add_stmt(ykpack::Statement::Unimplemented(format!("{:?}", inst)));
                                continue;
                            }
                        };
                        sir_builder.add_stmt(ykpack::Statement::Call(
                            ykpack::CallOperand::Unknown,
                            vec![],
                            dest,
                        ));
                    }*/
                }

                InstructionData::Jump {
                    opcode: Opcode::Jump,
                    args,
                    destination,
                } => {
                    for (param, arg) in func.dfg.block_params(*destination).iter().zip(args.as_slice(&func.dfg.value_lists).iter()) {
                        let param = sir_builder.local_for_value(
                            *param,
                            func.dfg.value_type(*param),
                        );
                        let arg = sir_builder.local_for_value(
                            func.dfg.resolve_aliases(*arg),
                            func.dfg.value_type(*arg),
                        );
                        sir_builder.add_stmt(ykpack::Statement::Assign(
                            ykpack::Place {
                                local: param,
                                projection: vec![],
                            },
                            ykpack::Rvalue::Use(ykpack::Operand::Place(ykpack::Place {
                                local: arg,
                                projection: vec![],
                            })),
                        ))
                    }
                    sir_builder.terminate_block(ykpack::Terminator::Goto(
                        sir_builder.bb_for_block(*destination),
                    ));
                }
                InstructionData::Branch {
                    opcode: Opcode::Brz,
                    args,
                    destination,
                } => {
                    let arg = sir_builder.local_for_value(
                        func.dfg.resolve_aliases(args.get(0, &func.dfg.value_lists).unwrap()),
                        func.dfg.ctrl_typevar(inst),
                    );
                    // FIXME write block params
                    sir_builder.terminate_block(ykpack::Terminator::SwitchInt {
                        discr: ykpack::Place {
                            local: arg,
                            projection: vec![],
                        },
                        values: vec![ykpack::SerU128::new(0)],
                        target_bbs: vec![sir_builder.bb_for_block(*destination)],
                        otherwise_bb: 0, // FIXME
                    });
                    break;
                }
                InstructionData::MultiAry {
                    opcode: Opcode::Return,
                    args,
                } => {
                    // FIXME write return params
                    sir_builder.add_stmt(ykpack::Statement::Assign(ykpack::Place {
                        local: ykpack::Local(0),
                        projection: vec![],
                    }, ykpack::Rvalue::Use(ykpack::Operand::Constant(ykpack::Constant::Int(
                        ykpack::ConstantInt::UnsignedInt(ykpack::UnsignedInt::U64(0)),
                    )))));
                    sir_builder.terminate_block(ykpack::Terminator::Return);
                }
                inst if inst.opcode().is_terminator() => {
                    sir_builder
                        .terminate_block(ykpack::Terminator::Unimplemented(format!("{:?}", inst)));
                    break;
                }
                inst_data => {
                    sir_builder.add_stmt(ykpack::Statement::Unimplemented(format!("{:?}", inst_data)));
                    /*for ret_val in func.dfg.inst_results(inst) {
                        if !func.dfg.value_type(*ret_val).is_int() {
                            continue;
                        }
                        let ret_val_local = sir_builder.local_for_value(
                            *ret_val,
                            func.dfg.value_type(*ret_val),
                        );
                        sir_builder.add_stmt(ykpack::Statement::Assign(ykpack::Place {
                            local: ret_val_local,
                            projection: vec![],
                        }, ykpack::Rvalue::Use(ykpack::Operand::Constant(ykpack::Constant::Int(
                            ykpack::ConstantInt::UnsignedInt(match func.dfg.value_type(*ret_val) {
                                types::I8 => ykpack::UnsignedInt::U8(0),
                                types::I16 => ykpack::UnsignedInt::U16(0),
                                types::I32 => ykpack::UnsignedInt::U32(0),
                                types::I64 => ykpack::UnsignedInt::U64(0),
                                ty => continue,
                            }),
                        )))));
                    };*/
                }
            }
        }
    }

    let body = sir_builder.finalize();

    //println!("{}", body);

    body
}
