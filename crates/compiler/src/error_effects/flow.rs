use super::substitution::instantiate;
use super::*;
use language::{Origin, OriginKind, Reception, concatenate, one, unknown};
use std::sync::Arc;
mod all;
mod values;

type Environment = BTreeMap<String, Value>;

#[derive(Clone, Default)]
struct Flow {
    collected: Option<Language>,
    normal: bool,
    returns: bool,
    breaks: bool,
    continues: bool,
    errors: Language,
    returned: Value,
}

impl Flow {
    fn normal() -> Self {
        Self {
            normal: true,
            ..Self::default()
        }
    }
    fn join(&mut self, other: Self) {
        let mut words = self.exit_words();
        words.extend(other.exit_words());
        self.normal |= other.normal;
        self.returns |= other.returns;
        self.breaks |= other.breaks;
        self.continues |= other.continues;
        self.errors.extend(other.errors);
        self.returned.join(other.returned);
        self.collected = Some(words);
    }
    fn has_non_error_exit(&self) -> bool {
        self.normal || self.returns || self.breaks || self.continues
    }
    fn exit_words(&self) -> Language {
        if self.has_non_error_exit() {
            self.collected.clone().unwrap_or_else(language::empty_word)
        } else {
            Language::new()
        }
    }
}

struct Evaluated {
    value: Value,
    errors: Language,
    success: bool,
}
impl Evaluated {
    fn pure(value: Value) -> Self {
        Self {
            value,
            errors: Language::new(),
            success: true,
        }
    }
}

pub(super) struct Analyzer<'a> {
    pub(super) checked: &'a CheckedModule,
    summaries: &'a BTreeMap<String, Summary>,
    pub(super) bound: usize,
    diagnose: bool,
    proof: bool,
    diagnostics: Vec<Diagnostic>,
    all: bool,
    trace_bound: Option<Arc<BTreeSet<Tag>>>,
    catch_trace: Option<Box<catch_diagnostics::Replay>>,
    in_finally: bool,
    caught: Vec<(SourceRange, Language)>,
    rethrow_paths: std::collections::HashMap<SourceRange, Language>,
    dead: bool,
}

impl<'a> Analyzer<'a> {
    pub fn new(
        checked: &'a CheckedModule,
        summaries: &'a BTreeMap<String, Summary>,
        bound: usize,
        diagnose: bool,
    ) -> Self {
        Self {
            checked,
            summaries,
            bound,
            diagnose,
            proof: false,
            diagnostics: Vec::new(),
            all: false,
            trace_bound: None,
            catch_trace: None,
            in_finally: false,
            caught: Vec::new(),
            rethrow_paths: Default::default(),
            dead: false,
        }
    }

    pub fn proving_conversions(mut self, proof: bool) -> Self {
        self.proof = proof;
        self
    }

    pub fn traced_function(
        mut self,
        function: &crate::CheckedFunction,
        allowed: BTreeSet<Tag>,
        replay: Option<catch_diagnostics::Replay>,
    ) -> (Language, Option<Box<catch_diagnostics::Replay>>) {
        self.trace_bound = Some(Arc::new(allowed));
        self.catch_trace = replay.map(Box::new);
        let summary = self.analyze_function(function);
        (summary.errors, self.catch_trace)
    }

    fn check_dead_code(&self) -> bool {
        self.diagnose
            || self
                .catch_trace
                .as_ref()
                .is_some_and(|trace| trace.has_targets())
    }

    fn at(&self, errors: Language, range: SourceRange, kind: OriginKind) -> Language {
        self.at_reception(errors, range, kind, None)
    }

    fn at_reception(
        &self,
        errors: Language,
        range: SourceRange,
        kind: OriginKind,
        reception: Option<Arc<Reception>>,
    ) -> Language {
        if let Some(allowed) = &self.trace_bound {
            errors.at(
                allowed,
                Origin {
                    range,
                    kind,
                    in_finally: self.in_finally,
                    forwarded: None,
                },
                self.catch_trace.is_some(),
                reception,
            )
        } else {
            errors
        }
    }

    pub fn function(mut self, function: &crate::CheckedFunction) -> (Summary, Vec<Diagnostic>) {
        let summary = self.analyze_function(function);
        if self.diagnose && !self.proof {
            let mut sites: Vec<_> = std::mem::take(&mut self.rethrow_paths)
                .into_iter()
                .collect();
            sites.sort_by_key(|(range, _)| (range.file_id.0, range.start_byte));
            for (range, paths) in sites {
                let selection = &self.checked.rethrows[&range];
                let start = selection.tail.unwrap() as usize;
                if paths.is_empty()
                    || paths
                        .iter()
                        .any(|word| word.prefix.len() + usize::from(word.longer) <= start)
                {
                    let (binding, definition) = selection.prove_tail.as_ref().unwrap();
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidControlFlow, binding.range,
                        format!("cannot prove that {} is nonempty for rethrow", binding.name),
                        "handle the shorter lists in a preceding catch or include a fixed Error",
                    ).with_secondary(*definition, "this catch binding can select an empty tail"));
                }
            }
        }
        (summary, self.diagnostics)
    }

    fn analyze_function(&mut self, function: &crate::CheckedFunction) -> Summary {
        let mut env = Environment::new();
        for (index, (parameter, ty)) in function
            .declaration
            .parameters
            .iter()
            .zip(&function.parameter_types)
            .enumerate()
        {
            let value = if *ty == Type::Error {
                Value::Error(BTreeSet::from([Tag::Parameter(Input {
                    owner: function.declaration.name.clone(),
                    index,
                    path: Vec::new(),
                })]))
            } else if tag(self.checked, ty).is_some() {
                self.typed(ty)
            } else {
                Value::Parameter {
                    owner: function.declaration.name.clone(),
                    index,
                    path: Vec::new(),
                    fallback: Box::new(self.typed(ty)),
                }
            };
            env.insert(parameter.name.clone(), value);
        }
        let flow = self.block(&function.declaration.body, &mut env);
        let success = flow.normal || flow.returns;
        let mut returned = flow.returned;
        if flow.normal {
            returned.join(self.typed(&function.result_type));
        }
        Summary {
            errors: language::normalize(flow.errors),
            success,
            returned: returned.finite(),
        }
    }

    fn typed(&self, ty: &Type) -> Value {
        if let Some(tag) = tag(self.checked, ty) {
            return Value::Error(BTreeSet::from([tag]));
        }
        match ty {
            Type::Future(inner) => Value::Future {
                errors: unknown(self.bound),
                success: true,
                value: Box::new(self.typed(inner)),
            },
            Type::Bool => Value::Bool(None),
            Type::Never => Value::Bottom,
            Type::Named(name) => Value::Opaque(name.clone()),
            Type::List(element) | Type::Map(_, element) => {
                Value::Collection(Box::new(self.typed(element)))
            }
            Type::FunctionReference(signature) if self.proof => signature
                .error_bound
                .as_ref()
                .map_or(Value::Plain, |bound| Value::BoundedCallable(bound.clone())),
            _ => Value::Plain,
        }
    }

    fn expression_value(&self, expression: &Expression) -> Value {
        // The contextual type of a pending conversion is only its destination.
        // Its value must establish the bound independently of that annotation.
        if self.proof
            && self
                .checked
                .error_conversions
                .contains_key(&expression.range)
        {
            return Value::Plain;
        }
        self.checked
            .error_upcasts
            .get(&expression.range)
            .or_else(|| self.checked.expression_type(expression.range))
            .map_or(Value::Plain, |ty| self.typed(ty))
    }

    fn block(&mut self, block: &Block, env: &mut Environment) -> Flow {
        if self.all {
            return self.all_block(block, env);
        }
        let outer: BTreeSet<_> = env.keys().cloned().collect();
        let mut exiting: Option<Environment> = None;
        let mut flow = Flow::normal();
        for statement in &block.statements {
            if flow.normal {
                let before = env.clone();
                let next = self.statement(statement, env);
                if next.returns || next.breaks || next.continues || !next.errors.is_empty() {
                    let saved = exiting.get_or_insert_with(|| before.clone());
                    join_env(saved, &before);
                    join_env(saved, env);
                }
                flow.normal = false;
                flow.join(next);
            } else if self.check_dead_code() {
                // Still check an unreachable catch locally, but never let dead
                // assignments alter the environment seen by finally.
                let dead = std::mem::replace(&mut self.dead, true);
                self.statement(statement, &mut env.clone());
                self.dead = dead;
            }
        }
        // finally can run at any exit, including an earlier failing expression.
        // Its reads must include those states as well as the fallthrough state.
        if let Some(exiting) = exiting {
            join_env(env, &exiting);
        }
        env.retain(|name, _| outer.contains(name));
        flow
    }

    fn statement(&mut self, statement: &Statement, env: &mut Environment) -> Flow {
        match &statement.kind {
            StatementKind::Block(block) => self.block(block, env),
            StatementKind::Local {
                name, initializer, ..
            }
            | StatementKind::Assign {
                name,
                value: initializer,
            } => {
                let value = self.expression(initializer, env, false);
                env.insert(name.clone(), value.value.finite());
                Flow {
                    normal: value.success,
                    errors: value.errors,
                    ..Flow::default()
                }
            }
            StatementKind::FieldAssign { base, value, .. } => {
                let a = self.expression(base, env, false);
                let b = self.expression(value, env, false);
                let mut errors = a.errors;
                if a.success {
                    errors.extend(b.errors);
                }
                Flow {
                    normal: a.success && b.success,
                    errors,
                    ..Flow::default()
                }
            }
            StatementKind::Expression(expression) => {
                let value = self.expression(expression, env, false);
                Flow {
                    normal: value.success,
                    errors: value.errors,
                    ..Flow::default()
                }
            }
            StatementKind::Throw(expression) => {
                let value = self.expression(expression, env, false);
                let mut errors = value.errors;
                if value.success {
                    errors.extend(self.at(
                        value.value.thrown_errors(),
                        statement.range,
                        OriginKind::Throw,
                    ));
                }

                Flow {
                    errors,
                    ..Flow::default()
                }
            }
            StatementKind::Rethrow { .. } => {
                let selection = &self.checked.rethrows[&statement.range];
                let mut catches = self.caught.iter();
                let mut paths = catches
                    .find(|(scope, _)| *scope == selection.handler)
                    .map_or_else(Language::new, |(_, paths)| paths.clone());
                for (_, inner) in catches {
                    paths = language::conditioned(&paths, inner);
                }
                if let Some((binding, _)) = &selection.prove_tail {
                    if self.diagnose && !self.proof {
                        let reached = self.rethrow_paths.entry(statement.range).or_default();
                        if !self.dead {
                            reached.extend(paths.clone());
                        }
                    }
                    if !self.dead
                        && let Some(trace) = &mut self.catch_trace
                    {
                        trace.record_rethrow(
                            binding.range,
                            selection.tail.unwrap() as usize,
                            &paths,
                        );
                    }
                }
                let forwarded = self.trace_bound.as_ref().map(|_| Origin {
                    range: statement.range,
                    kind: OriginKind::Throw,
                    in_finally: self.in_finally,
                    forwarded: Some(statement.range),
                });
                Flow {
                    errors: language::select_caught(
                        &paths,
                        &selection.elements,
                        selection.tail,
                        self.bound,
                        forwarded,
                    ),
                    ..Flow::default()
                }
            }
            StatementKind::Return(expression) => {
                let value = expression.as_ref().map_or_else(
                    || Evaluated::pure(Value::Plain),
                    |e| self.expression(e, env, false),
                );
                Flow {
                    returns: value.success,
                    errors: value.errors,
                    returned: value.value,
                    ..Flow::default()
                }
            }
            StatementKind::Break => Flow {
                breaks: true,
                ..Flow::default()
            },
            StatementKind::Continue => Flow {
                continues: true,
                ..Flow::default()
            },
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                let condition = self.expression(condition, env, false);
                let known = match condition.value {
                    Value::Bool(value) => value,
                    _ => None,
                };
                let mut yes_env = env.clone();
                let dead = self.dead;
                self.dead |= !condition.success || known == Some(false);
                let yes = self.block(then_block, &mut yes_env);
                self.dead = dead || !condition.success || known == Some(true);
                let mut no_env = env.clone();
                let no = else_branch
                    .as_ref()
                    .map_or_else(Flow::normal, |branch| self.statement(branch, &mut no_env));
                self.dead = dead;
                let mut flow = Flow {
                    errors: condition.errors,
                    ..Flow::default()
                };
                if condition.success {
                    if known != Some(false) {
                        flow.join(yes);
                        join_env(env, &yes_env);
                    }
                    if known != Some(true) {
                        flow.join(no);
                        join_env(env, &no_env);
                    }
                }
                flow
            }
            StatementKind::While { condition, body } => {
                if self.all {
                    return self.all_while(condition, body, env);
                }
                let mut loop_env = env.clone();
                let mut result = Flow::default();
                loop {
                    let before = loop_env.clone();
                    let condition = self.expression(condition, &loop_env, false);
                    let known = match condition.value {
                        Value::Bool(value) => value,
                        _ => None,
                    };
                    result.errors.extend(condition.errors);
                    if condition.success && known != Some(true) {
                        result.normal = true;
                    }
                    let body = self.block(body, &mut loop_env);
                    if condition.success && known != Some(false) {
                        result.normal |= body.breaks;
                        result.returns |= body.returns;
                        result.errors.extend(body.errors);
                        result.returned.join(body.returned);
                    }
                    for value in loop_env.values_mut() {
                        *value = value.forget_elements();
                    }
                    join_env(&mut loop_env, &before);
                    if loop_env == before {
                        break;
                    }
                }
                join_env(env, &loop_env);
                result
            }
            StatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => {
                let source = self.expression(iterable, env, false);
                let ty = self
                    .checked
                    .expression_type(iterable.range)
                    .expect("checked List");
                let Type::List(element) = ty else {
                    unreachable!("checked List")
                };
                let value = source
                    .value
                    .project(Projection::AllElements(ty.to_string()), self.typed(element));
                let mut loop_env = env.clone();
                let mut body_flow = Flow::default();
                loop {
                    let before = loop_env.clone();
                    if binding != "_" {
                        loop_env.insert(binding.clone(), value.clone());
                    }
                    let flow = self.block(body, &mut loop_env);
                    body_flow.join(flow);
                    loop_env.remove(binding);
                    for (name, value) in &mut loop_env {
                        // The private bulk constructors only append to output.
                        // Its element projection represents every visited input,
                        // including those from earlier iterations.
                        if name != "output"
                            || iterable.range.file_id != crate::collections::source_file().id()
                        {
                            *value = value.forget_elements();
                        }
                    }
                    join_env(&mut loop_env, &before);
                    if loop_env == before {
                        break;
                    }
                }
                if !source.success {
                    return Flow {
                        errors: source.errors,
                        ..Flow::default()
                    };
                }
                join_env(env, &loop_env);
                if self.all {
                    let delta = language::independent_elements(&body_flow.exit_words());
                    let mut iterations = language::empty_word();
                    if body_flow.normal || body_flow.continues {
                        loop {
                            let next = language::concatenate(&iterations, &delta, self.bound);
                            let before = iterations.len();
                            iterations.extend(next);
                            if iterations.len() == before {
                                break;
                            }
                        }
                    }
                    let mut words = iterations.clone();
                    if body_flow.breaks || body_flow.returns {
                        words.extend(language::concatenate(&iterations, &delta, self.bound));
                    }
                    return Flow {
                        normal: true,
                        returns: body_flow.returns,
                        returned: body_flow.returned,
                        errors: source.errors,
                        collected: Some(words),
                        ..Flow::default()
                    };
                }
                let mut errors = source.errors;
                errors.extend(body_flow.errors);
                Flow {
                    normal: true,
                    returns: body_flow.returns,
                    errors,
                    returned: body_flow.returned,
                    ..Flow::default()
                }
            }
            StatementKind::Switch { value, cases } => {
                let value_type = self.checked.expression_type(value.range).cloned();
                let value = self.expression(value, env, false);
                let mut flow = Flow {
                    errors: value.errors,
                    ..Flow::default()
                };
                for case in cases {
                    let mut branch_env = env.clone();
                    let variant = case.path.last().expect("checked enum case");
                    for (index, name) in case.bindings.iter().enumerate() {
                        let fallback = self.variant_field(value_type.as_ref(), variant, index);
                        branch_env.insert(
                            name.clone(),
                            value.value.project(
                                Projection::Variant(
                                    value_type
                                        .as_ref()
                                        .map_or_else(String::new, ToString::to_string),
                                    variant.clone(),
                                    index,
                                ),
                                fallback,
                            ),
                        );
                    }
                    let branch = self.block(&case.body, &mut branch_env);
                    let conditions = value.value.selection_guards(variant);
                    let branch = self.condition_branch(branch, &mut branch_env, &conditions);
                    if value.success && !conditions.is_empty() {
                        flow.join(branch);
                        join_env(env, &branch_env);
                    }
                }
                flow
            }
            StatementKind::TryErrors {
                all,
                body,
                handlers,
                cleanup,
            } => self.try_flow(statement.range, *all, body, handlers, cleanup.as_ref(), env),
            StatementKind::TryCatch {
                body,
                binding,
                binding_range,
                handler,
                cleanup,
            } => {
                let handler = CatchClause::whole(binding, *binding_range, handler);
                self.try_flow(
                    statement.range,
                    false,
                    body,
                    &[handler],
                    cleanup.as_ref(),
                    env,
                )
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                let handlers = handler
                    .as_ref()
                    .map(|handler| {
                        CatchClause::whole(&handler.binding, handler.binding_range, &handler.body)
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                self.try_flow(
                    statement.range,
                    true,
                    body,
                    &handlers,
                    cleanup.as_ref(),
                    env,
                )
            }
            StatementKind::TryFinally { body, cleanup }
            | StatementKind::TaskScope { body, cleanup } => {
                let outer = self.all;
                self.all = false;
                let flow = self.block(body, env);
                let result = self.cleanup(flow, Some(cleanup), env);
                self.all = outer;
                result
            }
        }
    }

    fn handlers(
        &mut self,
        range: SourceRange,
        body: Flow,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        env: &mut Environment,
    ) -> Flow {
        let mut remaining = body.errors;
        let mut flow = Flow {
            errors: Language::new(),
            ..body
        };
        let diagnose = self.diagnose && !self.proof;
        let targeted = self
            .catch_trace
            .as_ref()
            .is_some_and(|trace| trace.targets(range));
        let mut previous = Vec::new();
        for (index, handler) in handlers.iter().enumerate() {
            let pattern = pattern(self.checked, &handler.pattern);
            if diagnose && let Some(proof) = pattern.covered_by(&previous, self.bound) {
                self.diagnostics.push(catch_diagnostics::unreachable(
                    handler,
                    &handlers[..index],
                    proof,
                ));
            }
            let matched = remaining
                .iter()
                .filter(|word| pattern.matches(word) != Match::Never)
                .cloned()
                .collect::<Language>();
            let mut catch_env = env.clone();
            match &handler.pattern {
                CatchPattern::Binding(binding) => {
                    catch_env.insert(binding.name.clone(), Value::Plain);
                }
                CatchPattern::List { elements, rest, .. } => {
                    for (index, element) in elements.iter().enumerate() {
                        if let Some(binding) = &element.binding {
                            let expected = &pattern.prefix[index];
                            let tags = if *expected == Tag::Any {
                                matched
                                    .iter()
                                    .filter_map(|word| word.prefix.get(index).cloned())
                                    .collect()
                            } else {
                                BTreeSet::from([expected.clone()])
                            };
                            catch_env.insert(binding.name.clone(), Value::Error(tags));
                        }
                    }
                    if let Some(rest) = rest {
                        catch_env.insert(rest.name.clone(), Value::Plain);
                    }
                }
            }
            if !self.checked.rethrows.is_empty() {
                self.caught
                    .push((handler.body.range, language::accepted(&remaining, &pattern)));
            }
            let mut caught = self.block(&handler.body, &mut catch_env);
            if !self.checked.rethrows.is_empty() {
                self.caught.pop();
            }
            caught.errors = language::conditioned(&caught.errors, &matched);
            if !matched.is_empty() {
                flow.join(caught);
                join_env(env, &catch_env);
            }
            remaining = language::subtract(&remaining, &pattern);
            if diagnose || targeted {
                previous.push(pattern);
            }
        }
        if !handlers.is_empty() && !remaining.is_empty() {
            if diagnose {
                self.diagnostics
                    .push(catch_diagnostics::pending(range, &remaining));
            }
            if targeted {
                self.catch_trace
                    .as_mut()
                    .unwrap()
                    .record(range, &remaining, handlers, previous);
            }
        }
        flow.errors.extend(remaining);
        self.cleanup(flow, cleanup, env)
    }

    fn cleanup(&mut self, mut flow: Flow, cleanup: Option<&Block>, env: &mut Environment) -> Flow {
        let Some(cleanup) = cleanup else {
            return flow;
        };
        let in_finally = std::mem::replace(&mut self.in_finally, true);
        let independent = self.catch_trace.is_none() || !Reads::block(self.checked, cleanup, env);
        let mut cleaned = self.block(cleanup, env);
        self.in_finally = in_finally;
        if self.catch_trace.is_some() && !cleaned.errors.is_empty() {
            cleaned.errors.mark_cleanup(cleanup.range, independent);
        }
        let mut errors = concatenate(&flow.errors, &cleaned.errors, self.bound);
        if flow.has_non_error_exit() {
            errors.extend(cleaned.errors);
        }
        if cleaned.normal {
            errors.extend(flow.errors);
        }
        if !cleaned.normal {
            flow.normal = false;
            flow.returns = false;
            flow.breaks = false;
            flow.continues = false;
        }
        flow.errors = errors;
        flow
    }

    fn expression(
        &mut self,
        expression: &Expression,
        env: &Environment,
        immediate_await: bool,
    ) -> Evaluated {
        let evaluated = self.expression_inner(expression, env, immediate_await);
        if self.proof
            && self.diagnose
            && evaluated.success
            && let Some(conversion) = self.checked.error_conversions.get(&expression.range)
        {
            let actual = self.proven_bound(&evaluated.value);
            if actual
                .as_ref()
                .is_none_or(|actual| !within_bound(actual, &conversion.expected))
            {
                self.diagnostics.push(conversion_error(
                    expression.range,
                    &conversion.expected,
                    actual.as_deref(),
                    conversion.related.clone(),
                ));
            }
        }
        evaluated
    }

    fn proven_bound(&self, value: &Value) -> Option<Vec<String>> {
        let mut bound = BTreeSet::new();
        match value {
            Value::BoundedCallable(types) => bound.extend(types.iter().cloned()),
            Value::Callable(targets) => {
                for target in targets {
                    if self.checked.functions.iter().any(|function| {
                        function.declaration.name == *target && function.declaration.is_async
                    }) || crate::core::reference_signature(target).is_some()
                    {
                        continue;
                    }
                    bound.extend(public_types(self.summaries.get(target)?));
                }
            }
            Value::Parameter { fallback, .. }
            | Value::Conditional {
                value: fallback, ..
            } => {
                return self.proven_bound(fallback);
            }
            Value::Union(values) => {
                for value in values {
                    bound.extend(self.proven_bound(value)?);
                }
            }
            _ => return None,
        }
        if bound.contains("Error") {
            Some(vec!["Error".into()])
        } else {
            Some(bound.into_iter().collect())
        }
    }

    fn expression_inner(
        &mut self,
        expression: &Expression,
        env: &Environment,
        immediate_await: bool,
    ) -> Evaluated {
        match &expression.kind {
            ExpressionKind::GenericReference { .. } => {
                unreachable!("generic references are specialized before effects")
            }
            ExpressionKind::Literal(Literal::Bool(value)) => {
                Evaluated::pure(Value::Bool(Some(*value)))
            }
            ExpressionKind::Literal(_) | ExpressionKind::Unit | ExpressionKind::Function(_) => {
                Evaluated::pure(self.expression_value(expression))
            }
            ExpressionKind::Name(path) => {
                let value = if let Some(ty) = self.checked.error_narrowings.get(&expression.range) {
                    self.typed(ty)
                } else if let Some(name) = self.checked.static_references.get(&expression.range) {
                    Value::Callable(BTreeSet::from([name.clone()]))
                } else if path.len() == 1 {
                    env.get(&path[0])
                        .cloned()
                        .unwrap_or_else(|| self.expression_value(expression))
                } else {
                    self.project_name(path, env, self.expression_value(expression))
                };
                let mut result = match value {
                    Value::Unbound(value) => Evaluated {
                        success: *value != Value::Bottom,
                        value: *value,
                        errors: self.at(one(Tag::Any), expression.range, OriginKind::Operation),
                    },
                    value => {
                        let mut result = Evaluated::pure(value);
                        if path.len() > 1
                            && let Some(Value::Unbound(inner)) = env.get(&path[0])
                        {
                            result.errors =
                                self.at(one(Tag::Any), expression.range, OriginKind::Operation);
                            result.success = **inner != Value::Bottom;
                        }
                        result
                    }
                };
                if self.checked.native_record_reads.contains(&expression.range) {
                    result.errors.extend(self.at(
                        one(Tag::Any),
                        expression.range,
                        OriginKind::Operation,
                    ));
                }
                result
            }
            ExpressionKind::Await(operand) => {
                let ty = self
                    .checked
                    .expression_type(operand.range)
                    .map_or_else(String::new, ToString::to_string);
                let evaluated = self.expression(operand, env, true);
                let mut errors = evaluated.errors;
                let (failed, success, value) =
                    evaluated
                        .value
                        .awaited(self.bound, self.expression_value(expression), &ty);
                if evaluated.success {
                    let reception = if self.catch_trace.is_some() && !failed.is_empty() {
                        self.await_reception(operand, expression.range)
                    } else {
                        None
                    };
                    errors.extend(self.at_reception(
                        failed,
                        expression.range,
                        OriginKind::Await,
                        reception,
                    ));
                }
                Evaluated {
                    value,
                    errors,
                    success: evaluated.success && success,
                }
            }

            ExpressionKind::Is { value, .. } => {
                let value = self.expression(value, env, false);
                let expected = self
                    .checked
                    .error_tests
                    .get(&expression.range)
                    .and_then(|ty| tag(self.checked, ty));
                let known = if let (Value::Error(tags), Some(expected)) = (&value.value, expected) {
                    if tags.iter().all(|tag| *tag == expected) {
                        Some(true)
                    } else if tags
                        .iter()
                        .all(|tag| matches!(tag, Tag::Named(_)) && *tag != expected)
                    {
                        Some(false)
                    } else {
                        None
                    }
                } else {
                    None
                };
                Evaluated {
                    value: Value::Bool(known),
                    ..value
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                let value = self.expression(operand, env, false);
                let result = if *operator == UnaryOperator::Not {
                    Value::Bool(if let Value::Bool(known) = value.value {
                        known.map(|value| !value)
                    } else {
                        None
                    })
                } else {
                    self.expression_value(expression)
                };
                Evaluated {
                    value: result,
                    ..value
                }
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let a = self.expression(left, env, false);
                let known = match a.value {
                    Value::Bool(value) => value,
                    _ => None,
                };
                let b = self.expression(right, env, false);
                let skip = (*operator == BinaryOperator::And && known == Some(false))
                    || (*operator == BinaryOperator::Or && known == Some(true));
                let mut errors = a.errors;
                if a.success && !skip {
                    errors.extend(b.errors);
                }
                Evaluated {
                    value: self.expression_value(expression),
                    errors,
                    success: a.success && (skip || known.is_none() || b.success),
                }
            }
            ExpressionKind::Call { target, arguments }
            | ExpressionKind::GenericCall {
                target, arguments, ..
            } => self.call(expression, target, arguments, env, immediate_await, false),
            ExpressionKind::Spawn { target, arguments } => {
                self.call(expression, target, arguments, env, immediate_await, true)
            }
            ExpressionKind::Bind { arguments, .. } => {
                let (_, errors, success) = self.arguments(arguments, env);
                Evaluated {
                    value: self.checked.bindings.get(&expression.range).map_or_else(
                        || self.expression_value(expression),
                        |binding| Value::Callable(BTreeSet::from([binding.target.clone()])),
                    ),
                    errors,
                    success,
                }
            }
            ExpressionKind::FormatString(values) => {
                let mut result = self.sequence(expression, values.iter(), env);
                if values.iter().any(|value| {
                    self.checked.expression_type(value.range).is_some_and(|ty| {
                        crate::sema::display_reads_native_record(&self.checked.source_types, ty)
                    })
                }) {
                    result.errors.extend(self.at(
                        one(Tag::Any),
                        expression.range,
                        OriginKind::Operation,
                    ));
                }
                result
            }
            ExpressionKind::List(values) => {
                self.collection_literal(values.iter().map(|value| (None, value)), env)
            }
            ExpressionKind::Map(values) => {
                self.collection_literal(values.iter().map(|(key, value)| (Some(key), value)), env)
            }
            ExpressionKind::Record(fields) => self.record_value(expression, fields, env),
            ExpressionKind::RecordUpdate { base, fields } => {
                self.record_update(expression, base, fields, env)
            }
        }
    }

    fn sequence<'b>(
        &mut self,
        expression: &Expression,
        values: impl Iterator<Item = &'b Expression>,
        env: &Environment,
    ) -> Evaluated {
        let mut result = Evaluated::pure(self.expression_value(expression));
        for value in values {
            let evaluated = self.expression(value, env, false);
            if result.success {
                result.errors.extend(evaluated.errors);
                result.success = evaluated.success;
            }
        }
        result
    }

    fn arguments(
        &mut self,
        arguments: &Arguments,
        env: &Environment,
    ) -> (Vec<Value>, Language, bool) {
        let expressions: Vec<_> = match arguments {
            Arguments::Positional(values) => values.iter().collect(),
            Arguments::Named(values) => values.iter().map(|(_, value, _)| value).collect(),
        };
        let mut values = Vec::new();
        let mut errors = Language::new();
        let mut success = true;
        for expression in expressions {
            let value = self.expression(expression, env, false);
            if success {
                errors.extend(value.errors);
                success = value.success;
            }
            values.push(value.value);
        }
        (values, errors, success)
    }

    fn await_reception(&mut self, operand: &Expression, at: SourceRange) -> Option<Arc<Reception>> {
        let target = match &operand.kind {
            ExpressionKind::Call { target, .. } | ExpressionKind::GenericCall { target, .. } => {
                target
            }
            _ => return None,
        };
        if self.checked.indirect_fields.contains_key(&operand.range) {
            return None;
        }
        let name = target.join(".");
        let function = self
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == name && f.declaration.is_async)?;
        self.catch_trace
            .as_mut()
            .map(|trace| trace.direct(at, &function.declaration))
    }

    fn call(
        &mut self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        env: &Environment,
        _immediate_await: bool,
        spawn: bool,
    ) -> Evaluated {
        // The callee is evaluated before its arguments, just like LoadLocal /
        // CheckLocal in IR. try all can continue after its initializer failed.
        let unbound = self
            .checked
            .indirect_calls
            .contains_key(&expression.range)
            .then(|| env.get(&target[0]))
            .flatten();
        if matches!(unbound, Some(Value::Unbound(inner)) if **inner == Value::Bottom) {
            return Evaluated {
                value: Value::Bottom,
                errors: self.at(one(Tag::Any), expression.range, OriginKind::Operation),
                success: false,
            };
        }
        let (values, mut errors, argument_success) = self.arguments(arguments, env);
        if self.checked.data_operations.get(&expression.range) == Some(&crate::DataOperation::Panic)
        {
            return Evaluated {
                value: Value::Bottom,
                errors,
                success: false,
            };
        }
        if matches!(unbound, Some(Value::Unbound(_))) {
            errors.extend(self.at(one(Tag::Any), expression.range, OriginKind::Operation));
        }
        let invocation = self
            .checked
            .indirect_signatures
            .get(&expression.range)
            .and_then(|call| {
                if self.proof {
                    let value = self.project_name(target, env, Value::Plain);
                    Some(self.proven_bound(&value).map_or_else(
                        || unknown(self.bound),
                        |bound| contracts::for_bound(&bound, self.bound),
                    ))
                } else {
                    call.signature
                        .error_bound
                        .as_ref()
                        .map(|bound| contracts::for_bound(bound, self.bound))
                }
            });
        let bounded_invocation = invocation.is_some();
        let name = target.join(".");
        if argument_success && let Some(invocation) = invocation {
            let reception = if invocation.is_empty() {
                None
            } else {
                self.catch_trace
                    .as_mut()
                    .map(|trace| trace.callback(self.checked, expression.range, &name))
            };
            errors.extend(self.at_reception(
                invocation,
                expression.range,
                OriginKind::Call,
                reception,
            ));
        }
        let indirect = self.checked.indirect_calls.get(&expression.range);
        let mut targets = indirect.cloned().unwrap_or_else(|| vec![name.clone()]);
        if targets.is_empty() {
            targets.push(name.clone());
            if argument_success && !bounded_invocation {
                let reception = self
                    .catch_trace
                    .as_mut()
                    .map(|trace| trace.callback(self.checked, expression.range, &name));
                errors.extend(self.at_reception(
                    unknown(self.bound),
                    expression.range,
                    OriginKind::UnknownCallback,
                    reception,
                ));
            }
        }
        let mut result = Value::Bottom;
        let mut success = false;
        for name in &targets {
            let paths = self.candidate_guards(expression, target, env, name);
            if paths.is_empty() {
                continue;
            }
            let restrict = |errors: &Language| {
                paths
                    .iter()
                    .flat_map(|guards| language::restricted(errors, guards))
                    .collect::<Language>()
            };
            if let Some(summary) = self.summaries.get(name).filter(|_| {
                super::operations::scalar_constructor(self.checked, expression, target).is_none()
            }) {
                let summary = instantiate(summary, name, &values, self.bound);
                let function = if spawn {
                    None
                } else {
                    self.checked
                        .functions
                        .iter()
                        .find(|function| function.declaration.name == *name)
                };
                let asynchronous = spawn || function.is_some_and(|f| f.declaration.is_async);
                if asynchronous {
                    let future = self.condition_value(
                        Value::Future {
                            errors: summary.errors,
                            success: summary.success,
                            value: Box::new(summary.returned),
                        },
                        &paths,
                    );
                    result.join(future);
                    success = true;
                } else {
                    if argument_success && !bounded_invocation {
                        let propagated = restrict(&summary.errors);
                        let reception = if propagated.is_empty() {
                            None
                        } else {
                            self.catch_trace.as_mut().map(|trace| {
                                if let Some(function) = function
                                    && !self.checked.indirect_fields.contains_key(&expression.range)
                                {
                                    trace.direct(expression.range, &function.declaration)
                                } else {
                                    trace.callback(
                                        self.checked,
                                        expression.range,
                                        &target.join("."),
                                    )
                                }
                            })
                        };
                        errors.extend(self.at_reception(
                            propagated,
                            expression.range,
                            OriginKind::Call,
                            reception,
                        ));
                    }
                    result.join(self.condition_value(summary.returned, &paths));
                    success |= summary.success;
                }
            } else {
                let value = self
                    .constructed_value(expression, target, arguments, &values)
                    .or_else(|| self.collection_operation(expression, &values))
                    .unwrap_or_else(|| self.expression_value(expression));
                let primitive = self.primitive_errors(expression, target, arguments, name);
                if !primitive.is_empty() && argument_success && !bounded_invocation {
                    errors.extend(self.at(
                        restrict(&primitive),
                        expression.range,
                        OriginKind::Operation,
                    ));
                }
                result.join(self.condition_value(value, &paths));
                success = true;
            }
        }
        if bounded_invocation && !success {
            result.join(self.expression_value(expression));
            success = true;
        }
        Evaluated {
            value: result,
            errors,
            success: argument_success && success,
        }
    }
}

// The semantic environment joins different exits. A later statement that
// reads one of its values may depend on which exit was taken, even when both
// sides have compatible Error guards. Keep the earlier witness in that case.
struct Reads<'a> {
    checked: &'a CheckedModule,
    env: &'a Environment,
    found: bool,
    payload_reads: HashSet<SourceRange>,
}

impl Reads<'_> {
    fn block(checked: &CheckedModule, block: &Block, env: &Environment) -> bool {
        if !env.is_empty() {
            let mut reads = Reads {
                checked,
                env,
                found: false,
                payload_reads: HashSet::new(),
            };
            ast_walk::block(&mut block.clone(), &mut reads);
            return reads.found;
        }
        false
    }

    fn statement(checked: &CheckedModule, statement: &Statement, env: &Environment) -> bool {
        if !env.is_empty() {
            let mut reads = Reads {
                checked,
                env,
                found: false,
                payload_reads: HashSet::new(),
            };
            ast_walk::statement(&mut statement.clone(), &mut reads);
            return reads.found;
        }
        false
    }

    fn ignore_payload_read(&mut self, expression: &Expression) {
        if let ExpressionKind::Name(path) = &expression.kind
            && let [name] = path.as_slice()
            && self.env.get(name).is_some_and(|value| {
                !matches!(value, Value::Unbound(_) | Value::Union(_) | Value::Bottom)
            })
            && !self.checked.native_record_reads.contains(&expression.range)
            && self
                .checked
                .expression_type(expression.range)
                .is_some_and(|ty| *ty == Type::String || ty.scalar().is_some())
        {
            self.payload_reads.insert(expression.range);
        }
    }
}

impl Visitor for Reads<'_> {
    fn enter_statement(&mut self, statement: &Statement) {
        // Reading an initialized scalar for the message or payload of a
        // directly thrown nominal Error cannot change the resulting tag.
        if self.found {
            return;
        }
        if matches!(statement.kind, StatementKind::Rethrow { .. }) {
            self.found = true;
        }
        let StatementKind::Throw(expression) = &statement.kind else {
            return;
        };
        let (ExpressionKind::Call { target, arguments }
        | ExpressionKind::GenericCall {
            target, arguments, ..
        }) = &expression.kind
        else {
            return;
        };
        if self.env.contains_key(&target[0])
            || !self
                .checked
                .source_types
                .get(&target.join("."))
                .is_some_and(|ty| ty.kind == "error")
        {
            return;
        }
        match arguments {
            Arguments::Positional(values) => values
                .iter()
                .for_each(|value| self.ignore_payload_read(value)),
            Arguments::Named(values) => values
                .iter()
                .for_each(|(_, value, _)| self.ignore_payload_read(value)),
        }
    }

    fn expression(&mut self, expression: &mut Expression) {
        if self.found || self.payload_reads.remove(&expression.range) {
            return;
        }
        let path = match &expression.kind {
            ExpressionKind::Name(path)
            | ExpressionKind::Call { target: path, .. }
            | ExpressionKind::GenericCall { target: path, .. }
            | ExpressionKind::Spawn { target: path, .. }
            | ExpressionKind::Bind { target: path, .. } => path,
            _ => return,
        };
        self.found |= path.first().and_then(|root| self.env.get(root)).is_some_and(|value| {
            path.len() != 1
                || !matches!(expression.kind, ExpressionKind::Name(_))
                || !(matches!(value, Value::Bool(Some(_)))
                    || matches!(value, Value::Error(tags) if tags.len() == 1 && matches!(tags.first(), Some(Tag::Named(_)))))
        });
    }
}

pub(super) fn arguments_independent(
    checked: &CheckedModule,
    function: &crate::CheckedFunction,
) -> bool {
    let declaration = &function.declaration;
    if declaration.parameters.is_empty() {
        return true;
    }
    let inputs = declaration
        .parameters
        .iter()
        .zip(&function.parameter_types)
        .map(|(parameter, ty)| {
            let value =
                tag(checked, ty).map_or(Value::Plain, |tag| Value::Error(BTreeSet::from([tag])));
            (parameter.name.clone(), value)
        })
        .collect();
    !Reads::block(checked, &declaration.body, &inputs)
}

fn join_env(env: &mut Environment, other: &Environment) {
    for (name, value) in env {
        if let Some(other) = other.get(name) {
            value.join(other.clone());
        }
    }
}
