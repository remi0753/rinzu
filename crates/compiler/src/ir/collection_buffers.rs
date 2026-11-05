use super::*;
use crate::ast_walk::{self, Visitor};

pub(super) struct LoopBuffer {
    pub local: String,
    pub updates: Vec<SourceRange>,
    at_most_one_update: bool,
}

/// A fresh local stays private until the loop's normal/break exit. Reject
/// other reads and writes, including reads in arguments, captures, bounds and
/// cleanup paths. The initializer and loop must be siblings so an enclosing
/// handler cannot reach the local when the loop is unwound.
pub(super) fn loops(
    checked: &CheckedModule,
    block: &Block,
) -> HashMap<SourceRange, Vec<LoopBuffer>> {
    let mut loops = HashMap::<_, Vec<_>>::new();
    for (index, statement) in block.statements.iter().enumerate() {
        let StatementKind::Local {
            mutable: true,
            name,
            initializer,
            ..
        } = &statement.kind
        else {
            continue;
        };
        let kind = match &initializer.kind {
            ExpressionKind::List(elements) if elements.is_empty() => {
                crate::DataOperation::ListAppend
            }
            ExpressionKind::Map(entries) if entries.is_empty() => crate::DataOperation::MapInsert,
            _ => continue,
        };
        for next in &block.statements[index + 1..] {
            let loop_statement = matches!(
                next.kind,
                StatementKind::While { .. } | StatementKind::For { .. }
            );
            let mut uses = Uses {
                local: name,
                operation: kind,
                checked,
                allowed: HashSet::new(),
                updates: Vec::new(),
                observed: false,
                exits_scope: false,
                loop_depth: 0,
                nested_update: false,
            };
            ast_walk::statement(&mut next.clone(), &mut uses);
            if uses.observed {
                break;
            }
            if !uses.updates.is_empty() {
                if loop_statement && !uses.exits_scope {
                    loops.entry(next.range).or_default().push(LoopBuffer {
                        local: name.clone(),
                        at_most_one_update: !uses.nested_update && uses.updates.len() == 1,
                        updates: uses.updates,
                    });
                }
                break;
            }
        }
    }
    loops
}

struct Uses<'a> {
    local: &'a str,
    operation: crate::DataOperation,
    checked: &'a CheckedModule,
    allowed: HashSet<SourceRange>,
    updates: Vec<SourceRange>,
    observed: bool,
    exits_scope: bool,
    loop_depth: usize,
    nested_update: bool,
}

impl Visitor for Uses<'_> {
    fn enter_statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::While { .. } | StatementKind::For { .. } => self.loop_depth += 1,
            StatementKind::Assign { name, value } if name == self.local => {
                if self.checked.data_operations.get(&value.range) == Some(&self.operation)
                    && let ExpressionKind::Call {
                        arguments: Arguments::Positional(args),
                        ..
                    } = &value.kind
                    && let Some(first) = args.first()
                    && matches!(&first.kind, ExpressionKind::Name(path) if path.as_slice() == [self.local])
                {
                    self.allowed.insert(first.range);
                    self.updates.push(value.range);
                    self.nested_update |= self.loop_depth > 1;
                } else {
                    self.observed = true;
                }
            }
            StatementKind::TryErrors { .. }
            | StatementKind::TryAll { .. }
            | StatementKind::TryCatch { .. }
            | StatementKind::TryFinally { .. }
            | StatementKind::TaskScope { .. } => self.exits_scope = true,
            _ => {}
        }
    }

    fn leave_statement(&mut self, statement: &Statement) {
        if matches!(
            statement.kind,
            StatementKind::While { .. } | StatementKind::For { .. }
        ) {
            self.loop_depth -= 1;
        }
    }

    fn expression(&mut self, expression: &mut Expression) {
        match &expression.kind {
            ExpressionKind::Name(path)
                if path.first().is_some_and(|name| name == self.local)
                    && !self.allowed.contains(&expression.range) =>
            {
                self.observed = true;
            }
            ExpressionKind::Await(_) => self.exits_scope = true,
            _ => {}
        }
    }
}

/// Prove a public update cannot overflow when its input has a known small
/// bound, or it grows from empty at most once per bounded iteration.
pub(crate) fn in_bounds(checked: &CheckedModule) -> HashSet<SourceRange> {
    if !checked.data_operations.values().any(|operation| {
        matches!(
            operation,
            crate::DataOperation::ListAppend | crate::DataOperation::MapInsert
        )
    }) {
        return HashSet::new();
    }
    struct Bounds<'a> {
        checked: &'a CheckedModule,
        scopes: Vec<HashMap<String, usize>>,
        safe: HashSet<SourceRange>,
    }
    impl Bounds<'_> {
        fn length(&self, value: &Expression) -> Option<usize> {
            use crate::DataOperation::*;
            match &value.kind {
                ExpressionKind::List(values) => Some(values.len()),
                ExpressionKind::Map(values) => Some(values.len()),
                ExpressionKind::Name(path) if path.len() == 1 => self
                    .scopes
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&path[0]).copied()),
                ExpressionKind::Call {
                    arguments: Arguments::Positional(values),
                    ..
                } => {
                    let input = values.first()?;
                    match self.checked.data_operations.get(&value.range)? {
                        ListAppend | MapInsert => self.length(input)?.checked_add(1),
                        MapRemove | MapKeys | MapEntries => self.length(input),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
    }
    impl Visitor for Bounds<'_> {
        fn enter_block(&mut self, block: &Block) {
            self.scopes.push(HashMap::new());
            let loops = loops(self.checked, block);
            for (index, statement) in block.statements.iter().enumerate() {
                if let Some(buffers) = loops.get(&statement.range)
                    && bounded(self.checked, statement, &block.statements[..index])
                {
                    self.safe.extend(
                        buffers
                            .iter()
                            .filter(|buffer| buffer.at_most_one_update)
                            .flat_map(|buffer| buffer.updates.iter().copied()),
                    );
                }
            }
        }
        fn leave_block(&mut self, _: &Block) {
            self.scopes.pop();
        }
        fn leave_statement(&mut self, statement: &Statement) {
            if let StatementKind::Local {
                mutable: false,
                name,
                initializer,
                ..
            } = &statement.kind
                && let Some(length) = self.length(initializer)
            {
                self.scopes.last_mut().unwrap().insert(name.clone(), length);
            }
        }
        fn expression(&mut self, expression: &mut Expression) {
            if matches!(
                self.checked.data_operations.get(&expression.range),
                Some(crate::DataOperation::ListAppend | crate::DataOperation::MapInsert)
            ) && let ExpressionKind::Call {
                arguments: Arguments::Positional(args),
                ..
            } = &expression.kind
                && self
                    .length(&args[0])
                    .is_some_and(|length| length < i64::MAX as usize)
            {
                self.safe.insert(expression.range);
            }
        }
    }
    let mut bounds = Bounds {
        checked,
        scopes: Vec::new(),
        safe: HashSet::new(),
    };
    for function in &checked.functions {
        ast_walk::block(&mut function.declaration.body.clone(), &mut bounds);
    }
    bounds.safe
}

fn bounded(checked: &CheckedModule, statement: &Statement, preceding: &[Statement]) -> bool {
    let StatementKind::While { condition, body } = &statement.kind else {
        return matches!(statement.kind, StatementKind::For { .. });
    };
    let ExpressionKind::Binary {
        operator: BinaryOperator::Less,
        left,
        ..
    } = &condition.kind
    else {
        return false;
    };
    let ExpressionKind::Name(counter) = &left.kind else {
        return false;
    };
    if checked.expression_type(left.range) != Some(&Type::Int)
        || !crate::collections::bounded_counter(checked, condition, body)
    {
        return false;
    }
    let [counter] = counter.as_slice() else {
        return false;
    };
    struct Writes<'a>(&'a str, bool);
    impl Visitor for Writes<'_> {
        fn enter_statement(&mut self, statement: &Statement) {
            self.1 |=
                matches!(&statement.kind, StatementKind::Assign { name, .. } if name == self.0);
        }
    }
    for statement in preceding.iter().rev() {
        if let StatementKind::Local {
            name, initializer, ..
        } = &statement.kind
            && name == counter
        {
            return matches!(initializer.kind, ExpressionKind::Literal(Literal::Int(0)));
        }
        let mut writes = Writes(counter, false);
        ast_walk::statement(&mut statement.clone(), &mut writes);
        if writes.1 {
            return false;
        }
    }
    false
}
