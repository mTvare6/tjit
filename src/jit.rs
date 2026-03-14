use crate::parser::{Expr, Op};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use std::collections::HashMap;
use std::mem;

type LoopStack = Vec<(Block, Block)>;
type VariableMap = HashMap<String, Variable>;

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
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
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
        params: &[String],
        body: &Expr,
    ) -> Result<(), String> {
        self.module.clear_context(&mut self.ctx);

        for _ in params {
            self.ctx
                .func
                .signature
                .params
                .push(AbiParam::new(types::I64));
        }

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

        for (i, param_name) in params.iter().enumerate() {
            let val = builder.block_params(entry_block)[i];
            let var = Variable::new(variable_index);
            variable_index += 1;
            builder.declare_var(var, types::I64);
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

    pub fn compile(&mut self, program: &[Expr]) -> Result<fn() -> i64, String> {
        // global function compilation
        for expr in program {
            if let Expr::FnDecl(name, params, body) = expr {
                self.compile_function(name, params, body)?;
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

        for expr in program {
            // bypass function declarations during the local pass
            if matches!(expr, Expr::FnDecl(..)) {
                continue;
            }

            if let Some(val) = Self::compile_expr(
                expr,
                &mut self.module,
                &mut builder,
                &mut variables,
                &mut variable_index,
                &mut loop_stack,
            ) {
                return_val = val; // update return value to the last expression's value
            }
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
        expr: &Expr,
        module: &mut JITModule,
        builder: &mut FunctionBuilder,
        variables: &mut VariableMap,
        variable_index: &mut usize,
        loop_stack: &mut LoopStack,
    ) -> Option<Value> {
        match expr {
            Expr::Number(n) => Some(builder.ins().iconst(types::I64, *n)),
            Expr::BinaryOp(left, op, right) => {
                let lhs = Self::compile_expr(
                    left,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;
                let rhs = Self::compile_expr(
                    right,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;

                Some(match op {
                    Op::Add => builder.ins().iadd(lhs, rhs),
                    Op::Subtract => builder.ins().isub(lhs, rhs),
                    Op::Multiply => builder.ins().imul(lhs, rhs),
                    Op::Divide => builder.ins().sdiv(lhs, rhs),
                    Op::Eq => {
                        let bool_val = builder.ins().icmp(IntCC::Equal, lhs, rhs);
                        builder.ins().uextend(types::I64, bool_val)
                    }
                    Op::Lt => {
                        let bool_val = builder.ins().icmp(IntCC::SignedLessThan, lhs, rhs);
                        builder.ins().uextend(types::I64, bool_val)
                    }
                    Op::Le => {
                        let bool_val = builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs, rhs);
                        builder.ins().uextend(types::I64, bool_val)
                    }
                    Op::Gt => {
                        let bool_val = builder.ins().icmp(IntCC::SignedGreaterThan, lhs, rhs);
                        builder.ins().uextend(types::I64, bool_val)
                    }
                    Op::Ge => {
                        let bool_val =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs, rhs);
                        builder.ins().uextend(types::I64, bool_val)
                    }
                })
            }
            Expr::Variable(name) => Some(
                variables
                    .get(name)
                    .map(|var| builder.use_var(*var))
                    .unwrap_or_else(|| panic!("Undefined variable: {}", name)),
            ),
            Expr::Let(name, value) => {
                let val = Self::compile_expr(
                    value,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;
                let var = Variable::new(*variable_index);
                *variable_index += 1;
                builder.declare_var(var, types::I64);
                builder.def_var(var, val);
                variables.insert(name.clone(), var);
                Some(val) // return of let is the value assigned
            }
            Expr::Assign(name, value) => {
                let val = Self::compile_expr(
                    value,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;
                variables
                    .get(name)
                    .map(|var| builder.def_var(*var, val))
                    .unwrap_or_else(|| panic!("Undefined variable: {}", name));
                Some(val)
            }
            Expr::If(cond, then_branch, else_branch) => {
                let cond_val = Self::compile_expr(
                    cond,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;

                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let merge_block = builder.create_block();

                builder.append_block_param(merge_block, types::I64);

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
            Expr::Loop(body) => {
                let header_block = builder.create_block();
                let exit_block = builder.create_block();

                loop_stack.push((header_block, exit_block));

                builder.append_block_param(exit_block, types::I64);

                builder.ins().jump(header_block, &[]);
                builder.switch_to_block(header_block);

                let inner_val = Self::compile_expr(
                    body,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
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
            Expr::Break(body) => {
                let loop_end = match loop_stack.last() {
                    Some((_, end)) => *end,
                    None => panic!("'break' used outside of a loop"),
                };

                let val = Self::compile_expr(
                    body,
                    module,
                    builder,
                    variables,
                    variable_index,
                    loop_stack,
                )?;

                // dummy value to satisfy the type system
                builder.ins().jump(loop_end, &[val]);

                None
            }
            Expr::Continue => {
                if let Some((loop_start, _)) = loop_stack.last() {
                    builder.ins().jump(*loop_start, &[]);
                    None
                } else {
                    panic!("'continue' used outside of a loop");
                }
            }
            Expr::Block(exprs) => {
                let mut last_val = None;
                for e in exprs {
                    last_val = Self::compile_expr(
                        e,
                        module,
                        builder,
                        variables,
                        variable_index,
                        loop_stack,
                    );
                    // if an expression diverged, don't compile the rest of the block (illegal)
                    if last_val.is_none() {
                        break;
                    }
                }
                last_val
            }
            Expr::Call(name, args) => {
                let mut sig = module.make_signature();
                for _ in args {
                    sig.params.push(AbiParam::new(types::I64));
                }
                sig.returns.push(AbiParam::new(types::I64));

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
                    )?);
                }

                // jump to fn
                let call_inst = builder.ins().call(local_callee, &arg_values);
                Some(builder.inst_results(call_inst)[0])
            }
            Expr::FnDecl(_, _, _) => {
                // ignore inside compile_expr we process these at the top level
                Some(builder.ins().iconst(types::I64, 0))
            }
        }
    }
}
