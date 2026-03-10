mod lexer;
mod parser;

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use std::mem;

use lexer::Lexer;
use parser::{Expr, Op, Parser};

pub struct JITEngine {
    builder_ctx: FunctionBuilderContext,
    ctx: codegen::Context,
    module: JITModule,
}

impl JITEngine {
    pub fn new() -> Self {
        // Setup architecture flags
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap(); // We are jiting, no position independent code needed

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

    pub fn compile(&mut self, expr: &Expr) -> Result<fn() -> i64, String> {
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

        let result_val = Self::compile_expr(expr, &mut builder);

        // emit the return instruction (the `}` of the function)
        builder.ins().return_(&[result_val]);
        builder.finalize();

        // define and finalize the machine code into RAM
        let id = self
            .module
            .declare_function("main", Linkage::Export, &self.ctx.func.signature)
            .map_err(|e| e.to_string())?;

        self.module
            .define_function(id, &mut self.ctx)
            .map_err(|e| e.to_string())?;

        // clear the context so we can reuse the engine for the next script
        self.module.clear_context(&mut self.ctx);
        self.module.finalize_definitions().unwrap();

        let code_ptr = self.module.get_finalized_function(id);

        // transmute it to a safe rust function signature
        unsafe { Ok(mem::transmute::<*const u8, fn() -> i64>(code_ptr)) }
    }

    fn compile_expr(expr: &Expr, builder: &mut FunctionBuilder) -> Value {
        match expr {
            Expr::Number(n) => builder.ins().iconst(types::I64, *n),
            Expr::BinaryOp(left, op, right) => {
                let lhs = Self::compile_expr(left, builder);
                let rhs = Self::compile_expr(right, builder);

                match op {
                    Op::Add => builder.ins().iadd(lhs, rhs),
                    Op::Subtract => builder.ins().isub(lhs, rhs),
                    Op::Multiply => builder.ins().imul(lhs, rhs),
                    Op::Divide => builder.ins().sdiv(lhs, rhs),
                }
            }
        }
    }
}

fn main() {
    let source = "3 + 4 * 2 - 4 / 2";
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.collect_tokens();
    let mut parser = Parser::new(&tokens);
    let ast = parser.parse().unwrap();

    let mut jit = JITEngine::new();

    match jit.compile(&ast) {
        Ok(jit_fn) => {
            let result = jit_fn();
            println!("{}", result);
        }
        Err(e) => println!("Compilation failed: {}", e),
    }
}
