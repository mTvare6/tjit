use crate::parser::{Expr, Op, Type};
use std::collections::HashMap;

// typed intermediate representation (=MIR)
#[derive(Debug, Clone)]
pub enum TypedExpr {
    Number(i64, Type),
    Float(f64),
    Variable(String, Type),
    Let(String, Type, Box<TypedExpr>),
    Assign(String, Box<TypedExpr>, Type),
    BinaryOp(Box<TypedExpr>, Op, Box<TypedExpr>, Type),
    If(Box<TypedExpr>, Box<TypedExpr>, Box<TypedExpr>, Type),
    Loop(Box<TypedExpr>, Type),
    Break(Box<TypedExpr>),
    Continue,
    Block(Vec<TypedExpr>, Type),
    Call(String, Vec<TypedExpr>, Type),
    FnDecl(String, Vec<(String, Type)>, Type, Box<TypedExpr>),
}

impl TypedExpr {
    pub fn ty(&self) -> Type {
        match self {
            TypedExpr::Number(_, t) => *t,
            TypedExpr::Float(_) => Type::F64,
            TypedExpr::Variable(_, t) => *t,
            TypedExpr::Let(_, t, _) => *t,
            TypedExpr::Assign(_, _, t) => *t,
            TypedExpr::BinaryOp(_, _, _, t) => *t,
            TypedExpr::If(_, _, _, t) => *t,
            TypedExpr::Loop(_, t) => *t,
            TypedExpr::Break(e) => e.ty(),
            TypedExpr::Continue => Type::I64,
            TypedExpr::Block(_, t) => *t,
            TypedExpr::Call(_, _, t) => *t,
            TypedExpr::FnDecl(..) => Type::I64,
        }
    }
}

impl Type {
    fn is_integer_type(&self) -> bool {
        matches!(
            self,
            Type::I64
                | Type::I32
                | Type::I16
                | Type::I8
                | Type::U64
                | Type::U32
                | Type::U16
                | Type::U8
        )
    }
}

pub struct TypeChecker {
    variables: HashMap<String, Type>,
    functions: HashMap<String, (Vec<Type>, Type)>,
    loop_break_type: Option<Type>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            functions: HashMap::new(),
            loop_break_type: None,
        }
    }

    pub fn check_program(&mut self, program: &[Expr]) -> Result<Vec<TypedExpr>, String> {
        for expr in program {
            if let Expr::FnDecl(name, params, ret_type, _) = expr {
                let param_types = params.iter().map(|(_, t)| t.clone()).collect();
                self.functions
                    .insert(name.clone(), (param_types, ret_type.clone()));
            }
        }

        let mut typed_program = Vec::new();
        for expr in program {
            typed_program.push(self.check_expr(expr)?);
        }

        Ok(typed_program)
    }

    fn check_expr(&mut self, expr: &Expr) -> Result<TypedExpr, String> {
        match expr {
            Expr::Number(n) => Ok(TypedExpr::Number(*n, Type::I64)), // default until coerced
            Expr::Float(f) => Ok(TypedExpr::Float(*f)),
            Expr::Variable(name) => {
                let ty = self
                    .variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("Type Error: Undefined variable '{}'", name))?;
                Ok(TypedExpr::Variable(name.clone(), ty))
            }
            Expr::Let(name, declared_type, value) => {
                let mut typed_value = self.check_expr(value)?;

                // Literal type coercion
                if let TypedExpr::Number(n, _) = &mut typed_value {
                    if declared_type.is_integer_type() {
                        typed_value = TypedExpr::Number(*n, declared_type.clone());
                    }
                }

                if typed_value.ty() != *declared_type {
                    return Err(format!(
                        "Type Error: Mismatched types. Expected {:?}, found {:?}",
                        declared_type,
                        typed_value.ty()
                    ));
                }
                self.variables.insert(name.clone(), declared_type.clone());
                Ok(TypedExpr::Let(
                    name.clone(),
                    declared_type.clone(),
                    Box::new(typed_value),
                ))
            }
            Expr::Assign(name, value) => {
                let var_type = self
                    .variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("Type Error: Undefined variable '{}'", name))?;

                let mut typed_value = self.check_expr(value)?;

                // Literal type coercion
                if let TypedExpr::Number(n, _) = &mut typed_value {
                    if var_type.is_integer_type() {
                        typed_value = TypedExpr::Number(*n, var_type.clone());
                    }
                }

                if var_type != typed_value.ty() {
                    return Err(format!(
                        "Type Error: Cannot assign {:?} to variable of type {:?}",
                        typed_value.ty(),
                        var_type
                    ));
                }
                Ok(TypedExpr::Assign(
                    name.clone(),
                    Box::new(typed_value),
                    var_type,
                ))
            }
            Expr::BinaryOp(left, op, right) => {
                let t_left = self.check_expr(left)?;
                let t_right = self.check_expr(right)?;
                if t_left.ty() != t_right.ty() {
                    return Err(format!(
                        "Type Error: Binary operand mismatch. {:?} and {:?}",
                        t_left.ty(),
                        t_right.ty()
                    ));
                }

                let resolved_type = match op {
                    Op::Add | Op::Subtract | Op::Multiply | Op::Divide => t_left.ty(),
                    _ => Type::I64,
                };

                Ok(TypedExpr::BinaryOp(
                    Box::new(t_left),
                    *op,
                    Box::new(t_right),
                    resolved_type,
                ))
            }
            Expr::If(cond, then_branch, else_branch) => {
                let t_cond = self.check_expr(cond)?;
                if t_cond.ty() != Type::I64 {
                    return Err(format!(
                        "Type Error: If condition must evaluate to I64, found {:?}",
                        t_cond.ty()
                    ));
                }

                let t_then = self.check_expr(then_branch)?;
                let t_else = self.check_expr(else_branch)?;
                if t_then.ty() != t_else.ty() {
                    return Err(format!(
                        "Type Error: If branches must return identical types. Found {:?} and {:?}",
                        t_then.ty(),
                        t_else.ty()
                    ));
                }
                let ty = t_then.ty();
                Ok(TypedExpr::If(
                    Box::new(t_cond),
                    Box::new(t_then),
                    Box::new(t_else),
                    ty,
                ))
            }
            Expr::Loop(body) => {
                let previous_loop_type = self.loop_break_type.take();
                self.loop_break_type = Some(Type::I64); // default assumption, overwritten by break

                let t_body = self.check_expr(body)?;

                let resolved_type = self.loop_break_type.take().unwrap();
                self.loop_break_type = previous_loop_type;

                Ok(TypedExpr::Loop(Box::new(t_body), resolved_type))
            }
            Expr::Break(payload) => {
                let t_payload = self.check_expr(payload)?;
                if let Some(expected) = &self.loop_break_type {
                    if t_payload.ty() != *expected {
                        return Err(format!(
                            "Type Error: Break payload {:?} does not match expected loop type {:?}",
                            t_payload.ty(),
                            expected
                        ));
                    }
                } else {
                    return Err("Type Error: 'break' used outside of a loop".into());
                }
                Ok(TypedExpr::Break(Box::new(t_payload)))
            }
            Expr::Continue => {
                if self.loop_break_type.is_none() {
                    return Err("Type Error: 'continue' used outside of a loop".into());
                }
                Ok(TypedExpr::Continue)
            }
            Expr::Block(exprs) => {
                let mut t_exprs = Vec::new();
                let mut last_type = Type::I64;
                for e in exprs {
                    let t_e = self.check_expr(e)?;
                    last_type = t_e.ty();
                    t_exprs.push(t_e);
                }
                Ok(TypedExpr::Block(t_exprs, last_type))
            }
            Expr::Call(name, args) => {
                let (param_types, ret_type) = self
                    .functions
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("Type Error: Undefined function '{}'", name))?;

                if args.len() != param_types.len() {
                    return Err(format!(
                        "Type Error: '{}' expects {} args",
                        name,
                        param_types.len()
                    ));
                }

                let mut t_args = Vec::new();
                for (arg, expected_type) in args.iter().zip(param_types.iter()) {
                    let mut t_arg = self.check_expr(arg)?;

                    // Literal type coercion for arguments
                    if let TypedExpr::Number(n, _) = &mut t_arg {
                        if expected_type.is_integer_type() {
                            t_arg = TypedExpr::Number(*n, expected_type.clone());
                        }
                    }

                    if t_arg.ty() != *expected_type {
                        return Err(format!("Type Error: Argument mismatch in '{}'", name));
                    }
                    t_args.push(t_arg);
                }
                Ok(TypedExpr::Call(name.clone(), t_args, ret_type))
            }
            Expr::FnDecl(name, params, ret_type, body) => {
                let previous_vars = std::mem::take(&mut self.variables);

                for (param_name, param_type) in params {
                    self.variables
                        .insert(param_name.clone(), param_type.clone());
                }

                let t_body = self.check_expr(body)?;
                if t_body.ty() != *ret_type {
                    return Err(format!(
                        "Type Error: Function '{}' body evaluates to {:?}, but signature dictates {:?}",
                        name,
                        t_body.ty(),
                        ret_type
                    ));
                }

                self.variables = previous_vars;
                Ok(TypedExpr::FnDecl(
                    name.clone(),
                    params.clone(),
                    ret_type.clone(),
                    Box::new(t_body),
                ))
            }
        }
    }
}
