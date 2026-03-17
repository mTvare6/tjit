use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    // Data
    Number(i64),
    Float(f64),
    Identifier(String),

    // Operators
    Plus,
    Minus,
    Multiply,
    Divide,
    Assign,
    Dot,

    // Keywords
    Let,
    If,
    Else,
    Loop,
    Break,
    Continue,
    Fn,
    Struct,
    Enum,
    Match,

    // Relational
    LessThan,
    GreaterThan,
    LessThanEqual,
    GreaterThanEqual,
    Equal,

    // Control
    EOF,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    DoubleColon,
    Arrow,
    FatArrow,
    DotDot,
    DotDotEqual,
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
        let mut number_str = String::new();
        let mut is_float = false;

        while let Some(&ch) = self.chars.peek() {
            if ch.is_digit(10) {
                number_str.push(ch);
                self.chars.next();
            } else if ch == '.' && !is_float {
                let mut lookahead = self.chars.clone();
                lookahead.next();
                if lookahead.peek() == Some(&'.') {
                    break;
                }

                is_float = true;
                number_str.push(ch);
                self.chars.next();
            } else {
                break;
            }
        }

        if is_float {
            Token::Float(number_str.parse::<f64>().unwrap())
        } else {
            Token::Number(number_str.parse::<i64>().unwrap())
        }
    }

    fn read_ident(&mut self) -> String {
        let mut word = String::new();
        while let Some(&ch) = self.chars.peek() {
            if ch.is_alphanumeric() || ch == '_' {
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
            "if" => Token::If,
            "else" => Token::Else,
            "continue" => Token::Continue,
            "break" => Token::Break,
            "loop" => Token::Loop,
            "fn" => Token::Fn,
            "struct" => Token::Struct,
            "enum" => Token::Enum,
            "match" => Token::Match,
            _ => Token::Identifier(ident),
        }
    }

    pub fn next_token(&mut self) -> Token {
        while let Some(&ch) = self.chars.peek() {
            match ch {
                ' ' | '\t' | '\n' => {
                    self.chars.next();
                    continue;
                }
                '+' => {
                    self.chars.next();
                    return Token::Plus;
                }
                '-' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&'>') {
                        self.chars.next();
                        return Token::Arrow;
                    }
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
                ',' => {
                    self.chars.next();
                    return Token::Comma;
                }
                ':' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&':') {
                        self.chars.next();
                        return Token::DoubleColon;
                    }
                    return Token::Colon;
                }
                ';' => {
                    self.chars.next();
                    return Token::Semicolon;
                }
                '.' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&'.') {
                        self.chars.next();
                        if self.chars.peek() == Some(&'=') {
                            self.chars.next();
                            return Token::DotDotEqual;
                        }
                        return Token::DotDot;
                    }
                    return Token::Dot;
                }
                '=' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        return Token::Equal;
                    }
                    if self.chars.peek() == Some(&'>') {
                        self.chars.next();
                        return Token::FatArrow;
                    }
                    return Token::Assign;
                }
                '<' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        return Token::LessThanEqual;
                    }
                    return Token::LessThan;
                }
                '>' => {
                    self.chars.next();
                    if self.chars.peek() == Some(&'=') {
                        self.chars.next();
                        return Token::GreaterThanEqual;
                    }
                    return Token::GreaterThan;
                }
                '0'..='9' => return self.lex_number(),
                'a'..='z' | 'A'..='Z' | '_' => return self.lex_identifier(),
                '(' => {
                    self.chars.next();
                    return Token::LParen;
                }
                ')' => {
                    self.chars.next();
                    return Token::RParen;
                }
                '{' => {
                    self.chars.next();
                    return Token::LBrace;
                }
                '}' => {
                    self.chars.next();
                    return Token::RBrace;
                }
                ']' => {
                    self.chars.next();
                    return Token::RBracket;
                }
                '[' => {
                    self.chars.next();
                    return Token::LBracket;
                }
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
