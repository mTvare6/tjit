use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    // Data
    Number(i64),
    Identifier(String),

    // Operators
    Plus,
    Minus,
    Multiply,
    Divide,
    Assign,

    // Keywords
    Let,

    // Control
    EOF,
}

pub struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Lexer {
            chars: source.chars().peekable(),
        }
    }

    fn lex_number(&mut self) -> Token {
        let mut number = 0;
        while let Some(&ch) = self.chars.peek() {
            if ch.is_digit(10) {
                number = number * 10 + ch.to_digit(10).unwrap() as i64;
                self.chars.next();
            } else {
                break;
            }
        }
        Token::Number(number)
    }

    fn read_ident(&mut self) -> String {
        let mut word = String::new();
        while let Some(&ch) = self.chars.peek() {
            if ch.is_alphanumeric() {
                word.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }
        word
    }
    fn lex_identifier(&mut self) -> Token {
        let ident = self.read_ident();
        match ident.as_str() {
            "let" => Token::Let,
            _ => Token::Identifier(ident),
        }
    }

    pub fn next_token(&mut self) -> Token {
        while let Some(&ch) = self.chars.peek() {
            match ch {
                ' ' | '\t' | '\n' | ';' => {
                    self.chars.next();
                    continue;
                }
                '+' => {
                    self.chars.next();
                    return Token::Plus;
                }
                '-' => {
                    self.chars.next();
                    return Token::Minus;
                }
                '*' => {
                    self.chars.next();
                    return Token::Multiply;
                }
                '/' => {
                    self.chars.next();
                    return Token::Divide;
                }
                '=' => {
                    self.chars.next();
                    return Token::Assign;
                }
                '0'..='9' => return self.lex_number(),
                'a'..='z' | 'A'..='Z' => return self.lex_identifier(),
                _ => panic!("Unexpected character: {}", ch),
            }
        }
        Token::EOF
    }

    pub fn collect_tokens(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token();
            if token == Token::EOF {
                break;
            }
            tokens.push(token);
        }
        tokens
    }
}
