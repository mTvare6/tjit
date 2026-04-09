mod jit;
mod lexer;
mod parser;
mod type_system;

use jit::JITEngine;
use lexer::Lexer;
use parser::Parser;
use std::env;
use std::fs;
use type_system::TypeChecker;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: tjit <filename.tjit>");
        std::process::exit(1);
    }

    let filename = &args[1];
    let source = fs::read_to_string(filename).expect("Failed to read file");

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.collect_tokens();

    let mut parser = Parser::new(&tokens);
    let ast = parser.parse();

    let mut typechecker = TypeChecker::new();
    let ty_result = typechecker.check_program(&ast);
    let typed_ast = ty_result.unwrap_or_else(|e| panic!("{}", e));

    let mut jit = JITEngine::new();
    match jit.compile(&typed_ast, &typechecker.structs, &typechecker.enums) {
        Ok(jit_fn) => {
            let result = jit_fn();
            std::process::exit(result as i32)
        }
        Err(e) => println!("Compilation failed: {}", e),
    }
}
