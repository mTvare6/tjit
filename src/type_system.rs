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
    StructDecl(String, Vec<(String, Type)>),
    StructInit(String, Vec<(String, Box<TypedExpr>)>),
    FieldAccess(Box<TypedExpr>, String, Type),
    ArrayInit(Vec<TypedExpr>, Type),
    Index(Box<TypedExpr>, Box<TypedExpr>, Type),
}

impl TypedExpr {
    pub fn ty(&self) -> Type {
        match self {
            TypedExpr::Variable(_, t)
            | TypedExpr::Let(_, t, _)
            | TypedExpr::Assign(_, _, t)
            | TypedExpr::BinaryOp(_, _, _, t)
            | TypedExpr::If(_, _, _, t)
            | TypedExpr::Loop(_, t)
            | TypedExpr::Number(_, t)
            | TypedExpr::Block(_, t)
            | TypedExpr::FieldAccess(_, _, t)
            | TypedExpr::ArrayInit(_, t)
            | TypedExpr::Index(_, _, t)
            | TypedExpr::Call(_, _, t) => t.clone(),
            TypedExpr::Break(e) => e.ty(),
            TypedExpr::Float(_) => Type::F64,
            TypedExpr::Continue => Type::I64,
            TypedExpr::FnDecl(..) => Type::I64,
            TypedExpr::StructDecl(..) => Type::I64,
            TypedExpr::StructInit(name, _) => Type::Custom(name.clone()),
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

#[derive(Clone)]
pub struct StructLayout {
    pub size: u32,
    pub fields: HashMap<String, (Type, i32)>, // maps field name to its (type, byte_offset)
}

pub struct TypeChecker {
    variables: HashMap<String, Type>,
    functions: HashMap<String, (Vec<Type>, Type)>,
    structs: HashMap<String, StructLayout>,
    loop_break_type: Option<Type>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert(String::from("print"), (vec![Type::I64], Type::I64));
        Self {
            variables: HashMap::new(),
            functions,
            structs: HashMap::new(),
            loop_break_type: None,
        }
    }

    pub fn check_program(&mut self, program: &[Expr]) -> Result<Vec<TypedExpr>, String> {
        // global signatures and memory layouts
        for expr in program {
            match expr {
                Expr::FnDecl(name, params, ret_type, _) => {
                    let param_types = params.iter().map(|(_, t)| t.clone()).collect();
                    self.functions
                        .insert(name.clone(), (param_types, ret_type.clone()));
                }
                Expr::StructDecl(name, fields) => {
                    let mut layout = HashMap::new();
                    let mut offset: i32 = 0;
                    for (f_name, f_ty) in fields {
                        layout.insert(f_name.clone(), (f_ty.clone(), offset));
                        offset += 8;
                    }
                    self.structs.insert(
                        name.clone(),
                        StructLayout {
                            size: offset as u32,
                            fields: layout,
                        },
                    );
                }
                _ => {}
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

                let resolved_type = match declared_type {
                    Some(expected_ty) => {
                        // literal type coercion against explicit annotation
                        if let TypedExpr::Number(n, _) = &mut typed_value {
                            if expected_ty.is_integer_type() {
                                typed_value = TypedExpr::Number(*n, expected_ty.clone());
                            }
                        }

                        if typed_value.ty() != *expected_ty {
                            return Err(format!(
                                "Type Error: Mismatched types. Expected {:?}, found {:?}",
                                expected_ty,
                                typed_value.ty()
                            ));
                        }
                        expected_ty.clone()
                    }
                    None => typed_value.ty(), // infer type dynamically from the evaluated expression
                };

                self.variables.insert(name.clone(), resolved_type.clone());

                Ok(TypedExpr::Let(
                    name.clone(),
                    resolved_type,
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

                // literal type coercion
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
                let mut t_left = self.check_expr(left)?;
                let mut t_right = self.check_expr(right)?;

                // cross-coercion, when one side is a raw literal and the other is a known expression,
                // mutate the literal to match the known type
                if let TypedExpr::Number(n, _) = &mut t_right {
                    if t_left.ty().is_integer_type() {
                        t_right = TypedExpr::Number(*n, t_left.ty());
                    }
                } else if let TypedExpr::Number(n, _) = &mut t_left {
                    if t_right.ty().is_integer_type() {
                        t_left = TypedExpr::Number(*n, t_right.ty());
                    }
                }

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

                    // literal type coercion for arguments
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
            Expr::StructDecl(name, fields) => {
                Ok(TypedExpr::StructDecl(name.clone(), fields.clone()))
            }
            Expr::StructInit(name, fields) => {
                let layout = self
                    .structs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("Type Error: Unknown struct '{}'", name))?;

                let mut t_fields = Vec::new();
                for (f_name, f_val) in fields {
                    let mut t_val = self.check_expr(f_val)?;

                    let (expected_ty, _) = layout.fields.get(f_name).ok_or_else(|| {
                        format!("Type Error: '{}' has no field '{}'", name, f_name)
                    })?;

                    // coerce literal numbers to match the struct definition
                    if let TypedExpr::Number(n, _) = &mut t_val {
                        if expected_ty.is_integer_type() {
                            t_val = TypedExpr::Number(*n, expected_ty.clone());
                        }
                    }

                    if t_val.ty() != *expected_ty {
                        return Err(format!(
                            "Type Error: Field '{}' expects {:?}, got {:?}",
                            f_name,
                            expected_ty,
                            t_val.ty()
                        ));
                    }
                    t_fields.push((f_name.clone(), Box::new(t_val)));
                }
                Ok(TypedExpr::StructInit(name.clone(), t_fields))
            }
            Expr::FieldAccess(base, field_name) => {
                let t_base = self.check_expr(base)?;

                // ensure the variable we are applying '.' to is actually a custom struct
                let Type::Custom(struct_name) = t_base.ty() else {
                    return Err(format!(
                        "Type Error: Cannot access field '{}' on primitive type {:?}",
                        field_name,
                        t_base.ty()
                    ));
                };

                let layout = self.structs.get(&struct_name).unwrap();
                let (f_ty, _offset) = layout.fields.get(field_name).ok_or_else(|| {
                    format!(
                        "Type Error: Struct '{}' has no field '{}'",
                        struct_name, field_name
                    )
                })?;

                Ok(TypedExpr::FieldAccess(
                    Box::new(t_base),
                    field_name.clone(),
                    f_ty.clone(),
                ))
            }
            Expr::ArrayInit(elements) => {
                if elements.is_empty() {
                    return Err(
                        "Type Error: Cannot infer type of empty array without explicit annotation"
                            .into(),
                    );
                }

                let mut t_elements = Vec::new();

                // establish the baseline type
                let first_elem = self.check_expr(&elements[0])?;
                let element_ty = first_elem.ty();
                t_elements.push(first_elem);

                // all subsequent elements shoudl be baseline type
                for e in elements.iter().skip(1) {
                    let mut t_e = self.check_expr(e)?;

                    // literal type coercion
                    if let TypedExpr::Number(n, _) = &mut t_e {
                        if element_ty.is_integer_type() {
                            t_e = TypedExpr::Number(*n, element_ty.clone());
                        }
                    }

                    if t_e.ty() != element_ty {
                        return Err(format!(
                            "Type Error: Array elements must be homogenous. Expected {:?}, got {:?}",
                            element_ty,
                            t_e.ty()
                        ));
                    }
                    t_elements.push(t_e);
                }

                let array_ty = Type::Array(Box::new(element_ty), elements.len());
                Ok(TypedExpr::ArrayInit(t_elements, array_ty))
            }

            Expr::Index(array_expr, index_expr) => {
                let t_array = self.check_expr(array_expr)?;
                let mut t_index = self.check_expr(index_expr)?;

                if let TypedExpr::Number(n, _) = &mut t_index {
                    t_index = TypedExpr::Number(*n, Type::I64);
                }

                if !t_index.ty().is_integer_type() {
                    return Err(format!(
                        "Type Error: Array index must be an integer, got {:?}",
                        t_index.ty()
                    ));
                }

                // base expression should be an array, get its inner type
                let inner_ty = match t_array.ty() {
                    Type::Array(inner, _) => *inner,
                    _ => {
                        return Err(format!(
                            "Type Error: Cannot index into non-array type {:?}",
                            t_array.ty()
                        ));
                    }
                };

                Ok(TypedExpr::Index(
                    Box::new(t_array),
                    Box::new(t_index),
                    inner_ty,
                ))
            }
        }
    }
}
