use crate::parser::Pat;
use crate::parser::{Expr, Op, Type};
use std::collections::HashMap;

pub fn align_to(offset: u32, align: u32) -> u32 {
    (offset + align - 1) & !(align - 1)
}

pub fn size_and_align_of(
    ty: &Type,
    structs: &HashMap<String, StructLayout>,
    enums: &HashMap<String, EnumLayout>,
) -> (u32, u32) {
    match ty {
        Type::Int(bits) | Type::UInt(bits) => {
            let bytes = (bits + 7) / 8;
            let align = if bytes <= 1 {
                1
            } else if bytes <= 2 {
                2
            } else if bytes <= 4 {
                4
            } else {
                8
            };
            (bytes as u32, align as u32)
        }
        Type::F32 => (4, 4),
        Type::F64 => (8, 8),
        Type::String => (16, 8),
        Type::Array(inner, len) => {
            let (elem_size, align) = size_and_align_of(inner, structs, enums);
            let stride = align_to(elem_size, align);
            (stride * (*len as u32), align)
        }
        Type::Custom(name) | Type::Enum(name) => {
            if let Some(l) = structs.get(name) {
                (l.size, l.align)
            } else if let Some(l) = enums.get(name) {
                (l.size, l.align)
            } else {
                panic!("Fatal: Type '{}' not resolved during layout phase", name);
            }
        }
    }
}

// typed intermediate representation (=MIR)
#[allow(dead_code)]
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
    EnumDecl(String, Vec<(String, Vec<Type>)>),
    EnumInit(String, String, Vec<TypedExpr>),
    Match(Box<TypedExpr>, Vec<(TypedPat, Box<TypedExpr>)>, Type),
    StringLiteral(String),
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
            | TypedExpr::Match(_, _, t)
            | TypedExpr::Call(_, _, t) => t.clone(),
            TypedExpr::Break(e) => e.ty(),
            TypedExpr::Float(_) => Type::F64,
            TypedExpr::EnumInit(name, _, _) => Type::Custom(name.clone()),
            TypedExpr::Continue => Type::Int(64),
            TypedExpr::FnDecl(..) => Type::Int(64),
            TypedExpr::EnumDecl(..) => Type::Int(64),
            TypedExpr::StructDecl(..) => Type::Int(64),
            TypedExpr::StructInit(name, _) => Type::Custom(name.clone()),
            TypedExpr::StringLiteral(_) => Type::String,
        }
    }
}

impl Type {
    pub fn is_integer_type(&self) -> bool {
        matches!(self, Type::Int(_) | Type::UInt(_))
    }

    pub fn is_primitive(&self) -> bool {
        matches!(self, Type::Int(_) | Type::UInt(_) | Type::F32 | Type::F64)
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TypedPat {
    Wildcard(Type),
    Number(i64, Type),
    Range(i64, i64, bool, Type),
    Variable(String, Type),
    Struct(String, Vec<(String, TypedPat)>, Type),
    Enum(String, String, Vec<TypedPat>, Type),
}

#[allow(dead_code)]
impl TypedPat {
    pub fn ty(&self) -> Type {
        match self {
            TypedPat::Wildcard(t)
            | TypedPat::Number(_, t)
            | TypedPat::Range(_, _, _, t)
            | TypedPat::Variable(_, t)
            | TypedPat::Struct(_, _, t)
            | TypedPat::Enum(_, _, _, t) => t.clone(),
        }
    }
}

#[derive(Clone)]
pub struct StructLayout {
    pub size: u32,
    pub align: u32,
    pub fields: HashMap<String, (Type, i32)>, // maps field name to its (type, byte_offset)
}

#[derive(Clone)]
pub struct EnumLayout {
    pub size: u32,
    pub align: u32,
    pub variants: HashMap<String, (u32, Vec<(Type, i32)>)>, // maps variant name to (integer tag, payload types + offsets)
}

pub struct TypeChecker {
    variables: HashMap<String, Type>,
    functions: HashMap<String, (Vec<Type>, Type)>,
    pub structs: HashMap<String, StructLayout>,
    pub enums: HashMap<String, EnumLayout>,
    loop_break_type: Option<Type>,
    pending_structs: HashMap<String, Vec<(String, Type)>>,
    pending_enums: HashMap<String, Vec<(String, Vec<Type>)>>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert(String::from("print"), (vec![Type::Int(64)], Type::Int(64)));
        functions.insert(
            String::from("print_str"),
            (vec![Type::String], Type::Int(64)),
        );
        Self {
            variables: HashMap::new(),
            functions,
            structs: HashMap::new(),
            enums: HashMap::new(),
            loop_break_type: None,
            pending_structs: HashMap::new(),
            pending_enums: HashMap::new(),
        }
    }

    pub fn check_program(&mut self, program: &[Expr]) -> Result<Vec<TypedExpr>, String> {
        for expr in program {
            match expr {
                Expr::FnDecl(name, params, ret_type, _) => {
                    let param_types = params.iter().map(|(_, t)| t.clone()).collect();
                    self.functions
                        .insert(name.clone(), (param_types, ret_type.clone()));
                }
                Expr::StructDecl(name, fields) => {
                    self.pending_structs.insert(name.clone(), fields.clone());
                }
                Expr::EnumDecl(name, variants) => {
                    self.pending_enums.insert(name.clone(), variants.clone());
                }
                _ => {}
            }
        }

        // recursively resolve layouts
        let struct_names: Vec<String> = self.pending_structs.keys().cloned().collect();
        for name in struct_names {
            self.resolve_layout(&name);
        }

        let enum_names: Vec<String> = self.pending_enums.keys().cloned().collect();
        for name in enum_names {
            self.resolve_layout(&name);
        }

        // typecheck expressions
        let mut typed_program = Vec::new();
        for expr in program {
            typed_program.push(self.check_expr(expr)?);
        }

        Ok(typed_program)
    }

    fn check_pattern(&mut self, pat: &Pat, expected_ty: &Type) -> Result<TypedPat, String> {
        match pat {
            Pat::Wildcard => Ok(TypedPat::Wildcard(expected_ty.clone())),
            Pat::Number(n) => {
                if !expected_ty.is_integer_type() {
                    return Err(format!(
                        "Type Error: Cannot match number {} against type {:?}",
                        n, expected_ty
                    ));
                }
                Ok(TypedPat::Number(*n, expected_ty.clone()))
            }
            Pat::Range(start, end, inclusive) => {
                if !expected_ty.is_integer_type() {
                    return Err(format!(
                        "Type Error: Cannot match range against type {:?}",
                        expected_ty
                    ));
                }
                Ok(TypedPat::Range(
                    *start,
                    *end,
                    *inclusive,
                    expected_ty.clone(),
                ))
            }
            Pat::Variable(name) => {
                // if it's a variable, bind it to the current scope
                self.variables.insert(name.clone(), expected_ty.clone());
                Ok(TypedPat::Variable(name.clone(), expected_ty.clone()))
            }
            Pat::Struct(name, fields) => {
                let Type::Custom(target_name) = expected_ty else {
                    return Err(format!(
                        "Type Error: Expected {:?}, but pattern is Struct '{}'",
                        expected_ty, name
                    ));
                };
                if name != target_name {
                    return Err(format!(
                        "Type Error: Mismatched structs in pattern. Expected '{}', got '{}'",
                        target_name, name
                    ));
                }

                let layout = self.structs.get(name).unwrap().clone();
                let mut t_fields = Vec::new();

                for (f_name, f_pat) in fields {
                    let (f_ty, _) = layout.fields.get(f_name).ok_or_else(|| {
                        format!("Type Error: Struct '{}' has no field '{}'", name, f_name)
                    })?;

                    // validate the nested pattern against the field's type
                    let t_f_pat = self.check_pattern(f_pat, f_ty)?;
                    t_fields.push((f_name.clone(), t_f_pat));
                }
                Ok(TypedPat::Struct(
                    name.clone(),
                    t_fields,
                    expected_ty.clone(),
                ))
            }
            Pat::Enum(enum_name, variant_name, payloads) => {
                let Type::Custom(target_name) = expected_ty else {
                    return Err(format!(
                        "Type Error: Expected {:?}, but pattern is Enum '{}'",
                        expected_ty, enum_name
                    ));
                };
                if enum_name != target_name {
                    return Err(format!(
                        "Type Error: Mismatched enums in pattern. Expected '{}', got '{}'",
                        target_name, enum_name
                    ));
                }

                let layout = self.enums.get(enum_name).unwrap().clone();
                let (_, expected_payload_tys) =
                    layout.variants.get(variant_name).ok_or_else(|| {
                        format!(
                            "Type Error: Enum '{}' has no variant '{}'",
                            enum_name, variant_name
                        )
                    })?;

                if payloads.len() != expected_payload_tys.len() {
                    return Err(format!(
                        "Type Error: Variant '{}::{}' expects {} payloads, pattern provided {}",
                        enum_name,
                        variant_name,
                        expected_payload_tys.len(),
                        payloads.len()
                    ));
                }

                let mut t_payloads = Vec::new();
                for (p, (expected_p_ty, _)) in payloads.iter().zip(expected_payload_tys.iter()) {
                    // validate the nested payload pattern
                    let t_p = self.check_pattern(p, expected_p_ty)?;
                    t_payloads.push(t_p);
                }

                Ok(TypedPat::Enum(
                    enum_name.clone(),
                    variant_name.clone(),
                    t_payloads,
                    expected_ty.clone(),
                ))
            }
        }
    }

    // topological resolver
    fn resolve_layout(&mut self, name: &str) {
        if self.structs.contains_key(name) || self.enums.contains_key(name) {
            return;
        }

        if let Some(fields) = self.pending_structs.clone().get(name) {
            let mut layout = HashMap::new();
            let mut offset = 0;
            let mut max_align = 1;

            for (f_name, f_ty) in fields {
                if let Type::Custom(c_name) = f_ty {
                    self.resolve_layout(c_name);
                } // Recursion!

                let (f_size, f_align) = size_and_align_of(f_ty, &self.structs, &self.enums);
                offset = align_to(offset, f_align);
                layout.insert(f_name.clone(), (f_ty.clone(), offset as i32));
                offset += f_size;
                if f_align > max_align {
                    max_align = f_align;
                }
            }

            let size = align_to(offset, max_align);
            self.structs.insert(
                name.to_string(),
                StructLayout {
                    size,
                    align: max_align,
                    fields: layout,
                },
            );
        } else if let Some(variants) = self.pending_enums.clone().get(name) {
            let mut layout = HashMap::new();
            let mut max_size = 4; // at minimum, 4 bytes for the i32 tag
            let mut max_align = 4;
            let mut tag_counter = 0;

            for (v_name, v_tys) in variants {
                let mut payload_offset = 4; // tag lives at byte 0
                let mut variant_align = 4;
                let mut field_layouts = Vec::new();

                for f_ty in v_tys {
                    if let Type::Custom(c_name) = f_ty {
                        self.resolve_layout(c_name);
                    } // recursion

                    let (f_size, f_align) = size_and_align_of(f_ty, &self.structs, &self.enums);
                    payload_offset = align_to(payload_offset, f_align);
                    field_layouts.push((f_ty.clone(), payload_offset as i32));
                    payload_offset += f_size;
                    if f_align > variant_align {
                        variant_align = f_align;
                    }
                }

                if payload_offset > max_size {
                    max_size = payload_offset;
                }
                if variant_align > max_align {
                    max_align = variant_align;
                }

                layout.insert(v_name.clone(), (tag_counter, field_layouts));
                tag_counter += 1;
            }

            let size = align_to(max_size, max_align);
            self.enums.insert(
                name.to_string(),
                EnumLayout {
                    size,
                    align: max_align,
                    variants: layout,
                },
            );
        } else {
            panic!("Type Error: Undefined custom type '{}'", name);
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> Result<TypedExpr, String> {
        match expr {
            Expr::StringLiteral(s) => Ok(TypedExpr::StringLiteral(s.clone())),
            Expr::Number(n) => Ok(TypedExpr::Number(*n, Type::Int(64))), // default until coerced
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
                    _ => Type::Int(64),
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
                if t_cond.ty() != Type::Int(64) {
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
                self.loop_break_type = Some(Type::Int(64)); // default assumption, overwritten by break

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
                let mut last_type = Type::Int(64);
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
                    t_index = TypedExpr::Number(*n, Type::Int(64));
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
            Expr::EnumDecl(name, variants) => {
                Ok(TypedExpr::EnumDecl(name.clone(), variants.clone()))
            }
            Expr::EnumInit(enum_name, variant_name, payloads) => {
                let expected_payload_tys = {
                    let layout = self
                        .enums
                        .get(enum_name)
                        .ok_or_else(|| format!("Type Error: Unknown enum '{}'", enum_name))?;
                    let (_, expected_tys) = layout.variants.get(variant_name).ok_or_else(|| {
                        format!(
                            "Type Error: Enum '{}' has no variant '{}'",
                            enum_name, variant_name
                        )
                    })?;
                    expected_tys.clone()
                };

                if payloads.len() != expected_payload_tys.len() {
                    return Err(format!(
                        "Type Error: Variant '{}::{}' expects {} payloads, got {}",
                        enum_name,
                        variant_name,
                        expected_payload_tys.len(),
                        payloads.len()
                    ));
                }

                let mut t_payloads = Vec::new();
                for (p, (expected_ty, _offset)) in payloads.iter().zip(expected_payload_tys.iter())
                {
                    let mut eval_p = self.check_expr(p)?;

                    // literal coercion
                    if let TypedExpr::Number(n, _) = &mut eval_p {
                        if expected_ty.is_integer_type() {
                            eval_p = TypedExpr::Number(*n, expected_ty.clone());
                        }
                    }

                    if eval_p.ty() != *expected_ty {
                        return Err(format!(
                            "Type Error: Variant '{}::{}' expects payload of type {:?}, got {:?}",
                            enum_name,
                            variant_name,
                            expected_ty,
                            eval_p.ty()
                        ));
                    }
                    t_payloads.push(eval_p);
                }

                Ok(TypedExpr::EnumInit(
                    enum_name.clone(),
                    variant_name.clone(),
                    t_payloads,
                ))
            }
            Expr::Match(target, arms) => {
                let t_target = self.check_expr(target)?;
                let target_ty = t_target.ty();

                let mut t_arms = Vec::new();
                let mut return_ty = None;

                for (pat, body) in arms {
                    // isolate the scope for each match arm
                    let previous_vars = std::mem::take(&mut self.variables);
                    self.variables = previous_vars.clone();

                    // validate the pattern and automatically inject bindings
                    let t_pat = self.check_pattern(pat, &target_ty)?;

                    // evaluate the body with the newly bound variables
                    let t_body = self.check_expr(body)?;

                    // ensure all arms return the exact same type
                    if let Some(r_ty) = &return_ty {
                        if *r_ty != t_body.ty() {
                            return Err(format!(
                                "Type Error: Match arms have incompatible return types: {:?} and {:?}",
                                r_ty,
                                t_body.ty()
                            ));
                        }
                    } else {
                        return_ty = Some(t_body.ty());
                    }

                    t_arms.push((t_pat, Box::new(t_body)));

                    // restore the scope so variables don't leak into the next arm
                    self.variables = previous_vars;
                }

                Ok(TypedExpr::Match(
                    Box::new(t_target),
                    t_arms,
                    return_ty.unwrap_or(Type::Int(64)),
                ))
            }
        }
    }
}
