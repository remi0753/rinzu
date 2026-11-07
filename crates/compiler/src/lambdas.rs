//! Lexical free-variable analysis; captures are immutable value parameters.
use crate::{SourceRange, ast::*};
use std::collections::{HashMap, HashSet};

pub(crate) fn captures(
    function: &FunctionDecl,
    outer: &HashSet<String>,
) -> Vec<(String, SourceRange)> {
    let mut function = function.clone();
    let mut captures = Vec::new();
    let mut seen = HashSet::new();
    walk_function(&mut function, &HashSet::new(), &mut |name, range| {
        if outer.contains(name) && seen.insert(name.clone()) {
            captures.push((name.clone(), range));
        }
    });
    captures
}
pub(crate) fn rewrite(function: &mut FunctionDecl, names: &HashMap<String, String>) {
    walk_function(function, &HashSet::new(), &mut |name, _| {
        if let Some(capture) = names.get(name) {
            *name = capture.clone();
        }
    });
}

fn walk_function(
    f: &mut FunctionDecl,
    outer: &HashSet<String>,
    visit: &mut impl FnMut(&mut String, SourceRange),
) {
    let mut bound = outer.clone();
    bound.extend(f.parameters.iter().map(|p| p.name.clone()));
    for p in &mut f.parameters {
        ty(&mut p.ty, &bound, visit);
    }
    ty(&mut f.result, &bound, visit);
    block(&mut f.body, &bound, visit);
    if let Some(e) = &mut f.fallback {
        expression(e, &bound, visit);
    }
}
fn ty(t: &mut TypeRef, bound: &HashSet<String>, visit: &mut impl FnMut(&mut String, SourceRange)) {
    match &mut t.kind {
        TypeRefKind::Record(fields) => {
            for f in fields {
                ty(&mut f.ty, bound, visit);
            }
        }
        TypeRefKind::Generic { arguments, .. } => {
            for a in arguments {
                ty(a, bound, visit);
            }
        }
        TypeRefKind::List(t) | TypeRefKind::Future(t) => ty(t, bound, visit),
        TypeRefKind::Map(k, v) => {
            ty(k, bound, visit);
            ty(v, bound, visit);
        }
        TypeRefKind::Function {
            parameters,
            result,
            error_contract,
            ..
        } => {
            for p in parameters {
                ty(p, bound, visit);
            }
            ty(result, bound, visit);
            if let Some(contract) = error_contract {
                for value in &mut contract.types {
                    ty(value, bound, visit);
                }
            }
        }
        TypeRefKind::Bytes(e) => expression(e, bound, visit),
        _ => {}
    }
}
fn block(b: &mut Block, outer: &HashSet<String>, visit: &mut impl FnMut(&mut String, SourceRange)) {
    let mut bound = outer.clone();
    for s in &mut b.statements {
        statement(s, &mut bound, visit);
    }
}
fn statement(
    s: &mut Statement,
    bound: &mut HashSet<String>,
    visit: &mut impl FnMut(&mut String, SourceRange),
) {
    match &mut s.kind {
        StatementKind::Block(b) => block(b, bound, visit),
        StatementKind::Local {
            name,
            annotation,
            initializer,
            ..
        } => {
            if let Some(t) = annotation {
                ty(t, bound, visit);
            }
            expression(initializer, bound, visit);
            bound.insert(name.clone());
        }
        StatementKind::Assign { name, value } => {
            if !bound.contains(name) {
                visit(name, s.range);
            }
            expression(value, bound, visit);
        }
        StatementKind::FieldAssign { base, value, .. } => {
            expression(base, bound, visit);
            expression(value, bound, visit);
        }
        StatementKind::Expression(e) | StatementKind::Throw(e) => expression(e, bound, visit),
        StatementKind::Return(e) => {
            if let Some(e) = e {
                expression(e, bound, visit);
            }
        }
        StatementKind::If {
            condition,
            then_block,
            else_branch,
        } => {
            expression(condition, bound, visit);
            block(then_block, bound, visit);
            if let Some(s) = else_branch {
                statement(s, &mut bound.clone(), visit);
            }
        }
        StatementKind::While { condition, body } => {
            expression(condition, bound, visit);
            block(body, bound, visit);
        }
        StatementKind::For {
            binding,
            iterable,
            body,
            ..
        } => {
            expression(iterable, bound, visit);
            let mut inner = bound.clone();
            inner.insert(binding.clone());
            block(body, &inner, visit);
        }
        StatementKind::Switch { value, cases } => {
            expression(value, bound, visit);
            for c in cases {
                let mut case = bound.clone();
                case.extend(c.bindings.clone());
                block(&mut c.body, &case, visit);
            }
        }
        StatementKind::TryErrors {
            body,
            handlers,
            cleanup,
            ..
        } => {
            block(body, bound, visit);
            for handler in handlers {
                if let CatchPattern::List { elements, .. } = &mut handler.pattern {
                    for element in elements {
                        ty(&mut element.ty, bound, visit);
                    }
                }
                let mut names = bound.clone();
                names.extend(
                    handler
                        .pattern
                        .bindings()
                        .map(|binding| binding.name.clone()),
                );
                block(&mut handler.body, &names, visit);
            }
            if let Some(cleanup) = cleanup {
                block(cleanup, bound, visit);
            }
        }
        StatementKind::TryAll {
            body,
            handler,
            cleanup,
        } => {
            block(body, bound, visit);
            if let Some(h) = handler {
                let mut names = bound.clone();
                names.insert(h.binding.clone());
                block(&mut h.body, &names, visit);
            }
            if let Some(c) = cleanup {
                block(c, bound, visit);
            }
        }
        StatementKind::TryCatch {
            body,
            binding,
            handler,
            cleanup,
            ..
        } => {
            block(body, bound, visit);
            let mut caught = bound.clone();
            caught.insert(binding.clone());
            block(handler, &caught, visit);
            if let Some(b) = cleanup {
                block(b, bound, visit);
            }
        }
        StatementKind::TryFinally { body, cleanup }
        | StatementKind::TaskScope { body, cleanup } => {
            block(body, bound, visit);
            block(cleanup, bound, visit);
        }
        StatementKind::Break | StatementKind::Continue => {}
    }
}
fn path(
    path: &mut [String],
    range: SourceRange,
    bound: &HashSet<String>,
    visit: &mut impl FnMut(&mut String, SourceRange),
) {
    if let Some(name) = path.first_mut()
        && !bound.contains(name)
    {
        visit(name, range);
    }
}
fn arguments(
    args: &mut Arguments,
    bound: &HashSet<String>,
    visit: &mut impl FnMut(&mut String, SourceRange),
) {
    match args {
        Arguments::Positional(values) => {
            for e in values {
                expression(e, bound, visit);
            }
        }
        Arguments::Named(values) => {
            for (_, e, _) in values {
                expression(e, bound, visit);
            }
        }
    }
}
fn expression(
    e: &mut Expression,
    bound: &HashSet<String>,
    visit: &mut impl FnMut(&mut String, SourceRange),
) {
    match &mut e.kind {
        ExpressionKind::Function(f) => walk_function(f, bound, visit),
        ExpressionKind::Record(fields) => {
            for f in fields {
                if let Some(t) = &mut f.annotation {
                    ty(t, bound, visit);
                }
                expression(&mut f.value, bound, visit);
            }
        }
        ExpressionKind::Name(names) => path(names, e.range, bound, visit),
        ExpressionKind::Call {
            target,
            arguments: args,
        }
        | ExpressionKind::Spawn {
            target,
            arguments: args,
        } => {
            path(target, e.range, bound, visit);
            arguments(args, bound, visit);
        }
        ExpressionKind::GenericCall {
            target,
            type_arguments,
            member_type_arguments,
            arguments: args,
            ..
        } => {
            path(target, e.range, bound, visit);
            for t in type_arguments.iter_mut().chain(member_type_arguments) {
                ty(t, bound, visit);
            }
            arguments(args, bound, visit);
        }
        ExpressionKind::GenericReference {
            target,
            type_arguments,
            member_type_arguments,
            ..
        } => {
            path(target, e.range, bound, visit);
            for t in type_arguments.iter_mut().chain(member_type_arguments) {
                ty(t, bound, visit);
            }
        }
        ExpressionKind::Bind {
            target,
            type_arguments,
            arguments: args,
        } => {
            path(target, e.range, bound, visit);
            for t in type_arguments {
                ty(t, bound, visit);
            }
            arguments(args, bound, visit);
        }
        ExpressionKind::RecordUpdate { base, fields } => {
            expression(base, bound, visit);
            for (_, value, _) in fields {
                expression(value, bound, visit);
            }
        }
        ExpressionKind::List(values) | ExpressionKind::FormatString(values) => {
            for e in values {
                expression(e, bound, visit);
            }
        }
        ExpressionKind::Map(entries) => {
            for (k, v) in entries {
                expression(k, bound, visit);
                expression(v, bound, visit);
            }
        }
        ExpressionKind::Unary { operand, .. } | ExpressionKind::Await(operand) => {
            expression(operand, bound, visit)
        }
        ExpressionKind::Is { value, ty: target } => {
            expression(value, bound, visit);
            ty(target, bound, visit);
        }
        ExpressionKind::Binary { left, right, .. } => {
            expression(left, bound, visit);
            expression(right, bound, visit);
        }
        ExpressionKind::Literal(_) | ExpressionKind::Unit => {}
    }
}

pub(crate) fn normalize(module: &mut Module, replacements: &HashMap<SourceRange, ExpressionKind>) {
    struct Normalizer<'a>(&'a HashMap<SourceRange, ExpressionKind>);
    impl crate::ast_walk::Visitor for Normalizer<'_> {
        fn expression(&mut self, e: &mut Expression) {
            if matches!(e.kind, ExpressionKind::Function(_))
                && let Some(kind) = self.0.get(&e.range)
            {
                e.kind = kind.clone();
            }
        }
    }
    crate::ast_walk::module(module, &mut Normalizer(replacements));
}
