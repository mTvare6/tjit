use crate::lexer::Token;

#[derive(Debug)]
pub enum Expr {
    Number(i64),
    BinaryOp(Box<Expr>, Op, Box<Expr>),
    Variable(String),
    Let(String, Box<Expr>),
}

#[derive(Debug)]
pub enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
}

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(token)
    }

    fn forward(&mut self) {
        self.pos += 1;
    }

    pub fn new(tokens: &'a [Token]) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        match self.next()? {
            Token::Number(n) => Some(Expr::Number(*n)),
            Token::Identifier(name) => Some(Expr::Variable(name.clone())),
            _ => None,
        }
    }

    fn parse_factor(&mut self) -> Option<Expr> {
        let mut left = self.parse_primary()?;

        while let Some(token) = self.peek() {
            match token {
                Token::Multiply | Token::Divide => {
                    let op = match token {
                        Token::Multiply => Op::Multiply,
                        Token::Divide => Op::Divide,
                        _ => unreachable!(),
                    };
                    self.forward();
                    let right = self.parse_primary()?;
                    left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
                }
                _ => break,
            }
        }

        Some(left)
    }

    fn parse_term(&mut self) -> Option<Expr> {
        let mut left = self.parse_factor()?;

        while let Some(token) = self.peek() {
            match token {
                Token::Plus | Token::Minus => {
                    let op = match token {
                        Token::Plus => Op::Add,
                        Token::Minus => Op::Subtract,
                        _ => unreachable!(),
                    };
                    self.forward();
                    let right = self.parse_factor()?;
                    left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
                }
                _ => break,
            }
        }

        Some(left)
    }

    pub fn parse_declaration(&mut self) -> Option<Expr> {
        let Some(Token::Let) = self.peek() else {
            return self.parse_term();
        };

        self.next();

        let name = match self.next() {
            Some(Token::Identifier(n)) => n.clone(),
            _ => panic!("Expected variable name after 'let'"),
        };

        match self.next() {
            Some(Token::Assign) => {}
            _ => panic!("Expected '=' after variable name"),
        }

        let value = self.parse_term()?;

        Some(Expr::Let(name, Box::new(value)))
    }

    pub fn parse(&mut self) -> Vec<Expr> {
        let mut program = Vec::new();

        loop {
            if self.peek().is_none_or(|t| *t == Token::EOF) {
                break;
            }

            if let Some(expr) = self.parse_declaration() {
                program.push(expr);
            } else {
                panic!("Failed to parse at token: {:?}", self.peek());
            }
        }

        program
    }
}
