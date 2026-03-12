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

    variables: VariableMap,
    variable_index: usize,

    loop_stack: LoopStack, // stack of (loop_start, loop_end) blocks for break/continue
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
            variables: HashMap::new(),
            variable_index: 0,
            loop_stack: Vec::new(),
        }
    }

    pub fn compile(&mut self, program: &[Expr]) -> Result<fn() -> i64, String> {
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

        let mut return_val = builder.ins().iconst(types::I64, 0); // default return value
        for expr in program {
            if let Some(val) = Self::compile_expr(
                expr,
                &mut builder,
                &mut self.variables,
                &mut self.variable_index,
                &mut self.loop_stack,
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

        let code_ptr = self.module.get_finalized_function(id);

        // transmute it to a safe rust function signature
        unsafe { Ok(mem::transmute::<*const u8, fn() -> i64>(code_ptr)) }
    }

    fn compile_expr(
        expr: &Expr,
        builder: &mut FunctionBuilder,
        variables: &mut VariableMap,
        variable_index: &mut usize,
        loop_stack: &mut LoopStack,
    ) -> Option<Value> {
        match expr {
            Expr::Number(n) => Some(builder.ins().iconst(types::I64, *n)),
            Expr::BinaryOp(left, op, right) => {
                let lhs = Self::compile_expr(left, builder, variables, variable_index, loop_stack)?;
                let rhs =
                    Self::compile_expr(right, builder, variables, variable_index, loop_stack)?;

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
                let val =
                    Self::compile_expr(value, builder, variables, variable_index, loop_stack)?;
                let var = Variable::new(*variable_index);
                *variable_index += 1;
                builder.declare_var(var, types::I64);
                builder.def_var(var, val);
                variables.insert(name.clone(), var);
                Some(val) // return of let is the value assigned
            }
            Expr::Assign(name, value) => {
                let val =
                    Self::compile_expr(value, builder, variables, variable_index, loop_stack)?;
                variables
                    .get(name)
                    .map(|var| builder.def_var(*var, val))
                    .unwrap_or_else(|| panic!("Undefined variable: {}", name));
                Some(val)
            }
            Expr::If(cond, then_branch, else_branch) => {
                let cond_val =
                    Self::compile_expr(cond, builder, variables, variable_index, loop_stack)?;

                let then_block = builder.create_block();
                let else_block = builder.create_block();
                let merge_block = builder.create_block();

                builder.append_block_param(merge_block, types::I64);

                builder
                    .ins()
                    .brif(cond_val, then_block, &[], else_block, &[]);

                builder.switch_to_block(then_block);
                builder.seal_block(then_block);
                let then_val =
                    Self::compile_expr(then_branch, builder, variables, variable_index, loop_stack);

                if let Some(val) = then_val {
                    builder.ins().jump(merge_block, &[val]);
                }

                builder.switch_to_block(else_block);
                builder.seal_block(else_block);
                let else_val =
                    Self::compile_expr(else_branch, builder, variables, variable_index, loop_stack);

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

                let inner_val =
                    Self::compile_expr(body, builder, variables, variable_index, loop_stack);

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

                let val = Self::compile_expr(body, builder, variables, variable_index, loop_stack)?;

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
        }
    }
}
