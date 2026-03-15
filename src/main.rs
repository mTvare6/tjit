mod jit;
mod lexer;
mod parser;
mod type_system;

use jit::JITEngine;
use lexer::Lexer;
use parser::Parser;
use type_system::TypeChecker;

// Read the entire source code from stdin until EOF with prompt
fn read_source() -> String {
    use std::io::{self, Write};

    let mut source = String::new();
    loop {
        print!(">>> ");
        io::stdout().flush().unwrap();

        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => source.push_str(&line),
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
    source
}

fn main() {
    let source = read_source();
    println!();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.collect_tokens();

    let mut parser = Parser::new(&tokens);
    let ast = parser.parse();

    // println!("{:#?}", ast);

    let mut typechecker = TypeChecker::new();
    let ty_result = typechecker.check_program(&ast);

    let typed_ast = ty_result.unwrap_or_else(|e| panic!("{}", e));

    // println!("{:#?}", typed_ast);

    let mut jit = JITEngine::new();

    match jit.compile(&typed_ast) {
        Ok(jit_fn) => {
            let result = jit_fn();
            println!("{}", result);
        }
        Err(e) => println!("Compilation failed: {}", e),
    }
}
