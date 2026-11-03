use super::*;
use rinzu_runtime::supplant::{
    Cancellation, Generation, SupplantCoordinator, SupplantError, Winner,
};

struct Pending {
    future: u64,
    generation: Generation,
    call: NativeCall,
    context: MainTaskContext,
    cancellation: CancellationState,
    diagnostic: Diagnostic,
}
struct Active {
    future: u64,
    generation: Generation,
    cancellation: CancellationState,
    diagnostic_monitor: rinzu_runtime::native_diagnostics::ActivityMonitor,
}
#[derive(Default)]
pub(in crate::source_async) struct Supplants {
    coordinator: Arc<SupplantCoordinator>,
    pending: Option<Pending>,
    active: Option<Active>,
    result_roots: BTreeMap<u64, OwnerFrame>,
}
impl Supplants {
    pub(in crate::source_async) fn reserve(
        &mut self,
        frame: &mut Frame,
        operation: &rinzu_compiler::Operation,
        context: MainTaskContext,
        cancellation: &CancellationState,
    ) -> Result<()> {
        #[cfg(feature = "test-allocation-probe")]
        let _preparation = crate::allocation_probe::preparation();
        let OperationKind::ReserveSupplant {
            reservation,
            callee,
            targets,
            arguments,
            callbacks,
            ..
        } = &operation.kind
        else {
            bail!("supplant reservation lost its IR plan")
        };
        ensure!(
            !frame.supplant_reservations.contains_key(reservation),
            "supplant reservation reused before consumption"
        );
        let target = if let Some(callee) = callee {
            let Some(Value::FunctionReference(target)) = frame.values.get(callee) else {
                bail!("supplant callee lost its closed reference")
            };
            ensure!(
                targets.contains(target),
                "supplant target left its checked set"
            );
            target
        } else {
            ensure!(targets.len() == 1, "direct supplant has no unique target");
            &targets[0]
        };
        let signature = context
            .source_native
            .bindings
            .entries
            .get(target)
            .ok_or_else(|| anyhow!("missing physical supplant binding: {target}"))?
            .0
            .clone();
        ensure!(
            signature.is_supplant && !signature.is_async,
            "supplant plan requires the native supplant effect"
        );
        let arguments = arguments
            .iter()
            .map(|argument| {
                frame
                    .values
                    .get(argument)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing supplant argument"))
            })
            .collect::<Result<Vec<_>>>()?;
        let callbacks = callbacks.clone();
        let future = {
            let mut shared = context
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let future =
                shared.admit_with_lease(context.child_scope, context.callback_admission)?;
            shared.operations.insert(future, "supplant");
            future
        };
        frame.supplant_reservations.insert(*reservation, future);
        let generation = match self.coordinator.reserve() {
            Ok(generation) => generation,
            Err(SupplantError::Busy) => {
                let diagnostic = frame.failure.diagnostic(
                    FailureCause::Error(ErrorKind::SupplantBusy),
                    operation.range,
                );
                context
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .complete(future, Err(Failure::new(diagnostic)))?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let diagnostic = frame.failure.diagnostic(
            FailureCause::Error(ErrorKind::NullNativeValue),
            operation.range,
        );
        let call = NativeCall::prepare(
            &signature,
            &callbacks,
            arguments,
            &context.owner,
            &context.source_native,
            frame.failure.diagnostic(
                FailureCause::Error(ErrorKind::NativeRecordBusy),
                operation.range,
            ),
            CallbackOrigin {
                cancellation,
                main: Some(&context),
            },
        );
        match call {
            Ok(call) => {
                call.bind_foreign(generation.bits())?;
                ensure!(
                    self.pending.is_none() && self.active.is_none(),
                    "supplant start slot diverged from its call record"
                );
                self.active = Some(Active {
                    future,
                    generation,
                    cancellation: cancellation.clone(),
                    diagnostic_monitor: context.owner.borrow().diagnostic_monitor().clone(),
                });
                self.pending = Some(Pending {
                    future,
                    generation,
                    call,
                    context,
                    cancellation: cancellation.clone(),
                    diagnostic,
                });
            }
            Err(error) => {
                let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                    return Err(error);
                };
                self.coordinator.fail_preparation(generation)?;
                self.coordinator.finish_settlement(generation)?;
                context
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .complete(future, Err(failure.0.clone()))?;
            }
        }
        Ok(())
    }
    pub(in crate::source_async) fn queue(&self, future: u64) -> Result<()> {
        if let Some(active) = &self.active
            && active.future == future
        {
            self.coordinator.queue(active.generation)?;
        }
        Ok(())
    }
    pub(in crate::source_async) fn cancel(&self, future: u64, cancellation: Cancellation) -> bool {
        let Some(active) = self
            .active
            .as_ref()
            .filter(|active| active.future == future)
        else {
            return false;
        };
        active.cancellation.requested.set(true);
        if cancellation == Cancellation::Shutdown {
            active.cancellation.shutdown_requested.set(true);
        }
        self.coordinator.cancel(active.generation, cancellation);
        active.diagnostic_monitor.request_shutdown();
        true
    }
    pub(in crate::source_async) fn retire_result(&mut self, future: u64) {
        self.result_roots.remove(&future);
    }
    pub(in crate::source_async) fn is_baseline(&self) -> bool {
        self.pending.is_none()
            && self.active.is_none()
            && self.result_roots.is_empty()
            && self.coordinator.snapshot().phase == rinzu_runtime::supplant::Phase::Idle
    }
    /// Called by the Main runtime only after the whole synchronous call chain
    /// returned. Taking the record releases the registry borrow before entry.
    pub(in crate::source_async) fn drive(registry: &Rc<RefCell<Self>>) -> Result<bool> {
        let (pending, coordinator) = {
            let mut registry = registry.borrow_mut();
            (registry.pending.take(), Arc::clone(&registry.coordinator))
        };
        let Some(mut pending) = pending else {
            return Ok(false);
        };
        if pending.cancellation.requested.get() {
            coordinator.cancel(
                pending.generation,
                if pending.cancellation.shutdown_requested.get() {
                    Cancellation::Shutdown
                } else {
                    Cancellation::Task
                },
            );
        }
        let preparation = if coordinator.snapshot().winner.is_none() {
            pending.call.prepare_records_at_entry()
        } else {
            Ok(())
        };
        let outcome = if let Err(error) = preparation {
            let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                return Err(error);
            };
            let failed = coordinator.fail_preparation(pending.generation)?;
            pending.call.abandon_before_entry()?;
            if failed {
                Err(failure.0.clone())
            } else {
                Err(Self::cancellation_failure(
                    coordinator
                        .snapshot()
                        .winner
                        .expect("cancelled entry has a winner"),
                    pending.diagnostic,
                ))
            }
        } else if coordinator.begin_entry(pending.generation)? {
            coordinator.entered(pending.generation)?;
            let dispatcher = pending
                .context
                .owner
                .borrow()
                .dispatcher
                .clone()
                .ok_or_else(|| anyhow!("supplant Main dispatcher is unavailable"))?;
            dispatcher.queue().begin_loop(pending.generation.bits());
            pending
                .context
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .events
                .push(SourceAsyncEvent::NativeEntered {
                    scope: pending.context.scope,
                    export: pending.call.signature.name.clone(),
                });
            pending.call.enter(&pending.context.source_native)?;
            dispatcher.queue().end_loop(pending.generation.bits());
            if pending.cancellation.requested.get() {
                coordinator.cancel(
                    pending.generation,
                    if pending.cancellation.shutdown_requested.get() {
                        Cancellation::Shutdown
                    } else {
                        Cancellation::Task
                    },
                );
            }
            coordinator.returned(pending.generation)?;
            pending
                .context
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .events
                .push(SourceAsyncEvent::NativeLeft {
                    scope: pending.context.scope,
                    export: pending.call.signature.name.clone(),
                });
            pending.call.close()?;
            pending.call.settle()?;
            let winner = coordinator
                .snapshot()
                .winner
                .expect("physical supplant return has a winner");
            if winner == Winner::Returned {
                match pending.call.read(pending.diagnostic) {
                    Ok(value) => Ok(value),
                    Err(error) => {
                        let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                            return Err(error);
                        };
                        Err(failure.0.clone())
                    }
                }
            } else {
                pending.call.phase = Phase::Read;
                pending.call.counters.read.fetch_add(1, Ordering::Relaxed);
                Err(Self::cancellation_failure(winner, pending.diagnostic))
            }
        } else {
            pending.call.abandon_before_entry()?;
            Err(Self::cancellation_failure(
                coordinator
                    .snapshot()
                    .winner
                    .expect("cancelled supplant has a winner"),
                pending.diagnostic,
            ))
        };
        let result_roots = outcome
            .as_ref()
            .ok()
            .map(|value| OwnerFrame::new(&pending.context.owner, std::slice::from_ref(value)));
        let errors = pending.context.owner.borrow_mut().take_callback_errors();
        if pending.call.phase == Phase::Read {
            pending.call.phase = Phase::Released;
        }
        drop(pending.call);
        coordinator.finish_settlement(pending.generation)?;
        {
            let mut registry = registry.borrow_mut();
            let active = registry
                .active
                .take()
                .ok_or_else(|| anyhow!("supplant record lost its start slot"))?;
            ensure!(
                active.future == pending.future && active.generation == pending.generation,
                "supplant record generation mismatch"
            );
            if let Some(roots) = result_roots {
                ensure!(
                    registry
                        .result_roots
                        .insert(pending.future, roots)
                        .is_none(),
                    "duplicate supplant result root"
                );
            }
        }
        let mut shared = pending
            .context
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        shared.caught_errors.extend(errors);
        shared.complete(pending.future, outcome)?;
        Ok(true)
    }
    fn cancellation_failure(winner: Winner, mut diagnostic: Diagnostic) -> Failure {
        diagnostic.cause = match winner {
            Winner::Cancelled => FailureCause::Cancelled,
            Winner::Shutdown => FailureCause::Shutdown,
            _ => native_fatal(86),
        };
        Failure::new(diagnostic)
    }
}
