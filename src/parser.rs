use crate::lexer::Token;
use cranelift::prelude::types;

#[derive(Debug)]
pub enum Expr {
    Number(i64),
    Float(f64),
    BinaryOp(Box<Expr>, Op, Box<Expr>),
    Variable(String),
    Let(String, Option<Type>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Loop(Box<Expr>),
    Assign(String, Box<Expr>),
    Break(Box<Expr>),
    FnDecl(String, Vec<(String, Type)>, Type, Box<Expr>),
    Call(String, Vec<Expr>),
    Block(Vec<Expr>),
    Continue,
    StructDecl(String, Vec<(String, Type)>),
    StructInit(String, Vec<(String, Box<Expr>)>),
    FieldAccess(Box<Expr>, String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Custom(String),
}

impl Into<types::Type> for &Type {
    fn into(self) -> types::Type {
        match self {
            Type::I8 | Type::U8 => types::I8,
            Type::I16 | Type::U16 => types::I16,
            Type::I32 | Type::U32 => types::I32,
            Type::I64 | Type::U64 => types::I64,
            Type::F32 => types::F32,
            Type::F64 => types::F64,
            Type::Custom(..) => types::I64,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,

    Eq,
    Lt,
    Gt,
    Le,
    Ge,
}

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
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
        let expr = match self.peek()? {
            Token::Number(n) => {
                let val = *n;
                self.next();
                Some(Expr::Number(val))
            }
            Token::Float(f) => {
                let val = *f;
                self.next();
                Some(Expr::Float(val))
            }
            Token::Identifier(name) => {
                let var_name = name.clone();
                self.next();

                if self.peek() == Some(&Token::LParen) {
                    self.next();
                    let mut args = Vec::new();

                    if self.peek() != Some(&Token::RParen) {
                        loop {
                            args.push(
                                self.parse_expression()
                                    .expect("Expected argument expression"),
                            );
                            if self.peek() == Some(&Token::Comma) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                    }

                    let Some(Token::RParen) = self.next() else {
                        panic!("Expected ')' after arguments")
                    };
                    Some(Expr::Call(var_name, args))
                } else if self.peek() == Some(&Token::LBrace) {
                    self.next();
                    let mut fields = Vec::new();

                    if self.peek() != Some(&Token::RBrace) {
                        loop {
                            let field_name = match self.next() {
                                Some(Token::Identifier(n)) => n.clone(),
                                _ => panic!("Expected field name"),
                            };
                            let Some(Token::Colon) = self.next() else {
                                panic!("Expected ':'")
                            };

                            let field_value =
                                self.parse_expression().expect("Expected field value");
                            fields.push((field_name, Box::new(field_value)));

                            if self.peek() == Some(&Token::Comma) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                    }
                    let Some(Token::RBrace) = self.next() else {
                        panic!("Expected '}}'")
                    };
                    Some(Expr::StructInit(var_name, fields))
                } else {
                    Some(Expr::Variable(var_name))
                }
            }
            _ => None,
        };

        let mut expr = expr?;
        while self.peek() == Some(&Token::Dot) {
            self.next();

            let field_name = match self.next() {
                Some(Token::Identifier(name)) => name.clone(),
                _ => panic!("Expected field name after '.'"),
            };

            expr = Expr::FieldAccess(Box::new(expr), field_name);
        }

        Some(expr)
    }

    fn parse_type(&mut self) -> Type {
        if let Some(Token::Identifier(type_name)) = self.peek() {
            let ty = match type_name.as_str() {
                "i8" => Type::I8,
                "i16" => Type::I16,
                "i32" => Type::I32,
                "i64" => Type::I64,
                "u8" => Type::U8,
                "u16" => Type::U16,
                "u32" => Type::U32,
                "u64" => Type::U64,
                "f32" => Type::F32,
                "f64" => Type::F64,
                _ => Type::Custom(type_name.clone()),
            };
            self.next();
            ty
        } else {
            panic!("Expected type identifier");
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

    pub fn parse_var_decl(&mut self) -> Option<Expr> {
        self.next();

        let name = match self.next() {
            Some(Token::Identifier(n)) => n.clone(),
            _ => panic!("Expected variable name after 'let'"),
        };

        // check for optional type annotation
        let mut var_type = None;
        if self.peek() == Some(&Token::Colon) {
            self.next();
            var_type = Some(self.parse_type());
        }

        let Some(Token::Assign) = self.next() else {
            panic!("Expected '=' after variable name");
        };

        let value = self.parse_expression()?;

        Some(Expr::Let(name, var_type, Box::new(value)))
    }

    fn parse_struct_decl(&mut self) -> Option<Expr> {
        self.next();

        let name = match self.next() {
            Some(Token::Identifier(n)) => n.clone(),
            _ => panic!("Expected struct name"),
        };

        let Some(Token::LBrace) = self.next() else {
            panic!("Expected '{{' after struct name")
        };

        let mut fields = Vec::new();
        while self.peek() != Some(&Token::RBrace) {
            let field_name = match self.next() {
                Some(Token::Identifier(n)) => n.clone(),
                _ => panic!("Expected field name"),
            };

            let Some(Token::Colon) = self.next() else {
                panic!("Expected ':' after field name")
            };

            let field_type = self.parse_type();

            fields.push((field_name, field_type));

            if self.peek() == Some(&Token::Comma) {
                self.next();
            } else {
                break;
            }
        }

        let Some(Token::RBrace) = self.next() else {
            panic!("Expected '}}' at end of struct")
        };

        Some(Expr::StructDecl(name, fields))
    }

    pub fn parse_declaration(&mut self) -> Option<Expr> {
        match self.peek() {
            Some(Token::Fn) => self.parse_fn_decl(),
            Some(Token::Let) => self.parse_var_decl(),
            Some(Token::Struct) => self.parse_struct_decl(),
            _ => self.parse_expression(),
        }
    }

    fn parse_if(&mut self) -> Option<Expr> {
        self.next();
        let condition = self.parse_expression()?;
        let then_branch = self.parse_block()?;

        let Some(Token::Else) = self.next() else {
            panic!("Expected 'else' after then branch");
        };

        let else_branch = if self.peek() == Some(&Token::If) {
            self.parse_if()?
        } else {
            self.parse_block()?
        };

        Some(Expr::If(
            Box::new(condition),
            Box::new(then_branch),
            Box::new(else_branch),
        ))
    }

    fn parse_loop(&mut self) -> Option<Expr> {
        self.next();
        let body = self.parse_block()?;
        Some(Expr::Loop(Box::new(body)))
    }

    fn parse_relational(&mut self) -> Option<Expr> {
        let mut left = self.parse_term()?;

        while let Some(token) = self.peek() {
            match token {
                Token::Equal
                | Token::LessThan
                | Token::LessThanEqual
                | Token::GreaterThan
                | Token::GreaterThanEqual => {
                    let op = match token {
                        Token::Equal => Op::Eq,
                        Token::LessThan => Op::Lt,
                        Token::LessThanEqual => Op::Le,
                        Token::GreaterThan => Op::Gt,
                        Token::GreaterThanEqual => Op::Ge,
                        _ => unreachable!(),
                    };
                    self.forward();
                    let right = self.parse_term()?;
                    left = Expr::BinaryOp(Box::new(left), op, Box::new(right));
                }
                _ => break,
            }
        }

        Some(left)
    }

    fn parse_expression(&mut self) -> Option<Expr> {
        if let Some(Token::Identifier(name)) = self.peek() {
            if self.peek_next() == Some(&Token::Assign) {
                let var_name = name.clone();
                self.next();
                self.next();

                let value = self.parse_expression()?;
                return Some(Expr::Assign(var_name, Box::new(value)));
            }
        }
        match self.peek() {
            Some(Token::If) => self.parse_if(),
            Some(Token::Loop) => self.parse_loop(),
            Some(Token::Break) => self.parse_break(),
            Some(Token::Continue) => {
                self.next();
                Some(Expr::Continue)
            }
            _ => self.parse_relational(),
        }
    }

    fn parse_break(&mut self) -> Option<Expr> {
        self.next();

        let payload = self.parse_expression()?;

        Some(Expr::Break(Box::new(payload)))
    }

    fn parse_block(&mut self) -> Option<Expr> {
        let Some(Token::LBrace) = self.next() else {
            panic!("Expected '{{' to start block");
        };

        let mut exprs = Vec::new();
        while self.peek().is_some() && self.peek() != Some(&Token::RBrace) {
            if let Some(expr) = self.parse_declaration() {
                exprs.push(expr);
            } else {
                panic!("Failed to parse expression in block");
            }
        }

        let Some(Token::RBrace) = self.next() else {
            panic!("Expected '}}' to end block");
        };

        Some(Expr::Block(exprs))
    }

    fn parse_fn_decl(&mut self) -> Option<Expr> {
        self.next();

        let name = match self.next() {
            Some(Token::Identifier(n)) => n.clone(),
            _ => panic!("Expected function name"),
        };

        let Some(Token::LParen) = self.next() else {
            panic!("Expected '(' after function name")
        };

        let mut params = Vec::new();
        if self.peek() != Some(&Token::RParen) {
            loop {
                let param_name = match self.next() {
                    Some(Token::Identifier(p)) => p.clone(),
                    _ => panic!("Expected parameter name"),
                };

                let Some(Token::Colon) = self.next() else {
                    panic!("Expected ':' after parameter name");
                };

                let param_ty = self.parse_type();

                params.push((param_name, param_ty));

                if self.peek() == Some(&Token::Comma) {
                    self.next();
                } else {
                    break;
                }
            }
        }

        let Some(Token::RParen) = self.next() else {
            panic!("Expected ')' after parameters")
        };

        let Some(Token::Arrow) = self.next() else {
            panic!("Expected '->' after function signature")
        };

        let return_type = self.parse_type();

        let body = self.parse_block()?;

        Some(Expr::FnDecl(name, params, return_type, Box::new(body)))
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
