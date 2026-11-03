//! Post-order walkers shared by explicit capture collection and closure conversion.
use crate::ast::*;
pub(crate) trait Visitor {
    fn expression(&mut self, _expression: &mut Expression) {}
    fn ty(&mut self, _ty: &mut TypeRef) {}
    fn enter_function(&mut self, _function: &FunctionDecl) {}
    fn leave_function(&mut self, _function: &FunctionDecl) {}
    fn enter_block(&mut self, _block: &Block) {}
    fn leave_block(&mut self, _block: &Block) {}
    fn enter_statement(&mut self, _statement: &Statement) {}
    fn leave_statement(&mut self, _statement: &Statement) {}
}
pub(crate) fn module(module: &mut Module, visitor: &mut impl Visitor) {
    for item in &mut module.declarations {
        declaration(item, visitor);
    }
}
pub(crate) fn declaration(declaration: &mut Declaration, visitor: &mut impl Visitor) {
    match declaration {
        Declaration::Function(f) => {
            visitor.enter_function(f);
            for p in &mut f.parameters {
                ty(&mut p.ty, visitor);
            }
            ty(&mut f.result, visitor);
            block(&mut f.body, visitor);
            if let Some(e) = &mut f.fallback {
                expression(e, visitor);
            }
            visitor.leave_function(f);
        }
        Declaration::Record(r) => {
            for f in &mut r.fields {
                ty(&mut f.ty, visitor);
            }
        }
        Declaration::Enum(e) => {
            for v in &mut e.variants {
                for f in &mut v.fields {
                    ty(&mut f.ty, visitor);
                }
            }
        }
        Declaration::Error(error) => {
            if let Some(payload) = &mut error.payload {
                ty(&mut payload.ty, visitor);
            }
        }
        Declaration::Constant(c) => {
            if let Some(t) = &mut c.annotation {
                ty(t, visitor);
            }
            expression(&mut c.initializer, visitor);
        }
        Declaration::TypeAlias(alias) => ty(&mut alias.ty, visitor),
    }
}
pub(crate) fn ty(t: &mut TypeRef, v: &mut impl Visitor) {
    match &mut t.kind {
        TypeRefKind::Record(fields) => {
            for field in fields {
                ty(&mut field.ty, v);
            }
        }
        TypeRefKind::Generic { arguments, .. } => {
            for a in arguments {
                ty(a, v);
            }
        }
        TypeRefKind::List(t) | TypeRefKind::Future(t) => ty(t, v),
        TypeRefKind::Map(k, val) => {
            ty(k, v);
            ty(val, v);
        }
        TypeRefKind::Function {
            parameters, result, ..
        } => {
            for p in parameters {
                ty(p, v);
            }
            ty(result, v);
        }
        TypeRefKind::Bytes(e) => expression(e, v),
        TypeRefKind::Named(_) | TypeRefKind::ConcreteBytes(_) => {}
    }
    v.ty(t);
}
pub(crate) fn block(b: &mut Block, v: &mut impl Visitor) {
    v.enter_block(b);
    for s in &mut b.statements {
        statement(s, v);
    }
    v.leave_block(b);
}
pub(crate) fn statement(s: &mut Statement, v: &mut impl Visitor) {
    v.enter_statement(s);
    match &mut s.kind {
        StatementKind::Block(b) => block(b, v),
        StatementKind::Local {
            annotation,
            initializer,
            ..
        } => {
            if let Some(t) = annotation {
                ty(t, v);
            }
            expression(initializer, v);
        }
        StatementKind::FieldAssign { base, value, .. } => {
            expression(base, v);
            expression(value, v);
        }
        StatementKind::Assign { value, .. }
        | StatementKind::Expression(value)
        | StatementKind::Throw(value) => expression(value, v),
        StatementKind::Return(value) => {
            if let Some(e) = value {
                expression(e, v);
            }
        }
        StatementKind::If {
            condition,
            then_block,
            else_branch,
        } => {
            expression(condition, v);
            block(then_block, v);
            if let Some(s) = else_branch {
                statement(s, v);
            }
        }
        StatementKind::While { condition, body } => {
            expression(condition, v);
            block(body, v);
        }
        StatementKind::Switch { value, cases } => {
            expression(value, v);
            for c in cases {
                block(&mut c.body, v);
            }
        }
        StatementKind::TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            block(body, v);
            for handler in handlers {
                if let CatchPattern::List { elements, .. } = &mut handler.pattern {
                    for element in elements {
                        ty(&mut element.ty, v);
                    }
                }
                block(&mut handler.body, v);
            }
            if let Some(cleanup) = cleanup {
                block(cleanup, v);
            }
        }
        StatementKind::TryAll {
            body,
            handler,
            cleanup,
        } => {
            block(body, v);
            if let Some(h) = handler {
                block(&mut h.body, v);
            }
            if let Some(c) = cleanup {
                block(c, v);
            }
        }
        StatementKind::TryCatch {
            body,
            handler,
            cleanup,
            ..
        } => {
            block(body, v);
            block(handler, v);
            if let Some(b) = cleanup {
                block(b, v);
            }
        }
        StatementKind::TryFinally { body, cleanup }
        | StatementKind::TaskScope { body, cleanup } => {
            block(body, v);
            block(cleanup, v);
        }
        StatementKind::Break | StatementKind::Continue => {}
    }
    v.leave_statement(s);
}
fn arguments(a: &mut Arguments, v: &mut impl Visitor) {
    match a {
        Arguments::Positional(values) => {
            for e in values {
                expression(e, v);
            }
        }
        Arguments::Named(values) => {
            for (_, e, _) in values {
                expression(e, v);
            }
        }
    }
}
pub(crate) fn expression(e: &mut Expression, v: &mut impl Visitor) {
    match &mut e.kind {
        ExpressionKind::Function(f) => {
            v.enter_function(f);
            for p in &mut f.parameters {
                ty(&mut p.ty, v);
            }
            ty(&mut f.result, v);
            block(&mut f.body, v);
            if let Some(fallback) = &mut f.fallback {
                expression(fallback, v);
            }
            v.leave_function(f);
        }
        ExpressionKind::Record(fields) => {
            for field in fields {
                if let Some(t) = &mut field.annotation {
                    ty(t, v);
                }
                expression(&mut field.value, v);
            }
        }
        ExpressionKind::Bind {
            arguments: args,
            type_arguments,
            ..
        }
        | ExpressionKind::GenericCall {
            arguments: args,
            type_arguments,
            ..
        } => {
            for t in type_arguments {
                ty(t, v);
            }
            arguments(args, v);
        }
        ExpressionKind::Call { arguments: a, .. } | ExpressionKind::Spawn { arguments: a, .. } => {
            arguments(a, v)
        }
        ExpressionKind::RecordUpdate { base, fields } => {
            expression(base, v);
            for (_, e, _) in fields {
                expression(e, v);
            }
        }
        ExpressionKind::List(values) | ExpressionKind::FormatString(values) => {
            for e in values {
                expression(e, v);
            }
        }
        ExpressionKind::Map(values) => {
            for (k, val) in values {
                expression(k, v);
                expression(val, v);
            }
        }
        ExpressionKind::Unary { operand, .. } | ExpressionKind::Await(operand) => {
            expression(operand, v)
        }
        ExpressionKind::Binary { left, right, .. } => {
            expression(left, v);
            expression(right, v);
        }
        ExpressionKind::Is { value, ty: target } => {
            expression(value, v);
            ty(target, v);
        }
        ExpressionKind::Literal(_) | ExpressionKind::Name(_) | ExpressionKind::Unit => {}
    }
    v.expression(e);
}
