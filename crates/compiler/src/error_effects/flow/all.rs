//! Collection paths are separate from outward failures. In particular a
//! condition that fails before a branch must not acquire that branch's errors.
use super::*;
use language::empty_word;

impl Analyzer<'_> {
    pub(super) fn try_flow(
        &mut self,
        range: SourceRange,
        all: bool,
        body: &Block,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        env: &mut Environment,
    ) -> Flow {
        let surrounding = self.all;
        self.all = all;
        let mut body = self.block(body, env);
        self.all = false;
        let saved_control = (
            body.normal,
            body.returns,
            body.breaks,
            body.continues,
            body.returned.clone(),
        );
        if all {
            let mut words = body.exit_words();
            let clean = words
                .iter()
                .any(|word| word.prefix.is_empty() && !word.longer);
            words.retain(|word| !word.prefix.is_empty() || word.longer);
            body.errors = words;
            body.collected = None;
            if !clean {
                body.normal = false;
                body.returns = false;
                body.breaks = false;
                body.continues = false;
                body.returned = Value::Bottom;
            }
        }
        let mut flow = self.handlers(range, body, handlers, None, env);
        if all && flow.normal {
            flow.normal = saved_control.0;
            flow.returns |= saved_control.1;
            flow.breaks |= saved_control.2;
            flow.continues |= saved_control.3;
            if saved_control.1 {
                flow.returned.join(saved_control.4);
            }
        }
        let flow = self.cleanup(flow, cleanup, env);
        self.all = surrounding;
        flow
    }

    pub(super) fn all_block(&mut self, block: &Block, env: &mut Environment) -> Flow {
        let outer = env.keys().cloned().collect::<BTreeSet<_>>();
        let mut active = empty_word();
        let mut finished = Language::new();
        let mut result = Flow::default();
        let mut exiting: Option<Environment> = None;
        for statement in &block.statements {
            if active.is_empty() {
                if self.check_dead_code() {
                    let dead = std::mem::replace(&mut self.dead, true);
                    self.statement(statement, &mut env.clone());
                    self.dead = dead;
                }
                continue;
            }
            let before = env.clone();
            let next = self.statement(statement, env);
            let independent = self.catch_trace.is_none()
                || !active.iter().any(|word| !word.prefix.is_empty())
                || !Reads::statement(self.checked, statement, &before);
            let non_error = next.exit_words();
            if next.returns || next.breaks || next.continues {
                finished.extend(language::concatenate_with_evidence(
                    &active,
                    &non_error,
                    self.bound,
                    independent,
                ));
                let saved = exiting.get_or_insert_with(|| before.clone());
                join_env(saved, env);
            }
            let mut continued = if next.normal {
                non_error
            } else {
                Language::new()
            };
            // A repeated await can fail again without appending the same Future
            // a second time. Until responsibility/alias information is shared
            // with EF-06, include both collection possibilities conservatively.
            if !next.errors.is_empty() && contains_await(statement) {
                continued.extend(empty_word());
            }
            continued.extend(next.errors.clone());
            if !next.errors.is_empty() {
                join_env(env, &before);
                if let StatementKind::Local { name, .. } = &statement.kind
                    && let Some(value) = env.get_mut(name)
                {
                    *value = Value::Unbound(Box::new(std::mem::take(value)));
                }
            }
            active =
                language::concatenate_with_evidence(&active, &continued, self.bound, independent);
            result.returns |= next.returns;
            result.breaks |= next.breaks;
            result.continues |= next.continues;
            result.returned.join(next.returned);
        }
        result.normal = !active.is_empty();
        finished.extend(active);
        result.collected = Some(finished);
        if let Some(exiting) = exiting {
            join_env(env, &exiting);
        }
        env.retain(|name, _| outer.contains(name));
        result
    }

    pub(super) fn all_while(
        &mut self,
        condition: &Expression,
        body: &Block,
        env: &mut Environment,
    ) -> Flow {
        let mut loop_env = env.clone();
        let mut body_flow = Flow::default();
        let mut condition_errors = Language::new();
        let mut may_enter = false;
        let mut may_exit = false;
        loop {
            let before = loop_env.clone();
            let value = self.expression(condition, &loop_env, false);
            let known = match value.value {
                Value::Bool(value) => value,
                _ => None,
            };
            condition_errors.extend(value.errors);
            may_exit |= value.success && known != Some(true);
            let dead = self.dead;
            self.dead |= !value.success || known == Some(false);
            let flow = self.block(body, &mut loop_env);
            self.dead = dead;
            if value.success && known != Some(false) {
                may_enter = true;
                body_flow.join(flow);
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
        let body_words = language::independent_elements(&body_flow.exit_words());
        let mut iterations = empty_word();
        if may_enter && (body_flow.normal || body_flow.continues) {
            loop {
                let next = concatenate(&iterations, &body_words, self.bound);
                let before = iterations.len();
                iterations.extend(next);
                if before == iterations.len() {
                    break;
                }
            }
        }
        let mut normal = if may_exit {
            iterations.clone()
        } else {
            Language::new()
        };
        if body_flow.breaks {
            normal.extend(concatenate(&iterations, &body_words, self.bound));
        }
        let mut words = normal.clone();
        if body_flow.returns {
            words.extend(concatenate(&iterations, &body_words, self.bound));
        }
        Flow {
            normal: !normal.is_empty(),
            returns: body_flow.returns,
            returned: body_flow.returned,
            errors: concatenate(&iterations, &condition_errors, self.bound),
            collected: Some(words),
            ..Flow::default()
        }
    }
}

fn contains_await(statement: &Statement) -> bool {
    struct Await(bool);
    impl Visitor for Await {
        fn expression(&mut self, expression: &mut Expression) {
            self.0 |= matches!(expression.kind, ExpressionKind::Await(_));
        }
    }
    let mut visitor = Await(false);
    ast_walk::statement(&mut statement.clone(), &mut visitor);
    visitor.0
}
