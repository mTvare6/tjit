use crate::parser::{Op, Type};
use crate::type_system::{
    EnumLayout, StructLayout, TypedExpr, TypedPat, align_to, bit_size_of, size_and_align_of,
};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use std::collections::HashMap;
use std::mem;

type LoopStack = Vec<(Block, Block)>;
type VariableMap = HashMap<String, Variable>;

extern "C" fn print_i64(val: i64) -> i64 {
    println!("=> {}", val);
    0
}

extern "C" fn print_str(fat_ptr_addr: i64) -> i64 {
    let rodata_ptr = unsafe { *(fat_ptr_addr as *const i64) };
    let len = unsafe { *((fat_ptr_addr + 8) as *const i64) };
    let slice = unsafe { std::slice::from_raw_parts(rodata_ptr as *const u8, len as usize) };
    let s = std::str::from_utf8(slice).unwrap();

    println!("=> {}", s);
    0
}

// mimic memcpy for inline value semantics
fn emit_memory_copy(builder: &mut FunctionBuilder, src_ptr: Value, dest_ptr: Value, size: u32) {
    // usually compilers links memcpy but unrolling 8b chunk loop is good enough
    let mut offset = 0;
    while offset < size {
        let chunk = builder
            .ins()
            .load(types::I64, MemFlags::new(), src_ptr, offset as i32);
        builder
            .ins()
            .store(MemFlags::new(), chunk, dest_ptr, offset as i32);
        offset += 8;
    }
}

// for selecting arithmetic instructions
macro_rules! emit_binary_op {
    ($builder:expr, $ty:expr, $lhs:expr, $rhs:expr, $int_op:ident, $uint_op:ident, $float_op:ident) => {
        match $ty {
            Type::Int(_) => $builder.ins().$int_op($lhs, $rhs),
            Type::UInt(_) => $builder.ins().$uint_op($lhs, $rhs),
            Type::F32 | Type::F64 => $builder.ins().$float_op($lhs, $rhs),
            Type::Custom(_) | Type::Array(..) | Type::Enum(..) | Type::String => {
                panic!("Fatal: Cannot perform arithmetic operations on non numeric type")
            }
        }
    };
}

// for selecting relational conditional instructions
macro_rules! emit_cmp_op {
    ($builder:expr, $ty:expr, $lhs:expr, $rhs:expr, $int_cc:ident, $uint_cc:ident, $float_cc:ident) => {
        match $ty {
            Type::Int(_) => {
                let b = $builder.ins().icmp(IntCC::$int_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::UInt(_) => {
                let b = $builder.ins().icmp(IntCC::$uint_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::F32 | Type::F64 => {
                let b = $builder.ins().fcmp(FloatCC::$float_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::Custom(_) | Type::Array(..) | Type::Enum(..) | Type::String => {
                panic!("Fatal: Cannot compare raw struct memory directly")
            }
        }
    };
}

pub struct JITEngine {
    builder_ctx: FunctionBuilderContext,
    ctx: codegen::Context,
    module: JITModule,
}

impl JITEngine {
    pub fn new() -> Self {
        // setup architecture flags
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap(); // jiting, no position independent code needed

        let isa_builder = cranelift_native::builder().unwrap();
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();

        // get executable memory
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        builder.symbol("print", print_i64 as *const u8);
        builder.symbol("print_str", print_str as *const u8);

        let module = JITModule::new(builder);

        Self {
            builder_ctx: FunctionBuilderContext::new(),
            ctx: module.make_context(),
            module,
        }
    }

    fn compile_function(
        &mut self,
        name: &str,
        params: &[(String, Type)],
        ret_ty: &Type,
        body: &TypedExpr,
        structs: &HashMap<String, StructLayout>,
        enums: &HashMap<String, EnumLayout>,
    ) -> Result<(), String> {
        self.module.clear_context(&mut self.ctx);

        for (_, param_type) in params {
            let cl_type = param_type.into();
            self.ctx.func.signature.params.push(AbiParam::new(cl_type));
        }

        let cl_ret_type = ret_ty.into();
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(cl_ret_type));

        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);

        // create the entry block (the `{` of the function)
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let mut variables = HashMap::new();
        let mut variable_index = 0;
        let mut loop_stack = Vec::new();

        for (i, (param_name, param_ty)) in params.iter().enumerate() {
            let val = builder.block_params(entry_block)[i];
            let var = Variable::new(variable_index);
            variable_index += 1;
            let cl_type = param_ty.into();
            builder.declare_var(var, cl_type);
            builder.def_var(var, val);
            variables.insert(param_name.clone(), var);
        }

        let return_val = Self::compile_expr(
            body,
            &mut self.module,
            &mut builder,
            &mut variables,
            &mut variable_index,
            &mut loop_stack,
            &structs,
            &enums,
        )
        .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));

        // emit the return instruction (the `}` of the function)
        builder.ins().return_(&[return_val]);
        builder.finalize();

        // register the function in the module
        let id = self
            .module
            .declare_function(name, Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| e.to_string())?;

        self.module
            .define_function(id, &mut self.ctx)
            .map_err(|e| {
                println!("{}", self.ctx.func.display());
                e.to_string()
            })?;

        // clear the context so we can reuse the engine for the next script
        Ok(())
    }

    // recursive decision tree generator
    fn emit_pattern_match(
        builder: &mut FunctionBuilder,
        pat: &TypedPat,
        ptr: Value,        // physical memory address being inspected
        fail_block: Block, // wildcard if all fails
        variables: &mut VariableMap,
        variable_index: &mut usize,
        structs: &HashMap<String, StructLayout>,
        enums: &HashMap<String, EnumLayout>,
    ) {
        match pat {
            TypedPat::Wildcard(_) => { /* always match, emit nothing */ }
            TypedPat::Number(n, p_ty) => {
                let cl_type = p_ty.into();
                let val = builder.ins().load(cl_type, MemFlags::new(), ptr, 0);
                let expected = builder.ins().iconst(cl_type, *n);
                let is_match = builder.ins().icmp(IntCC::Equal, val, expected);

                let success_block = builder.create_block();
                builder
                    .ins()
                    .brif(is_match, success_block, &[], fail_block, &[]);
                builder.switch_to_block(success_block);
                builder.seal_block(success_block);
            }
            TypedPat::Range(start, end, inclusive, p_ty) => {
                let cl_type = p_ty.into();
                let val = builder.ins().load(cl_type, MemFlags::new(), ptr, 0);
                let s_val = builder.ins().iconst(cl_type, *start);
                let e_val = builder.ins().iconst(cl_type, *end);

                let cmp_start = builder
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, val, s_val);
                let cmp_end = if *inclusive {
                    builder.ins().icmp(IntCC::SignedLessThanOrEqual, val, e_val)
                } else {
                    builder.ins().icmp(IntCC::SignedLessThan, val, e_val)
                };

                let in_bounds = builder.ins().band(cmp_start, cmp_end);

                let success_block = builder.create_block();
                builder
                    .ins()
                    .brif(in_bounds, success_block, &[], fail_block, &[]);
                builder.switch_to_block(success_block);
                builder.seal_block(success_block);
            }

            TypedPat::Variable(name, p_ty) => {
                // load the value and bind it to a cl variable
                let val = builder.ins().load(p_ty.into(), MemFlags::new(), ptr, 0);
                let var = Variable::new(*variable_index);
                *variable_index += 1;
                builder.declare_var(var, p_ty.into());
                builder.def_var(var, val);
                variables.insert(name.clone(), var);
            }

            TypedPat::Enum(enum_name, variant_name, payloads, _) => {
                let layout = enums.get(enum_name).unwrap();
                let (expected_tag, expected_payload_tys) =
                    layout.variants.get(variant_name).unwrap();

                let tag_val = builder.ins().load(types::I32, MemFlags::new(), ptr, 0);
                let expected_tag_val = builder.ins().iconst(types::I32, *expected_tag as i64);
                let is_match = builder.ins().icmp(IntCC::Equal, tag_val, expected_tag_val);

                let success_block = builder.create_block();
                builder
                    .ins()
                    .brif(is_match, success_block, &[], fail_block, &[]);
                builder.switch_to_block(success_block);
                builder.seal_block(success_block);

                for (i, p) in payloads.iter().enumerate() {
                    let (expected_ty, bit_offset) = &expected_payload_tys[i];
                    let bits = crate::type_system::bit_size_of(expected_ty);
                    let word_offset = (*bit_offset / 64) * 8;
                    let shift = *bit_offset % 64;

                    let extracted_val = if expected_ty.is_integer_type() && bits < 64 {
                        let chunk = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            ptr,
                            word_offset as i32,
                        );
                        let shifted = builder.ins().ushr_imm(chunk, shift as i64);
                        let raw_mask = 1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1);
                        let mask_val = builder.ins().iconst(types::I64, raw_mask as i64);
                        let mut isolated = builder.ins().band(shifted, mask_val);

                        if let Type::Int(_) = expected_ty {
                            let shift_up = 64 - bits;
                            isolated = builder.ins().ishl_imm(isolated, shift_up as i64);
                            isolated = builder.ins().sshr_imm(isolated, shift_up as i64);
                        }
                        isolated
                    } else {
                        let cl_type = expected_ty.into();
                        builder
                            .ins()
                            .load(cl_type, MemFlags::new(), ptr, word_offset as i32)
                    };

                    // Allocate temp slot for the nested pattern to inspect
                    let temp_slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8);
                    let temp_slot = builder.create_sized_stack_slot(temp_slot_data);
                    let temp_ptr = builder.ins().stack_addr(types::I64, temp_slot, 0);

                    if expected_ty.is_primitive() {
                        builder
                            .ins()
                            .store(MemFlags::new(), extracted_val, temp_ptr, 0);
                        Self::emit_pattern_match(
                            builder,
                            p,
                            temp_ptr,
                            fail_block,
                            variables,
                            variable_index,
                            structs,
                            enums,
                        );
                    } else {
                        Self::emit_pattern_match(
                            builder,
                            p,
                            extracted_val,
                            fail_block,
                            variables,
                            variable_index,
                            structs,
                            enums,
                        );
                    }
                }
            }

            TypedPat::Struct(struct_name, fields, _) => {
                let layout = structs.get(struct_name).unwrap();

                for (f_name, f_pat) in fields {
                    let (expected_ty, bit_offset) = layout.fields.get(f_name).unwrap();
                    let bits = crate::type_system::bit_size_of(expected_ty);
                    let word_offset = (*bit_offset / 64) * 8;
                    let shift = *bit_offset % 64;

                    let extracted_val = if expected_ty.is_integer_type() && bits < 64 {
                        let chunk = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            ptr,
                            word_offset as i32,
                        );
                        let shifted = builder.ins().ushr_imm(chunk, shift as i64);
                        let raw_mask = 1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1);
                        let mask_val = builder.ins().iconst(types::I64, raw_mask as i64);
                        let mut isolated = builder.ins().band(shifted, mask_val);

                        if let Type::Int(_) = expected_ty {
                            let shift_up = 64 - bits;
                            isolated = builder.ins().ishl_imm(isolated, shift_up as i64);
                            isolated = builder.ins().sshr_imm(isolated, shift_up as i64);
                        }
                        isolated
                    } else {
                        let cl_type = expected_ty.into();
                        builder
                            .ins()
                            .load(cl_type, MemFlags::new(), ptr, word_offset as i32)
                    };

                    let temp_slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8);
                    let temp_slot = builder.create_sized_stack_slot(temp_slot_data);
                    let temp_ptr = builder.ins().stack_addr(types::I64, temp_slot, 0);

                    if expected_ty.is_primitive() {
                        builder
                            .ins()
                            .store(MemFlags::new(), extracted_val, temp_ptr, 0);
                        Self::emit_pattern_match(
                            builder,
                            f_pat,
                            temp_ptr,
                            fail_block,
                            variables,
                            variable_index,
                            structs,
                            enums,
                        );
                    } else {
                        Self::emit_pattern_match(
                            builder,
                            f_pat,
                            extracted_val,
                            fail_block,
                            variables,
                            variable_index,
                            structs,
                            enums,
                        );
                    }
                }
            }
        }
    }

    pub fn compile(
        &mut self,
        program: &[TypedExpr],
        structs: &HashMap<String, StructLayout>,
        enums: &HashMap<String, EnumLayout>,
    ) -> Result<fn() -> i64, String> {
        // global function compilation
        for expr in program {
            if let TypedExpr::FnDecl(name, params, ret_ty, body) = expr {
                self.compile_function(name, params, ret_ty, body, structs, enums)?;
            }
        }

        self.module.clear_context(&mut self.ctx);
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I64));

        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);

        // create the entry block (the `{` of the function)
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let mut variables = HashMap::new();
        let mut variable_index = 0;
        let mut loop_stack = Vec::new();

        // default return value
        let mut return_val = builder.ins().iconst(types::I64, 0);
        let mut final_type = Type::Int(64);

        for expr in program {
            // bypass function declarations during the local pass
            if matches!(expr, TypedExpr::FnDecl(..) | TypedExpr::StructDecl(..)) {
                continue;
            }
            if let Some(val) = Self::compile_expr(
                expr,
                &mut self.module,
                &mut builder,
                &mut variables,
                &mut variable_index,
                &mut loop_stack,
                &structs,
                &enums,
            ) {
                return_val = val;
                final_type = expr.ty(); // track the type of the last evaluation
            }
        }

        // ABI coercion, force the final value to match the i64 return signature
        match final_type {
            Type::F32 | Type::F64 => {
                return_val = builder.ins().fcvt_to_sint(types::I64, return_val);
            }
            Type::Int(bits) => {
                let shift_up = 64 - bits;
                return_val = builder.ins().ishl_imm(return_val, shift_up as i64);
                return_val = builder.ins().sshr_imm(return_val, shift_up as i64);
            }
            Type::UInt(bits) => {
                let mask = 1u64.checked_shl(bits as u32).unwrap_or(0).wrapping_sub(1);
                let mask_val = builder.ins().iconst(types::I64, mask as i64);
                return_val = builder.ins().band(return_val, mask_val);
            }
            _ => {} // arrays, structs, enums don't get returned
        }

        // emit the return instruction (the `}` of the function)
        builder.ins().return_(&[return_val]);
        builder.finalize();

        // define and finalize the machine code into RAM
        let id = self
            .module
            .declare_function("main", Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| e.to_string())?;

        self.module
            .define_function(id, &mut self.ctx)
            .map_err(|e| {
                // dump the raw IR to the terminal on failure
                println!("{}", self.ctx.func.display());
                e.to_string()
            })?;

        // clear the context so we can reuse the engine for the next script
        self.module.clear_context(&mut self.ctx);
        self.module.finalize_definitions().unwrap();

        // transmute it to a safe rust function signature
        let code_ptr = self.module.get_finalized_function(id);
        unsafe { Ok(mem::transmute::<*const u8, fn() -> i64>(code_ptr)) }
    }

    fn compile_expr(
        expr: &TypedExpr,
        module: &mut JITModule,
        builder: &mut FunctionBuilder,
        variables: &mut VariableMap,
        variable_index: &mut usize,
        loop_stack: &mut LoopStack,
        structs: &HashMap<String, StructLayout>,
        enums: &HashMap<String, EnumLayout>,
    ) -> Option<Value> {
        match expr {
            TypedExpr::StringLiteral(s) => {
                let mut data_ctx = cranelift_module::DataDescription::new();
                let bytes = s.clone().into_bytes();
                let len = bytes.len() as i64;

                // no null terminator(fat pointer)
                data_ctx.define(bytes.into_boxed_slice());

                // unique id for this string
                static STR_ID: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let id = STR_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                let data_id = module
                    .declare_data(
                        &format!("str_{}", id),
                        Linkage::Local,
                        false, // read-only memory
                        false,
                    )
                    .unwrap();

                // define in binary and get the pointer to the text
                module.define_data(data_id, &data_ctx).unwrap();
                let local_id = module.declare_data_in_func(data_id, &mut builder.func);
                let rodata_ptr = builder.ins().symbol_value(types::I64, local_id);
                let len_val = builder.ins().iconst(types::I64, len);

                // 16 bytes on the stack for (ptr, len) tuple
                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, 16);
                let slot = builder.create_sized_stack_slot(slot_data);
                let fat_ptr_base = builder.ins().stack_addr(types::I64, slot, 0);

                builder
                    .ins()
                    .store(MemFlags::new(), rodata_ptr, fat_ptr_base, 0);
                builder
                    .ins()
                    .store(MemFlags::new(), len_val, fat_ptr_base, 8);

                Some(fat_ptr_base)
            }
            TypedExpr::Number(n, num_ty) => {
                let cl_type = num_ty.into();
                Some(builder.ins().iconst(cl_type, *n))
            }
            TypedExpr::Float(f) => Some(builder.ins().f64const(*f)),
            TypedExpr::BinaryOp(left, op, right, op_type) => {
                let lhs = Self::compile_expr(
                    left,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;
                let rhs = Self::compile_expr(
                    right,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                Some(match op {
                    Op::Add => emit_binary_op!(builder, op_type, lhs, rhs, iadd, iadd, fadd),
                    Op::Subtract => emit_binary_op!(builder, op_type, lhs, rhs, isub, isub, fsub),
                    Op::Multiply => emit_binary_op!(builder, op_type, lhs, rhs, imul, imul, fmul),
                    Op::Divide => emit_binary_op!(builder, op_type, lhs, rhs, sdiv, udiv, fdiv),

                    Op::Eq => emit_cmp_op!(builder, op_type, lhs, rhs, Equal, Equal, Equal),
                    Op::Lt => emit_cmp_op!(
                        builder,
                        op_type,
                        lhs,
                        rhs,
                        SignedLessThan,
                        UnsignedLessThan,
                        LessThan
                    ),
                    Op::Le => emit_cmp_op!(
                        builder,
                        op_type,
                        lhs,
                        rhs,
                        SignedLessThanOrEqual,
                        UnsignedLessThanOrEqual,
                        LessThanOrEqual
                    ),
                    Op::Gt => emit_cmp_op!(
                        builder,
                        op_type,
                        lhs,
                        rhs,
                        SignedGreaterThan,
                        UnsignedGreaterThan,
                        GreaterThan
                    ),
                    Op::Ge => emit_cmp_op!(
                        builder,
                        op_type,
                        lhs,
                        rhs,
                        SignedGreaterThanOrEqual,
                        UnsignedGreaterThanOrEqual,
                        GreaterThanOrEqual
                    ),
                })
            }
            TypedExpr::Variable(name, _) => Some(
                variables
                    .get(name)
                    .map(|var| builder.use_var(*var))
                    .unwrap_or_else(|| panic!("Undefined variable: {}", name)),
            ),
            TypedExpr::Let(name, declared_ty, value) => {
                let val = Self::compile_expr(
                    value,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;
                let var = Variable::new(*variable_index);
                *variable_index += 1;

                let cl_type = declared_ty.into();
                builder.declare_var(var, cl_type);
                builder.def_var(var, val);
                variables.insert(name.clone(), var);
                Some(val) // return of let is the value assigned
            }
            TypedExpr::Assign(name, value, _) => {
                let val = Self::compile_expr(
                    value,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;
                variables
                    .get(name)
                    .map(|var| builder.def_var(*var, val))
                    .unwrap_or_else(|| panic!("Undefined variable: {}", name));
                Some(val)
            }
            TypedExpr::If(cond, then_branch, else_branch, branch_type) => {
                let cond_val = Self::compile_expr(
                    cond,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let merge_block = builder.create_block();

                let cl_type = branch_type.into();
                builder.append_block_param(merge_block, cl_type);

                builder
                    .ins()
                    .brif(cond_val, then_block, &[], else_block, &[]);

                builder.switch_to_block(then_block);
                builder.seal_block(then_block);
                let then_val = Self::compile_expr(
                    then_branch,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                );
                if let Some(val) = then_val {
                    builder.ins().jump(merge_block, &[val]);
                }

                builder.switch_to_block(else_block);
                builder.seal_block(else_block);
                let else_val = Self::compile_expr(
                    else_branch,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                );
                if let Some(val) = else_val {
                    builder.ins().jump(merge_block, &[val]);
                }

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);

                if then_val.is_none() && else_val.is_none() {
                    None
                } else {
                    Some(builder.block_params(merge_block)[0])
                }
            }
            TypedExpr::Loop(body, loop_ty) => {
                let header_block = builder.create_block();
                let exit_block = builder.create_block();

                loop_stack.push((header_block, exit_block));

                let cl_type = loop_ty.into();
                builder.append_block_param(exit_block, cl_type);

                builder.ins().jump(header_block, &[]);
                builder.switch_to_block(header_block);

                let inner_val = Self::compile_expr(
                    body,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                );
                if inner_val.is_some() {
                    builder.ins().jump(header_block, &[]);
                }

                loop_stack.pop();
                builder.switch_to_block(exit_block);
                builder.seal_block(header_block);
                builder.seal_block(exit_block);

                Some(builder.block_params(exit_block)[0])
            }
            TypedExpr::Break(body) => {
                let loop_end = loop_stack.last().unwrap().1;
                let val = Self::compile_expr(
                    body,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;
                // dummy value to satisfy the type system
                builder.ins().jump(loop_end, &[val]);
                None
            }
            TypedExpr::Continue => {
                let loop_start = loop_stack.last().unwrap().0;
                builder.ins().jump(loop_start, &[]);
                None
            }
            TypedExpr::Block(exprs, _) => {
                let mut last_val = None;
                for e in exprs {
                    last_val = Self::compile_expr(
                        e,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    );
                    // if an expression diverged, don't compile the rest of the block (illegal)
                    if last_val.is_none() {
                        break;
                    }
                }
                last_val
            }
            TypedExpr::Call(name, args, ret_ty) => {
                let mut sig = module.make_signature();
                for arg in args {
                    let cl_type = (&arg.ty()).into();
                    sig.params.push(AbiParam::new(cl_type));
                }
                let cl_ret_type = ret_ty.into();
                sig.returns.push(AbiParam::new(cl_ret_type));

                // global module holds the function id
                let callee = module
                    .declare_function(name, Linkage::Import, &sig)
                    .expect("Function not found");
                // get the function into the local builder's context
                let local_callee = module.declare_func_in_func(callee, &mut builder.func);

                // eval the arguments to IR
                let mut arg_values = Vec::new();
                for arg in args {
                    arg_values.push(Self::compile_expr(
                        arg,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    )?);
                }

                // jump to fn
                let call_inst = builder.ins().call(local_callee, &arg_values);
                Some(builder.inst_results(call_inst)[0])
            }
            TypedExpr::FnDecl(..) => {
                // ignore inside compile_expr we process these at the top level
                Some(builder.ins().iconst(types::I64, 0))
            }
            TypedExpr::StructDecl(..) => {
                // requires no runtime execution
                Some(builder.ins().iconst(types::I64, 0))
            }
            TypedExpr::StructInit(name, fields) => {
                let layout = structs.get(name).unwrap();

                // allocate struct based on calculated bytes
                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size_bytes);
                let slot = builder.create_sized_stack_slot(slot_data);
                let base_ptr = builder.ins().stack_addr(types::I64, slot, 0);

                for (f_name, f_expr) in fields {
                    let val = Self::compile_expr(
                        f_expr,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    )?;

                    let (expected_ty, bit_offset) = layout.fields.get(f_name).unwrap();
                    let bits = bit_size_of(expected_ty);

                    let word_offset = (*bit_offset / 64) * 8;
                    let shift = *bit_offset % 64;

                    if expected_ty.is_integer_type() && bits < 64 {
                        let chunk = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            base_ptr,
                            word_offset as i32,
                        );

                        let raw_mask = 1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1);
                        let inverted_mask = !(raw_mask << shift);
                        let mask_val = builder.ins().iconst(types::I64, inverted_mask as i64);

                        let cleared = builder.ins().band(chunk, mask_val);

                        let shifted_val = builder.ins().ishl_imm(val, shift as i64);
                        let merged = builder.ins().bor(cleared, shifted_val);

                        builder
                            .ins()
                            .store(MemFlags::new(), merged, base_ptr, word_offset as i32);
                    } else {
                        builder
                            .ins()
                            .store(MemFlags::new(), val, base_ptr, word_offset as i32);
                    }
                }
                Some(base_ptr)
            }
            TypedExpr::FieldAccess(base, f_name, f_ty) => {
                // get the base and later return the memory pointer (base_ptr) of the struct
                let base_ptr = Self::compile_expr(
                    base,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                let Type::Custom(struct_name) = base.ty() else {
                    panic!("Fatal: Attempted field access on non-struct pointer");
                };

                let layout = structs.get(&struct_name).unwrap();
                let (_, bit_offset) = layout.fields.get(f_name).unwrap();

                let word_offset = (*bit_offset / 64) * 8;
                let shift = *bit_offset % 64;
                let bits = bit_size_of(f_ty);

                let extracted_val = if f_ty.is_integer_type() && bits < 64 {
                    // load the 64-bit chunk and shift the target bits down to zero
                    let chunk = builder.ins().load(
                        types::I64,
                        MemFlags::new(),
                        base_ptr,
                        word_offset as i32,
                    );
                    let shifted = builder.ins().ushr_imm(chunk, shift as i64);

                    // mask off the upper garbage bits
                    let raw_mask = 1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1);
                    let mask_val = builder.ins().iconst(types::I64, raw_mask as i64);
                    let mut isolated = builder.ins().band(shifted, mask_val);

                    // sign-extend if it's a signed integer
                    if let Type::Int(_) = f_ty {
                        let shift_up = 64 - bits;
                        isolated = builder.ins().ishl_imm(isolated, shift_up as i64);
                        isolated = builder.ins().sshr_imm(isolated, shift_up as i64);
                    }
                    isolated
                } else {
                    let cl_type = f_ty.into();
                    builder
                        .ins()
                        .load(cl_type, MemFlags::new(), base_ptr, word_offset as i32)
                };

                Some(extracted_val)
            }
            TypedExpr::ArrayInit(elements, _array_ty) => {
                let inner_ty = elements[0].ty();
                let (elem_size, align) = size_and_align_of(&inner_ty, structs, enums);
                let stride = align_to(elem_size, align);
                let total_size = stride * (elements.len() as u32);

                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, total_size);
                let slot = builder.create_sized_stack_slot(slot_data);
                let base_ptr = builder.ins().stack_addr(types::I64, slot, 0);

                for (i, elem_expr) in elements.iter().enumerate() {
                    let val = Self::compile_expr(
                        elem_expr,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    )?;
                    let offset = (i as i32) * (stride as i32);
                    builder.ins().store(MemFlags::new(), val, base_ptr, offset);
                }
                Some(base_ptr)
            }
            TypedExpr::Index(array_expr, index_expr, inner_ty) => {
                let base_ptr = Self::compile_expr(
                    array_expr,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                let mut index_val = Self::compile_expr(
                    index_expr,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                // if passed an i8 or i32 as the index, expand to i64
                match index_expr.ty() {
                    Type::Int(bits) => {
                        if bits < 64 {
                            index_val = builder.ins().sextend(types::I64, index_val);
                        } else if bits > 64 {
                            index_val = builder.ins().ireduce(types::I64, index_val);
                        }
                    }
                    Type::UInt(bits) => {
                        if bits < 64 {
                            index_val = builder.ins().uextend(types::I64, index_val);
                        } else if bits > 64 {
                            index_val = builder.ins().ireduce(types::I64, index_val);
                        }
                    }
                    _ => {}
                }

                let (elem_size, align) = size_and_align_of(inner_ty, structs, enums);
                let stride = align_to(elem_size, align);

                let offset_val = builder.ins().imul_imm(index_val, stride as i64);
                let target_ptr = builder.ins().iadd(base_ptr, offset_val);

                let cl_type = inner_ty.into();
                let val = builder.ins().load(cl_type, MemFlags::new(), target_ptr, 0);
                Some(val)
            }
            TypedExpr::EnumDecl(..) => Some(builder.ins().iconst(types::I64, 0)),
            TypedExpr::EnumInit(enum_name, variant_name, payloads) => {
                let layout = enums.get(enum_name).unwrap();
                let (tag, field_layouts) = layout.variants.get(variant_name).unwrap();

                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size_bytes);
                let slot = builder.create_sized_stack_slot(slot_data);
                let base_ptr = builder.ins().stack_addr(types::I64, slot, 0);

                let tag_val = builder.ins().iconst(types::I32, *tag as i64);
                builder.ins().store(MemFlags::new(), tag_val, base_ptr, 0);

                for (i, p) in payloads.iter().enumerate() {
                    let p_val = Self::compile_expr(
                        p,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    )?;
                    let (expected_ty, bit_offset) = &field_layouts[i];
                    let bits = crate::type_system::bit_size_of(expected_ty);

                    let word_offset = (*bit_offset / 64) * 8; // byte offset
                    let shift = *bit_offset % 64;

                    if expected_ty.is_integer_type() && bits < 64 {
                        let chunk = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            base_ptr,
                            word_offset as i32,
                        );
                        let raw_mask = 1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1);
                        let inverted_mask = !(raw_mask << shift);
                        let mask_val = builder.ins().iconst(types::I64, inverted_mask as i64);

                        let cleared = builder.ins().band(chunk, mask_val);
                        let shifted_val = builder.ins().ishl_imm(p_val, shift as i64);
                        let merged = builder.ins().bor(cleared, shifted_val);
                        builder
                            .ins()
                            .store(MemFlags::new(), merged, base_ptr, word_offset as i32);
                    } else if expected_ty.is_primitive() {
                        builder
                            .ins()
                            .store(MemFlags::new(), p_val, base_ptr, word_offset as i32);
                    } else {
                        let (p_size, _) = size_and_align_of(expected_ty, structs, enums);
                        let dest_ptr = builder.ins().iadd_imm(base_ptr, word_offset as i64);
                        emit_memory_copy(builder, p_val, dest_ptr, p_size);
                    }
                }
                Some(base_ptr)
            }
            TypedExpr::Match(target, arms, ret_ty) => {
                let target_ty = target.ty();
                let target_val = Self::compile_expr(
                    target,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                let base_ptr = if target_ty.is_integer_type()
                    || target_ty == Type::F64
                    || target_ty == Type::F32
                {
                    let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, 8);
                    let slot = builder.create_sized_stack_slot(slot_data);
                    let ptr = builder.ins().stack_addr(types::I64, slot, 0);
                    builder.ins().store(MemFlags::new(), target_val, ptr, 0);
                    ptr
                } else {
                    target_val // Arrays, Structs, and Enums are already memory pointers
                };

                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, ret_ty.into());

                let mut current_check_block = builder.create_block();
                builder.ins().jump(current_check_block, &[]);

                for (pat, body) in arms {
                    builder.switch_to_block(current_check_block);
                    builder.seal_block(current_check_block);

                    // block to jump to if the pattern fails
                    let next_check_block = builder.create_block();

                    // emit the recursive decision tree
                    Self::emit_pattern_match(
                        builder,
                        pat,
                        base_ptr,
                        next_check_block,
                        variables,
                        variable_index,
                        structs,
                        enums,
                    );

                    // if emit_pattern_match didn't jump to next_check_block
                    let body_val = Self::compile_expr(
                        body,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                        structs,
                        enums,
                    );

                    if let Some(v) = body_val {
                        builder.ins().jump(merge_block, &[v]);
                    }

                    // move to the fail block for the next iteration
                    current_check_block = next_check_block;
                }

                // when all arms fails, trap
                builder.switch_to_block(current_check_block);
                builder.seal_block(current_check_block);
                builder.ins().trap(TrapCode::UnreachableCodeReached);

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);

                Some(builder.block_params(merge_block)[0])
            }
            TypedExpr::Cast(expr, target_ty) => {
                let val = Self::compile_expr(
                    expr,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                let src_ty = expr.ty();

                Some(match (src_ty, target_ty) {
                    // Integer to Integer (Sign Extension)
                    (Type::Int(_), Type::Int(d)) | (Type::UInt(_), Type::Int(d)) => {
                        let shift_up = 64 - d;
                        let shifted = builder.ins().ishl_imm(val, shift_up as i64);
                        builder.ins().sshr_imm(shifted, shift_up as i64)
                    }
                    // Integer to Unsigned (Bitwise Truncation/Masking)
                    (Type::Int(_), Type::UInt(d)) | (Type::UInt(_), Type::UInt(d)) => {
                        let raw_mask = 1u64.checked_shl(*d as u32).unwrap_or(0).wrapping_sub(1);
                        let mask_val = builder.ins().iconst(types::I64, raw_mask as i64);
                        builder.ins().band(val, mask_val)
                    }

                    // Float to Integer (Truncates decimals)
                    (Type::F32 | Type::F64, Type::Int(_)) => {
                        builder.ins().fcvt_to_sint(types::I64, val)
                    }
                    (Type::F32 | Type::F64, Type::UInt(_)) => {
                        builder.ins().fcvt_to_uint(types::I64, val)
                    }

                    // Integer to Float (Adds decimal precision)
                    (Type::Int(_), Type::F32) => builder.ins().fcvt_from_sint(types::F32, val),
                    (Type::Int(_), Type::F64) => builder.ins().fcvt_from_sint(types::F64, val),
                    (Type::UInt(_), Type::F32) => builder.ins().fcvt_from_uint(types::F32, val),
                    (Type::UInt(_), Type::F64) => builder.ins().fcvt_from_uint(types::F64, val),

                    // Float to Float (Promote or Demote)
                    (Type::F32, Type::F64) => builder.ins().fpromote(types::F64, val),
                    (Type::F64, Type::F32) => builder.ins().fdemote(types::F32, val),

                    _ => val, // Fallback (No-Op)
                })
            }
        }
    }
}
