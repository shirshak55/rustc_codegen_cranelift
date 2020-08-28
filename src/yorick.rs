use std::convert::TryFrom;

use cranelift_codegen::entity::SecondaryMap;
use cranelift_codegen::ir::{
    self, types, Block, Function, Inst, InstructionData, Opcode, Type, Value,
};

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

fn pack_ty_for_clif_ty(types: &mut ykpack::Types, clif_ty: Type) -> (u64, u32) {
    // FIXME re-use types
    let ty_index = types.types.len() as u32;
    match clif_ty {
        types::B1 => types
            .types
            .push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U8)),
        types::I8 => types
            .types
            .push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U8)),
        types::I16 => types
            .types
            .push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U16)),
        types::I32 => types
            .types
            .push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U32)),
        types::I64 => types
            .types
            .push(ykpack::Ty::UnsignedInt(ykpack::UnsignedIntTy::U64)),
        _ => todo!("{}", clif_ty),
    }
    (types.crate_hash, ty_index)
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

    fn switch_to_block(&mut self, block: Block) {
        assert!(self.current_block.is_none());
        let i = self.block_map[block];
        assert!(i != 0);
        assert_eq!(self.body.blocks[(i - 1) as usize].term, ykpack::Terminator::Unreachable);
        assert!(self.body.blocks[(i - 1) as usize].stmts.is_empty());
        self.current_block = Some(i - 1);
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

pub(crate) fn encode_sir(
    types: &mut ykpack::Types,
    symbol_name: &str,
    func: &Function,
) -> ykpack::Body {
    println!("====================================\n");
    println!("{}", func);

    let mut body = ykpack::Body {
        symbol_name: symbol_name.to_string(),
        flags: 0,                 // FIXME
        trace_inputs_local: None, // FIXME
        local_decls: vec![],
        blocks: vec![],
    };

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
        sir_builder.switch_to_block(block);
        for inst in func.layout.block_insts(block) {
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
                        *arg,
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
                        *arg,
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
                        *x,
                        func.dfg.value_type(*x),
                    );
                    let y = sir_builder.local_for_value(
                        *y,
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
                    func_ref: _,
                } => {
                    if args.len(&func.dfg.value_lists) != 0 {
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
                            ret_vals => panic!("{:?}", ret_vals),
                        };
                        sir_builder.add_stmt(ykpack::Statement::Call(
                            ykpack::CallOperand::Unknown,
                            vec![],
                            dest,
                        ));
                    }
                }

                InstructionData::Jump {
                    opcode: Opcode::Jump,
                    args,
                    destination,
                } => {
                    // FIXME write block params
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
                        args.get(0, &func.dfg.value_lists).unwrap(),
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
                    sir_builder.terminate_block(ykpack::Terminator::Return);
                }
                inst if inst.opcode().is_terminator() => {
                    sir_builder
                        .terminate_block(ykpack::Terminator::Unimplemented(format!("{:?}", inst)));
                    break;
                }
                inst => sir_builder.add_stmt(ykpack::Statement::Unimplemented(format!("{:?}", inst))),
            }
        }
    }

    let body = sir_builder.finalize();

    println!("{}", body);

    body
}
