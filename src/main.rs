mod lexer;
mod jit;
mod parser;

use jit::JITEngine;
use parser::Parser;
use lexer::Lexer;

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

    let mut jit = JITEngine::new();

    match jit.compile(&ast) {
        Ok(jit_fn) => {
            let result = jit_fn();
            println!("{}", result);
        }
        Err(e) => println!("Compilation failed: {}", e),
    }
}
