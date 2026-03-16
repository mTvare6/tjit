use crate::parser::{Op, Type};
use crate::type_system::{EnumLayout, StructLayout, TypedExpr, align_to, size_and_align_of};
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

// for selecting arithmetic instructions
macro_rules! emit_binary_op {
    ($builder:expr, $ty:expr, $lhs:expr, $rhs:expr, $int_op:ident, $uint_op:ident, $float_op:ident) => {
        match $ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 => $builder.ins().$int_op($lhs, $rhs),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 => $builder.ins().$uint_op($lhs, $rhs),
            Type::F32 | Type::F64 => $builder.ins().$float_op($lhs, $rhs),
            Type::Custom(_) | Type::Array(..) | Type::Enum(..) => {
                panic!("Fatal: Cannot perform arithmetic operations on non numeric type")
            }
        }
    };
}

// for selecting relational conditional instructions
macro_rules! emit_cmp_op {
    ($builder:expr, $ty:expr, $lhs:expr, $rhs:expr, $int_cc:ident, $uint_cc:ident, $float_cc:ident) => {
        match $ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 => {
                let b = $builder.ins().icmp(IntCC::$int_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
                let b = $builder.ins().icmp(IntCC::$uint_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::F32 | Type::F64 => {
                let b = $builder.ins().fcmp(FloatCC::$float_cc, $lhs, $rhs);
                $builder.ins().uextend(types::I64, b)
            }
            Type::Custom(_) | Type::Array(..) | Type::Enum(..) => {
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
        let mut final_type = Type::I64;

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

        // ABI coercion, force the final value to match the `i64` return signature
        match final_type {
            Type::F32 | Type::F64 => {
                // convert float to signed integer
                return_val = builder.ins().fcvt_to_sint(types::I64, return_val);
            }
            Type::I8 | Type::I16 | Type::I32 => {
                // sign-extend smaller integers to 64-bit
                return_val = builder.ins().sextend(types::I64, return_val);
            }
            Type::U8 | Type::U16 | Type::U32 => {
                // zero-extend unsigned integers to 64-bit
                return_val = builder.ins().uextend(types::I64, return_val);
            }
            _ => {} // I64 and U64 require no width coercion
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

                // allocate a contiguous block on the call stack
                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size);
                let slot = builder.create_sized_stack_slot(slot_data);

                // get physical memory pointer to the start of the struct
                let base_ptr = builder.ins().stack_addr(types::I64, slot, 0);

                // store at the correct byte offset
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
                    let (_, offset) = layout.fields.get(f_name).unwrap();

                    builder.ins().store(MemFlags::new(), val, base_ptr, *offset);
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
                let (_, offset) = layout.fields.get(f_name).unwrap();

                // load directly from base_pointer + byte_offset
                let cl_type = f_ty.into();
                let val = builder
                    .ins()
                    .load(cl_type, MemFlags::new(), base_ptr, *offset);

                Some(val)
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
                    Type::I8 | Type::I16 | Type::I32 => {
                        index_val = builder.ins().sextend(types::I64, index_val);
                    }
                    Type::U8 | Type::U16 | Type::U32 => {
                        index_val = builder.ins().uextend(types::I64, index_val);
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

                let slot_data = StackSlotData::new(StackSlotKind::ExplicitSlot, layout.size);
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

                    let (_, offset) = field_layouts[i];
                    builder
                        .ins()
                        .store(MemFlags::new(), p_val, base_ptr, offset);
                }
                Some(base_ptr)
            }
            TypedExpr::Match(target, arms, ret_ty) => {
                // evaluate the target to get the base pointer
                let base_ptr = Self::compile_expr(
                    target,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                    structs,
                    enums,
                )?;

                // load the tag from byte offset 0
                let tag_val = builder.ins().load(types::I32, MemFlags::new(), base_ptr, 0);

                // setup the exit block for when a match arm finishes
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, ret_ty.into());

                let mut next_block = builder.create_block();

                builder.ins().jump(next_block, &[]);

                // build the cascading decision tree
                for (arm_enum, variant_name, bind_names, body) in arms {
                    let layout = enums.get(arm_enum).unwrap();
                    let (expected_tag, expected_payload_tys) =
                        layout.variants.get(variant_name).unwrap();

                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);

                    // check if RAM tag == expected tag
                    let expected_tag_val = builder.ins().iconst(types::I32, *expected_tag as i64);
                    let is_match = builder.ins().icmp(IntCC::Equal, tag_val, expected_tag_val);

                    let arm_block = builder.create_block();
                    next_block = builder.create_block(); // prepare the block for the next arm to check

                    // branch execution
                    builder
                        .ins()
                        .brif(is_match, arm_block, &[], next_block, &[]);

                    // inside the successful match arm
                    builder.switch_to_block(arm_block);
                    builder.seal_block(arm_block);

                    // If written `Some(val) =>`, bind `val` to the payload in RAM
                    let p_tys = expected_payload_tys;
                    for (i, b_name) in bind_names.iter().enumerate() {
                        let (p_ty, offset) = &expected_payload_tys[i];

                        let p_val =
                            builder
                                .ins()
                                .load(p_ty.into(), MemFlags::new(), base_ptr, *offset);

                        let var = Variable::new(*variable_index);
                        *variable_index += 1;
                        builder.declare_var(var, p_ty.into());
                        builder.def_var(var, p_val);
                        variables.insert(b_name.clone(), var);
                    }

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
                }

                // fallback if no arms match (rust ensures exhaustiveness at compile time, but trap in the JIT to be safe)
                builder.switch_to_block(next_block);
                builder.seal_block(next_block);
                builder.ins().trap(TrapCode::UnreachableCodeReached);

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);

                Some(builder.block_params(merge_block)[0])
            }
        }
    }
}
