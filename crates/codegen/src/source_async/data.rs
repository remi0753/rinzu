//! Each Domain's typed frames share its real Owner heap.
use super::*;
use rinzu_compiler::{SourceRange, StackRoot, Type};
use rinzu_runtime::data::{
    DataError, ManagedData, ManagedHandle, ManagedRoots, ManagedSlot, NativeRecordLayout,
    NativeUserDataEvidence, NativeUserDataInvocation, NativeUserDataRegistry, NativeUserDataScope,
};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, HeapError};
use rinzu_runtime::value::ValueType;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeapEvidence {
    pub objects: usize,
    pub roots: usize,
    pub collections: usize,
    pub moved_objects: usize,
    pub peak_objects: usize,
    pub peak_frames: usize,
    pub userdata: NativeUserDataEvidence,
    pub native_record_scratch: rinzu_runtime::data::NativeRecordScratchStats,
}

struct OwnerScope {
    active: bool,
    after: Option<SourceRange>,
    deferred: Option<SourceRange>,
}

pub(super) struct OwnerValues {
    pub(super) stdout: rinzu_runtime::stdio::Output,
    pub(super) tracer: rinzu_runtime::error_trace::TraceContext,
    transitions: Arc<rinzu_runtime::transition::TransitionMachine>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    monitor: Option<rinzu_runtime::native_diagnostics::ActivityMonitor>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) dispatcher: Option<Rc<rinzu_runtime::owner_dispatcher::OwnerDispatcher>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) dispatcher_context: Option<Box<super::dispatcher::Context>>,
    heap: DomainHeap,
    data: ManagedData,
    schema: crate::data_schema::DataSchema,
    frames: BTreeMap<u64, Vec<ManagedHandle>>,
    future_frames: BTreeMap<u64, Vec<u64>>,
    pub(super) future_state: std::sync::Weak<Mutex<SharedState>>,
    next_future_gc: usize,
    results: BTreeMap<u64, ManagedHandle>,
    next_frame: u64,
    evidence: HeapEvidence,
    userdata: NativeUserDataRegistry,
    userdata_roots: ManagedRoots,
    callback_errors: Vec<Failure>,
    scopes: HashMap<(u64, SourceRange), OwnerScope>,
}

fn checked<T>(result: std::result::Result<T, DataError>) -> Result<T> {
    match result {
        Err(
            DataError::AddressSpaceExhausted | DataError::Heap(HeapError::AddressSpaceExhausted),
        ) => rinzu_runtime::allocation::fatal(),
        other => other.map_err(|error| anyhow!("Owner typed data invariant: {error:?}")),
    }
}
pub(super) fn native_fatal(code: i32) -> ! {
    // SAFETY: an invalid internal storage lifetime or bookkeeping OOM cannot
    // run managed cleanup, allocate diagnostics or unwind through native code.
    rinzu_runtime::native_fatal::terminate(code)
}

impl OwnerValues {
    pub(super) fn enable_gc_stress(&mut self) {
        self.heap.enable_gc_stress();
    }

    pub(super) fn gc_stress(&self) -> bool {
        self.heap.gc_stress()
    }
    fn enter_scope(
        &mut self,
        frame: u64,
        scope: SourceRange,
        deferred: Option<SourceRange>,
    ) -> Result<()> {
        self.scopes
            .try_reserve(1)
            .unwrap_or_else(|_| native_fatal(71));
        ensure!(
            self.scopes
                .insert(
                    (frame, scope),
                    OwnerScope {
                        active: true,
                        after: None,
                        deferred,
                    }
                )
                .is_none(),
            "Owner lexical scope entered twice"
        );
        Ok(())
    }
    fn retire_scopes(
        &mut self,
        frame: u64,
        plan: &crate::source_failure::ScopeReleasePlan,
    ) -> Result<()> {
        // Cleanup failure may skip the pending ExitScope. Move its delayed
        // release to the remaining outer finally before reclaiming storage.
        for ((owner, _), scope) in &mut self.scopes {
            if *owner == frame
                && scope
                    .after
                    .is_some_and(|after| plan.ended_cleanups.contains(&after))
            {
                scope.after = plan.after;
            }
        }
        for range in &plan.scopes {
            if let Some(scope) = self.scopes.get_mut(&(frame, *range)) {
                scope.active = false;
                scope.after = plan.after.or(scope
                    .deferred
                    .filter(|after| !plan.ended_cleanups.contains(after)));
            }
        }
        self.scopes.retain(|(owner, _), scope| {
            if *owner != frame || scope.active || scope.after.is_some() {
                return true;
            }
            false
        });
        Ok(())
    }
    fn exit_scope(
        &mut self,
        frame: u64,
        range: SourceRange,
        after: Option<SourceRange>,
    ) -> Result<()> {
        self.retire_scopes(
            frame,
            &crate::source_failure::ScopeReleasePlan {
                scopes: vec![range],
                ended_cleanups: vec![range],
                after,
            },
        )
    }
    fn close_activation(&mut self, frame: u64) -> Result<()> {
        self.scopes.retain(|(owner, _), _scope| {
            if *owner != frame {
                return true;
            }
            false
        });
        Ok(())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_prepare<'a>(
        &mut self,
        arguments: impl Iterator<Item = (&'a Arc<NativeRecordLayout>, ManagedHandle)> + Clone,
    ) -> std::result::Result<
        (rinzu_runtime::data::NativeRecordLoan, usize),
        rinzu_runtime::data::NativeRecordLoanError,
    > {
        let before = self.data.native_record_loan_stats().targets;
        let loan = self
            .data
            .prepare_native_records_iter(&mut self.heap, arguments)?;
        Ok((loan, self.data.native_record_loan_stats().targets - before))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_pointer(
        &self,
        loan: rinzu_runtime::data::NativeRecordLoan,
        argument: usize,
    ) -> u64 {
        self.data
            .native_record_argument_pointer(&self.heap, loan, argument)
            .unwrap_or_else(|_| native_fatal(86)) as u64
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_enter(&mut self, loan: rinzu_runtime::data::NativeRecordLoan) {
        self.data
            .enter_native_records(&self.heap, loan)
            .unwrap_or_else(|_| native_fatal(86));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_returned(&mut self, loan: rinzu_runtime::data::NativeRecordLoan) {
        self.data
            .return_native_records(&self.heap, loan)
            .unwrap_or_else(|_| native_fatal(86));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_commit(&mut self, loan: rinzu_runtime::data::NativeRecordLoan) {
        self.data.commit_native_records(&mut self.heap, loan);
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_discard(&mut self, loan: rinzu_runtime::data::NativeRecordLoan) {
        self.data
            .discard_prepared_native_records(&mut self.heap, loan)
            .unwrap_or_else(|_| native_fatal(86));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_release(&mut self, loan: rinzu_runtime::data::NativeRecordLoan) {
        self.data
            .release_native_records(&mut self.heap, loan)
            .unwrap_or_else(|_| native_fatal(86));
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_record_busy(&mut self, mut diagnostic: Diagnostic) -> Result<Failure> {
        let handle = checked(self.data.create_error_value(
            &mut self.heap,
            &rinzu_runtime::error_value::ErrorValue::new(4, "native record is already in use"),
        ))?;
        diagnostic.cause = FailureCause::Error(ErrorKind::User);
        checked(
            self.data
                .first_error_source(&mut self.heap, handle, diagnostic.source),
        )?;
        let error = checked(self.data.lease_error(&self.heap, handle))?;
        let mut failure = Failure::with_managed(diagnostic, error);
        if let Some(trace) = self.tracer.capture(diagnostic.source) {
            failure.traces.insert(0, trace);
        }
        Ok(failure)
    }
    /// # Safety
    /// The returned pointer has this exact validated native record contract.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub unsafe fn receive_native_record(
        &mut self,
        layout: &rinzu_runtime::data::NativeRecordLayout,
        pointer: u64,
    ) -> std::result::Result<Value, rinzu_runtime::data::NativeRecordError> {
        // SAFETY: caller retains the native struct until all fields are copied.
        let handle = unsafe {
            self.data
                .receive_native_record(&mut self.heap, layout, pointer as *const u8)
        }?;
        Ok(self.allocated(layout.ty, handle))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn native_opaque_pointer(&self, value: &Value, identity: &str) -> Result<u64> {
        let index = self.schema.index(&Type::Native {
            name: identity.to_owned(),
            call_only: false,
        })?;
        let Value::Managed { ty, handle } = value else {
            bail!("native opaque argument is not in its Owner heap");
        };
        ensure!(*ty == index, "native opaque nominal type mismatch");
        checked(self.data.native_opaque_pointer(&self.heap, *handle, index))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn reserve_native_opaque(
        &mut self,
        identity: &str,
    ) -> Result<(usize, rinzu_runtime::data::OpaqueReservation)> {
        let ty = self.schema.index(&Type::Native {
            name: identity.to_owned(),
            call_only: false,
        })?;
        let reservation = checked(self.data.reserve_native_opaque(&mut self.heap, ty))?;
        self.evidence.peak_objects = self.evidence.peak_objects.max(self.heap.object_count());
        Ok((ty, reservation))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn receive_native_opaque(
        &mut self,
        ty: usize,
        reservation: &mut rinzu_runtime::data::OpaqueReservation,
        pointer: u64,
    ) -> std::result::Result<Value, rinzu_runtime::data::NativeOpaqueError> {
        let handle = self
            .data
            .receive_native_opaque(&mut self.heap, reservation, pointer)?;
        Ok(self.allocated(ty, handle))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn discard_native_opaque(
        &mut self,
        reservation: &mut rinzu_runtime::data::OpaqueReservation,
    ) -> Result<()> {
        checked(self.data.discard_native_opaque(&mut self.heap, reservation))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub unsafe fn receive_native_data(
        &mut self,
        ty: &Type,
        pointer: u64,
    ) -> std::result::Result<Value, rinzu_runtime::data::NativeInputError> {
        let index = self
            .schema
            .index(ty)
            .unwrap_or_else(|_| std::process::abort());
        let input = pointer as usize as *const u8;
        // SAFETY: the closed native result signature selects the one-pointer
        // contract; external storage stays readable until this owned copy ends.
        let handle = unsafe {
            match ty {
                Type::String => self.data.receive_native_string(&mut self.heap, input),
                Type::FixedBytes(length) => {
                    self.data
                        .receive_native_bytes(&mut self.heap, *length, input)
                }
                Type::Bytes => self
                    .data
                    .receive_native_byte_array(&mut self.heap, input.cast()),
                _ => std::process::abort(),
            }
        }?;
        Ok(self.allocated(index, handle))
    }

    pub fn new(module: &IrModule) -> Result<Rc<RefCell<Self>>> {
        Self::with_heap(module, DomainHeap::new(0x4d41_494e))
    }

    pub fn worker(module: &IrModule, domain: &mut WorkerDomain) -> Result<Rc<RefCell<Self>>> {
        let identity = domain.id().0;
        let heap = std::mem::replace(domain.heap_mut(), DomainHeap::new(identity));
        Self::with_heap(module, heap)
    }

    fn with_heap(module: &IrModule, heap: DomainHeap) -> Result<Rc<RefCell<Self>>> {
        let schema = crate::data_schema::DataSchema::new(module)?;
        let mut data = ManagedData::new(heap.domain_id());
        checked(data.configure_values(&heap, schema.value.clone()))?;
        checked(data.configure_sources(&heap, crate::error_sources::catalog(module)))?;
        let userdata = NativeUserDataRegistry::new(&heap);
        let transitions = Arc::new(rinzu_runtime::transition::TransitionMachine::new());
        transitions
            .activate_managed()
            .unwrap_or_else(|_| native_fatal(86));
        Ok(Rc::new(RefCell::new(Self {
            stdout: rinzu_runtime::stdio::Output::default(),
            tracer: rinzu_runtime::error_trace::TraceContext::from_environment(false),
            transitions,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            monitor: Some(rinzu_runtime::native_diagnostics::ActivityMonitor::new()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            dispatcher: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            dispatcher_context: None,
            heap,
            data,
            schema,
            frames: BTreeMap::new(),
            future_frames: BTreeMap::new(),
            future_state: std::sync::Weak::new(),
            next_future_gc: 1024,
            results: BTreeMap::new(),
            next_frame: 0,
            evidence: HeapEvidence::default(),
            userdata,
            userdata_roots: ManagedRoots::default(),
            callback_errors: Vec::new(),
            scopes: HashMap::new(),
        })))
    }

    fn allocated(&mut self, ty: usize, handle: ManagedHandle) -> Value {
        self.evidence.peak_objects = self.evidence.peak_objects.max(self.heap.object_count());
        Value::Managed { ty, handle }
    }

    pub fn error_list(&mut self, errors: &[rinzu_runtime::failure::Failure]) -> Result<Value> {
        let ty = self.schema.index(&Type::List(Box::new(Type::Error)))?;
        let mut slots = Vec::new();
        for failure in errors {
            for (index, _diagnostic) in std::iter::once(&failure.primary)
                .chain(&failure.cleanup)
                .enumerate()
            {
                let handle = if let Some(error) = failure.managed.get(&index) {
                    checked(self.data.leased_error(&self.heap, error))?
                } else {
                    checked(
                        self.data
                            .create_error(&mut self.heap, &failure.diagnostic_at(index).unwrap()),
                    )?
                };
                slots.push(ManagedSlot::Reference(handle));
            }
        }
        let handle = checked(self.data.create_composite(&mut self.heap, ty, None, &slots))?;
        Ok(self.allocated(ty, handle))
    }
    pub fn adopt(&mut self, value: Value, ty: &Type) -> Result<Value> {
        let index = self.schema.index(ty)?;
        let handle = match value {
            Value::String(text) => checked(self.data.create_string(&mut self.heap, &text))?,
            Value::Bytes(bytes) => {
                let length = match ty {
                    Type::FixedBytes(n) => *n,
                    Type::Bytes => bytes.len() as u64,
                    _ => bail!("not bytes"),
                };
                checked(self.data.create_bytes(&mut self.heap, length, &bytes))?
            }
            Value::Error(failure) => checked(self.data.create_error(&mut self.heap, &failure))?,
            Value::Managed { ty: actual, handle } => {
                ensure!(
                    actual == index,
                    "Owner value has a different canonical type: expected {ty} (index {index}), received index {actual}"
                );
                return Ok(Value::Managed { ty: actual, handle });
            }
            other => return Ok(other),
        };
        Ok(self.allocated(index, handle))
    }

    pub fn export(&self, value: &Value) -> Result<Value> {
        let Value::Managed { ty, handle } = value else {
            return Ok(value.clone());
        };
        Ok(
            match self.schema.value.ty(*ty).map_err(|e| anyhow!("{e:?}"))? {
                ValueType::String => Value::String(checked(self.data.string(&self.heap, *handle))?),
                ValueType::DynamicBytes => {
                    Value::Bytes(checked(self.data.dynamic_bytes(&self.heap, *handle))?)
                }
                ValueType::Bytes(length) => {
                    Value::Bytes(checked(self.data.bytes(&self.heap, *handle, *length))?)
                }
                ValueType::Error | ValueType::Other => {
                    Value::Error(checked(self.data.error(&self.heap, *handle))?)
                }
                _ => bail!("legacy native arguments cannot contain composite values"),
            },
        )
    }

    fn slot(&mut self, index: usize, value: &Value) -> Result<ManagedSlot> {
        let bits = match value {
            Value::Managed { ty, handle } => {
                ensure!(*ty == index, "composite child canonical type mismatch");
                handle.bits()
            }
            Value::Unit => 0,
            Value::Scalar(value) => value.bits(),
            Value::Future(future) => *future,
            _ => bail!("composite child is not prepared in its Owner heap"),
        };
        checked(self.schema.slot(index, bits))
    }

    pub fn transport_descriptor(&self, types: &[Type]) -> Result<TypedDescriptor> {
        Ok(TypedDescriptor::new(
            self.schema.value.clone(),
            types
                .iter()
                .map(|ty| self.schema.index(ty))
                .collect::<Result<Vec<_>>>()?,
        )?)
    }

    pub fn transport_snapshot(
        &mut self,
        types: &[Type],
        values: &[Value],
    ) -> Result<Vec<rinzu_runtime::value::Value>> {
        ensure!(
            types.len() == values.len(),
            "transport argument count mismatch"
        );
        types
            .iter()
            .zip(values)
            .map(|(ty, value)| {
                let index = self.schema.index(ty)?;
                let slot = self.slot(index, value)?;
                checked(self.data.snapshot(&self.heap, index, slot))
            })
            .collect()
    }

    pub fn transport_materialize(
        &mut self,
        types: &[Type],
        values: &[rinzu_runtime::value::Value],
    ) -> Result<Vec<Value>> {
        let indices = types
            .iter()
            .map(|ty| self.schema.index(ty))
            .collect::<Result<Vec<_>>>()?;
        let slots = checked(
            self.data
                .materialize_values(&mut self.heap, &indices, values),
        )?;
        indices
            .into_iter()
            .zip(slots)
            .map(|(ty, slot)| {
                Ok(match slot {
                    ManagedSlot::Scalar(value) => Value::Scalar(value),
                    ManagedSlot::Unit => Value::Unit,
                    ManagedSlot::Reference(handle) => self.allocated(ty, handle),
                    ManagedSlot::Opaque(_) => bail!("transport materialized an opaque slot"),
                })
            })
            .collect()
    }

    pub fn finish_worker(&mut self, domain: &mut WorkerDomain) -> Result<HeapEvidence> {
        ensure!(
            self.heap.domain_id() == domain.id().0,
            "Worker heap identity mismatch"
        );
        let evidence = self.finish()?;
        std::mem::swap(&mut self.heap, domain.heap_mut());
        Ok(evidence)
    }

    pub fn retain_result(&mut self, future: u64, value: &Value) {
        if let Value::Managed { handle, .. } = value {
            self.results.insert(future, *handle);
        }
    }
    pub fn release_result(&mut self, future: u64) {
        self.results.remove(&future);
    }

    pub fn finish(&mut self) -> Result<HeapEvidence> {
        if self.transitions.state() != rinzu_runtime::transition::ExecutionState::Managed {
            native_fatal(86);
        }
        ensure!(
            self.callback_errors.is_empty(),
            "callback observations were not transferred after physical return"
        );
        ensure!(
            self.frames.is_empty(),
            "Owner teardown has active or parked root frames"
        );
        ensure!(
            self.scopes.is_empty(),
            "Owner teardown has active lexical scopes"
        );

        self.evidence.userdata = self.userdata.finish(&self.heap);
        if self.userdata_roots.lease_count() != 0 {
            native_fatal(86);
        }
        if self.future_state.strong_count() != 0 {
            self.collect()?;
        }
        self.results.clear();
        checked(self.data.release_all(&mut self.heap))?;
        let report = self
            .heap
            .collect(CollectionKind::Full)
            .map_err(|e| anyhow!("{e:?}"))?;
        self.evidence.collections += 1;
        self.evidence.moved_objects += report.moved_objects;
        self.evidence.objects = self.heap.object_count();
        self.evidence.roots = self.data.root_count();
        self.evidence.native_record_scratch = self.data.native_record_scratch_stats();
        self.transitions
            .deactivate_managed()
            .unwrap_or_else(|_| native_fatal(86));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        drop(self.monitor.take());
        Ok(self.evidence)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn diagnostic_monitor(&self) -> &rinzu_runtime::native_diagnostics::ActivityMonitor {
        self.monitor.as_ref().unwrap_or_else(|| native_fatal(86))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn activity_metadata(&self) -> (u64, u64) {
        // SAFETY: pthread identity is native metadata, never dereferenced.
        let owner = unsafe { libc::pthread_self() as u64 };
        let depth = self
            .transitions
            .native_frame_depth()
            .unwrap_or_else(|_| native_fatal(86)) as u64
            + 1;
        (owner, depth)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn prepare_readiness(
        &self,
        symbol: &str,
        source: Option<SourcePosition>,
    ) -> rinzu_runtime::ready_work::ReadyReceipt {
        let symbol = std::ffi::CString::new(symbol).unwrap_or_else(|_| native_fatal(86));
        let source = source.unwrap_or(SourcePosition {
            file: 0,
            start_byte: 0,
            end_byte: 0,
        });
        let (owner, depth) = self.activity_metadata();
        self.diagnostic_monitor()
            .prepare_ready(&symbol, source, owner, depth)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn prepare_wait(
        &self,
        task: u64,
        dependency: u64,
        symbol: &str,
        source: Option<SourcePosition>,
    ) -> rinzu_runtime::native_diagnostics::WaitDependency {
        let symbol = std::ffi::CString::new(symbol).unwrap_or_else(|_| native_fatal(86));
        let source = source.unwrap_or(SourcePosition {
            file: 0,
            start_byte: 0,
            end_byte: 0,
        });
        let (owner, depth) = self.activity_metadata();
        self.diagnostic_monitor()
            .wait_for(task, dependency, &symbol, source, owner, depth)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn note_owner_turn(&self) {
        self.diagnostic_monitor().owner_turn();
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn note_shutdown_request(&self) {
        self.diagnostic_monitor().request_shutdown();
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn prepare_activity(
        &self,
        kind: rinzu_runtime::native_diagnostics::ActivityKind,
        symbol: &str,
        source: Option<SourcePosition>,
    ) -> rinzu_runtime::native_diagnostics::Activity {
        let symbol = std::ffi::CString::new(symbol).unwrap_or_else(|_| native_fatal(86));
        let (owner, depth) = self.activity_metadata();
        let source = source.unwrap_or(SourcePosition {
            file: 0,
            start_byte: 0,
            end_byte: 0,
        });
        if matches!(
            kind,
            rinzu_runtime::native_diagnostics::ActivityKind::Native
        ) {
            return self
                .diagnostic_monitor()
                .prepare_native(&symbol, source, owner, depth);
        }
        self.diagnostic_monitor()
            .prepare(kind, &symbol, source, owner, depth)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn is_managed_activation(&self) -> bool {
        self.transitions.state() == rinzu_runtime::transition::ExecutionState::Managed
    }
    pub(super) fn callback_scope(&self) -> rinzu_runtime::transition::CallbackScope {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.note_owner_turn();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if rinzu_runtime::callback_entry::is_foreign_execution() {
            // SAFETY: the common entry pins the exact capsule and admitted
            // invocation; no ordinary entry can authorize post-return work.
            return unsafe { self.transitions.admitted_callback_scope() }
                .unwrap_or_else(|_| native_fatal(86));
        }
        self.transitions
            .callback_scope()
            .unwrap_or_else(|_| native_fatal(86))
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn dispatcher_can_enter(&self) -> bool {
        matches!(
            self.transitions.state(),
            rinzu_runtime::transition::ExecutionState::Managed
                | rinzu_runtime::transition::ExecutionState::NativeSafe
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn dispatcher_scope(&self) -> Option<rinzu_runtime::transition::CallbackScope> {
        (self.transitions.state() == rinzu_runtime::transition::ExecutionState::NativeSafe)
            .then(|| self.callback_scope())
    }

    fn publish(&mut self, frame: u64, roots: Vec<ManagedHandle>) -> Result<()> {
        checked(self.data.validate_roots(&self.heap, roots.iter().copied()))?;
        *self
            .frames
            .get_mut(&frame)
            .ok_or_else(|| anyhow!("missing Owner root frame"))? = roots;
        let future_due = self.future_state.upgrade().is_some_and(|state| {
            state
                .try_lock()
                .is_ok_and(|shared| shared.future_scopes.len() >= self.next_future_gc)
        });
        if self.heap.collection_due() || future_due {
            self.collect()?;
        }
        Ok(())
    }

    pub(super) fn collect(&mut self) -> Result<()> {
        if let Some(state) = self.future_state.upgrade()
            && let Ok(mut shared) = state.try_lock()
        {
            let futures = self
                .future_frames
                .values()
                .flatten()
                .copied()
                .chain(shared.gc_waiters.keys().copied())
                .chain(
                    shared
                        .future_scopes
                        .keys()
                        .filter(|id| !shared.completed.contains_key(id))
                        .copied(),
                );
            let roots = self
                .frames
                .values()
                .flatten()
                .copied()
                .chain(self.userdata_roots.live());
            let reachable = checked(self.data.reachable_futures(
                &self.heap,
                roots,
                futures,
                &self.schema.future_types(),
                &self.results,
            ))?;
            let dead = shared
                .completed
                .keys()
                .filter(|future| !reachable.contains(future))
                .copied()
                .collect::<Vec<_>>();
            for future in dead {
                if shared.retire_unreferenced(future)? {
                    self.results.remove(&future);
                }
            }
            self.next_future_gc = shared.future_scopes.len().saturating_mul(2).max(1024);
        }
        let live = self
            .frames
            .values()
            .flatten()
            .chain(self.results.values())
            .copied()
            .chain(self.userdata_roots.live())
            .collect::<Vec<_>>();
        checked(self.data.retain_roots(&mut self.heap, live))?;
        let report = self
            .heap
            .collect(CollectionKind::Full)
            .map_err(|e| anyhow!("{e:?}"))?;
        self.evidence.collections += 1;
        self.evidence.moved_objects += report.moved_objects;
        Ok(())
    }

    pub fn native_userdata_enter(&mut self) -> NativeUserDataScope {
        self.userdata.enter_scope(&self.heap, &self.data)
    }

    pub fn native_userdata_prepare(
        &mut self,
        scope: NativeUserDataScope,
        identity: &str,
        value: &Value,
    ) -> Result<u64> {
        let expected = self.schema.index(&Type::Named(identity.into()))?;
        let Value::Managed { ty, handle } = value else {
            native_fatal(86);
        };
        if *ty != expected {
            native_fatal(86);
        }
        Ok(self
            .userdata
            .prepare(
                &self.heap,
                &self.data,
                &mut self.userdata_roots,
                scope,
                expected,
                *handle,
            )
            .bits())
    }

    pub fn native_userdata_open(&mut self, scope: NativeUserDataScope) {
        self.userdata.open(&self.heap, scope);
    }
    pub fn native_userdata_expect_result(
        &mut self,
        scope: NativeUserDataScope,
        identity: &str,
    ) -> Result<()> {
        let ty = self.schema.index(&Type::Named(identity.into()))?;
        self.userdata
            .expect_result(&self.heap, &self.data, scope, ty);
        Ok(())
    }
    pub fn native_userdata_receive_result(
        &mut self,
        scope: NativeUserDataScope,
        identity: &str,
        pointer: u64,
    ) -> std::result::Result<Value, ErrorKind> {
        let ty = self
            .schema
            .index(&Type::Named(identity.into()))
            .unwrap_or_else(|_| native_fatal(86));
        let handle = self
            .userdata
            .receive_result(&self.heap, &self.data, scope, ty, pointer as *const u8)
            .map_err(|rinzu_runtime::data::NativeUserDataError::Null| ErrorKind::NullNativeValue)?;
        Ok(Value::Managed { ty, handle })
    }
    pub fn native_userdata_finish_result(&mut self, scope: NativeUserDataScope) {
        self.userdata
            .finish_result(&self.heap, &mut self.userdata_roots, scope);
    }

    pub fn native_userdata_returned(&mut self, scope: NativeUserDataScope) {
        self.userdata
            .returned(&self.heap, &mut self.userdata_roots, scope);
    }

    pub fn native_userdata_discard(&mut self, scope: NativeUserDataScope) {
        self.userdata
            .discard_prepared(&self.heap, &mut self.userdata_roots, scope);
    }

    pub fn native_userdata_admit(
        &mut self,
        scope: NativeUserDataScope,
    ) -> NativeUserDataInvocation {
        self.userdata.admit(&self.heap, scope)
    }

    pub fn record_callback_error(&mut self, failure: Failure) {
        self.callback_errors
            .try_reserve(1)
            .unwrap_or_else(|_| native_fatal(71));
        self.callback_errors.push(failure);
    }
    pub fn take_callback_errors(&mut self) -> Vec<Failure> {
        std::mem::take(&mut self.callback_errors)
    }

    pub fn native_userdata_resolve(
        &mut self,
        invocation: NativeUserDataInvocation,
        identity: &str,
        pointer: u64,
    ) -> std::result::Result<Value, ErrorKind> {
        let ty = self
            .schema
            .index(&Type::Named(identity.into()))
            .unwrap_or_else(|_| native_fatal(86));
        let handle = self
            .userdata
            .resolve(&self.heap, &self.data, invocation, ty, pointer as *const u8)
            .map_err(|rinzu_runtime::data::NativeUserDataError::Null| ErrorKind::NullNativeValue)?;
        Ok(Value::Managed { ty, handle })
    }

    pub fn native_userdata_settle(&mut self, invocation: NativeUserDataInvocation) {
        self.userdata
            .settle(&self.heap, &mut self.userdata_roots, invocation);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub unsafe fn native_length(
        &mut self,
        kind: rinzu_runtime::callback_entry::LengthKind,
        input: *const u8,
    ) -> Result<u64> {
        use rinzu_runtime::callback_entry::LengthKind;
        let handle = match kind {
            // SAFETY: this synchronous callback was admitted on the heap Owner;
            // the receiver copies the complete signature-selected input first.
            LengthKind::Bytes(length) => unsafe {
                self.data
                    .receive_native_bytes(&mut self.heap, length, input)
            },
            // SAFETY: String is one native NUL-terminated pointer; core length
            // declares no fallback, so unhandled input Error is fatal.
            LengthKind::Utf8 => unsafe { self.data.receive_native_string(&mut self.heap, input) },
        }
        .unwrap_or_else(|_| std::process::abort());
        let frame = self.next_frame;
        self.next_frame = self
            .next_frame
            .checked_add(1)
            .unwrap_or_else(|| std::process::abort());
        self.frames.insert(frame, vec![handle]);
        self.evidence.peak_frames = self.evidence.peak_frames.max(self.frames.len());
        self.evidence.peak_objects = self.evidence.peak_objects.max(self.heap.object_count());
        self.collect()?;
        let result = match kind {
            LengthKind::Bytes(length) => {
                checked(self.data.byte_length(&self.heap, handle, length))?
            }
            LengthKind::Utf8 => checked(self.data.string_length(&self.heap, handle))? as u64,
        };
        self.frames.remove(&frame);
        self.collect()?;
        Ok(result)
    }

    pub fn callback_capture_variant(&mut self, value: &Value, expected: &Type) -> Result<usize> {
        let Value::Managed { ty, handle } = value else {
            bail!("callback capture is not managed")
        };
        ensure!(
            *ty == self.schema.index(expected)?,
            "callback capture type mismatch"
        );
        let children = checked(self.schema.child_types(*ty, None, 1))?;
        let ManagedSlot::Reference(environment) =
            checked(self.data.project(&mut self.heap, *handle, *ty, None, 0))?
        else {
            bail!("callback capture environment is not a reference")
        };
        Ok(checked(self.data.enum_variant(&self.heap, environment, children[0]))? as usize)
    }

    pub fn enum_variant(&self, value: &Value) -> Result<u32> {
        let Value::Managed { ty, handle } = value else {
            bail!("switch value is not a managed enum");
        };
        checked(self.data.enum_variant(&self.heap, *handle, *ty))
    }

    fn thrown_error(
        &mut self,
        frame: &Frame,
        operation: &rinzu_compiler::Operation,
        handle: ManagedHandle,
    ) -> Result<anyhow::Error> {
        let first = checked(self.data.error_source(&self.heap, handle))?.is_none();
        let mut diagnostic = frame
            .failure
            .diagnostic(FailureCause::Error(ErrorKind::User), operation.range);
        diagnostic.source = checked(self.data.first_error_source(
            &mut self.heap,
            handle,
            diagnostic.source,
        ))?;
        let error = checked(self.data.lease_error(&self.heap, handle))?;
        let mut failure = Failure::with_managed(diagnostic, error);
        let trace = checked(self.data.error_trace(&self.heap, handle))?;
        let trace = trace.or_else(|| {
            first
                .then(|| frame.failure.tracer().capture(diagnostic.source))
                .flatten()
        });
        if let Some(trace) = trace {
            let trace = checked(self.data.first_error_trace(&mut self.heap, handle, trace))?;
            failure.traces.insert(0, trace);
        }
        Ok(anyhow!(ManagedFailure(failure)))
    }

    fn operation(
        &mut self,
        frame: &Frame,
        operation: &rinzu_compiler::Operation,
        root_frame: u64,
    ) -> Result<Option<Value>> {
        let get = |id: ValueId| {
            frame
                .values
                .get(&id)
                .ok_or_else(|| anyhow!("missing typed data operand"))
        };
        let result_type = || {
            self.schema
                .index(&operation.result.as_ref().expect("typed result").ty)
        };
        let value = match &operation.kind {
            OperationKind::ThrowError { value, .. } => {
                let Value::Managed { handle, .. } = get(*value)? else {
                    bail!("throw operand is not managed Error")
                };
                return Err(self.thrown_error(frame, operation, *handle)?);
            }
            OperationKind::ErrorIs { operand, target } => {
                let Value::Managed { handle, .. } = get(*operand)? else {
                    bail!("is operand is not managed Error")
                };
                let target = self.schema.index(target)?;
                let matched = checked(self.data.error_is(&self.heap, *handle, target))?;
                Value::Scalar(ScalarValue::from_bits(ScalarKind::Bool, u64::from(matched)).unwrap())
            }
            OperationKind::ErrorCast { operand } => {
                let ty = result_type()?;
                let Value::Managed { handle, .. } = get(*operand)? else {
                    bail!("Error cast operand is not managed")
                };
                let actual = checked(self.data.error_record_type(&self.heap, *handle))?;
                ensure!(
                    matches!(self.schema.value.ty(ty), Ok(ValueType::Error)) || actual == Some(ty),
                    "Error cast identity mismatch"
                );
                Value::Managed {
                    ty,
                    handle: *handle,
                }
            }
            OperationKind::ErrorField { error, field } => {
                let ty = result_type()?;
                let Value::Managed { handle, .. } = get(*error)? else {
                    bail!("Error metadata operand is not managed")
                };
                let slot = checked(self.data.error_metadata(
                    &mut self.heap,
                    *handle,
                    *field as usize,
                    ty,
                ))?;
                match slot {
                    ManagedSlot::Reference(handle) => Value::Managed { ty, handle },
                    _ => bail!("Error metadata is not a managed value"),
                }
            }
            OperationKind::ErrorMatch {
                errors,
                types,
                rest,
            } => {
                let Value::Managed { ty, handle } = get(*errors)? else {
                    bail!("catch list is not managed")
                };
                let types = types
                    .iter()
                    .map(|ty| self.schema.index(ty))
                    .collect::<Result<Vec<_>>>()?;
                let matched = checked(
                    self.data
                        .match_error_list(&self.heap, *handle, *ty, &types, *rest),
                )?;
                Value::Scalar(ScalarValue::from_bits(ScalarKind::Bool, u64::from(matched)).unwrap())
            }
            OperationKind::ErrorIndex { errors, index } => {
                let Value::Managed { ty, handle } = get(*errors)? else {
                    bail!("catch list is not managed")
                };
                let handle = match checked(self.data.project(
                    &mut self.heap,
                    *handle,
                    *ty,
                    None,
                    *index as usize,
                ))? {
                    ManagedSlot::Reference(handle) => handle,
                    _ => bail!("catch list contains a non-Error value"),
                };
                let ty = result_type()?;
                ensure!(
                    matches!(self.schema.value.ty(ty), Ok(ValueType::Error))
                        || checked(self.data.error_record_type(&self.heap, handle))? == Some(ty),
                    "catch binding identity mismatch"
                );
                Value::Managed { ty, handle }
            }
            OperationKind::ErrorRest { errors, start } => {
                let Value::Managed { ty, handle } = get(*errors)? else {
                    bail!("catch list is not managed")
                };
                let handle = checked(self.data.error_list_rest(
                    &mut self.heap,
                    *handle,
                    *ty,
                    *start as usize,
                ))?;
                self.allocated(result_type()?, handle)
            }
            OperationKind::EnterScope { scope } => {
                self.enter_scope(root_frame, *scope, frame.failure.scope_finally(*scope))?;
                Value::Unit
            }
            OperationKind::ExitScope {
                scope,
                deferred_finally,
            } => {
                self.exit_scope(
                    root_frame,
                    *scope,
                    deferred_finally
                        .last()
                        .map(|id| frame.failure.cleanup_range(*id)),
                )?;
                Value::Unit
            }
            OperationKind::GcSafepoint { stack_map, .. } => {
                let mut roots = Vec::new();
                let mut futures = Vec::new();
                for root in &stack_map.roots {
                    let value = match root {
                        StackRoot::Value { value, .. } => get(*value)?,
                        StackRoot::Local { name, .. } => {
                            let Some(value) = frame.locals.get(name) else {
                                continue;
                            };
                            value
                        }
                    };
                    if let Value::Managed { handle, .. } = value {
                        roots.push(*handle);
                    } else if let Value::Future(future) = value {
                        futures.push(*future);
                    }
                }
                if let Some((_, Value::Managed { handle, .. })) = &frame.deferred_return {
                    roots.push(*handle);
                }
                if let Some((_, Value::Future(future))) = &frame.deferred_return {
                    futures.push(*future);
                }
                self.future_frames.insert(root_frame, futures);
                self.publish(root_frame, roots)?;
                Value::Unit
            }
            OperationKind::Constant(ConstantValue::String(_) | ConstantValue::Bytes(_)) => {
                let OperationKind::Constant(value) = &operation.kind else {
                    unreachable!()
                };
                self.adopt(
                    constant(value, &operation.result.as_ref().unwrap().ty)?,
                    &operation.result.as_ref().unwrap().ty,
                )?
            }
            OperationKind::FormatString { parts, .. } => {
                let ty = result_type()?;
                let parts = parts
                    .iter()
                    .map(|(id, ty)| {
                        let index = self.schema.index(ty)?;
                        let slot = match get(*id)? {
                            Value::Handler(_) | Value::FunctionReference(_) => {
                                ManagedSlot::Opaque(0)
                            }
                            value => self.slot(index, value)?,
                        };
                        Ok((index, slot))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let formatted =
                    self.data
                        .format_string(&mut self.heap, &self.schema.display, &parts);
                let handle = match formatted {
                    Err(DataError::Thrown(value)) => {
                        let handle = checked(self.data.create_error_value(&mut self.heap, &value))?;
                        return Err(self.thrown_error(frame, operation, handle)?);
                    }
                    other => checked(other)?,
                };
                self.allocated(ty, handle)
            }
            OperationKind::Stdout {
                operand,
                ty,
                newline,
                ..
            } => {
                let index = self.schema.index(ty)?;
                let slot = match get(*operand)? {
                    Value::Handler(_) | Value::FunctionReference(_) => ManagedSlot::Opaque(0),
                    value => self.slot(index, value)?,
                };
                let bytes =
                    checked(
                        self.data
                            .display(&self.heap, &self.schema.display, index, slot),
                    )?;
                let native = self
                    .transitions
                    .native_scope(rinzu_runtime::transition::RootPublication {
                        stack_map_id: root_frame,
                        root_count: self.frames.values().map(Vec::len).sum(),
                    })
                    .map_err(|error| anyhow!("stdout native transition: {error}"))?;
                let result = self.stdout.write(&bytes, *newline);
                drop(native);
                if result.is_err() {
                    return Err(crate::source_failure::ManagedFailure(Failure::new(
                        frame
                            .failure
                            .diagnostic(FailureCause::Error(ErrorKind::Io), operation.range),
                    ))
                    .into());
                }
                Value::Unit
            }
            OperationKind::DataOperation {
                kind,
                arguments,
                argument_types,
                ..
            } => {
                let result = result_type()?;
                let types = argument_types
                    .iter()
                    .map(|ty| self.schema.index(ty))
                    .collect::<Result<Vec<_>>>()?;
                let slots = arguments
                    .iter()
                    .zip(&types)
                    .map(|(id, ty)| self.slot(*ty, get(*id)?))
                    .collect::<Result<Vec<_>>>()?;
                let outcome = self
                    .data
                    .operation(&mut self.heap, *kind, result, &types, &slots);
                let slot = match outcome {
                    Err(DataError::Panic(message)) => {
                        let native = self
                            .transitions
                            .native_scope(rinzu_runtime::transition::RootPublication {
                                stack_map_id: root_frame,
                                root_count: self.frames.values().map(Vec::len).sum(),
                            })
                            .unwrap_or_else(|_| native_fatal(86));
                        rinzu_runtime::stdio::write_panic(&message);
                        drop(native);
                        return Err(crate::source_failure::ManagedFailure(Failure::new(
                            frame
                                .failure
                                .diagnostic(FailureCause::Panic, operation.range),
                        ))
                        .into());
                    }
                    Err(DataError::Thrown(value)) => {
                        let handle = checked(self.data.create_error_value(&mut self.heap, &value))?;
                        return Err(self.thrown_error(frame, operation, handle)?);
                    }
                    Err(DataError::CollectionOverflow {
                        operation: kind,
                        current,
                        added,
                    }) => {
                        let handle = checked(self.schema.collection_overflow(
                            &mut self.data,
                            &mut self.heap,
                            kind,
                            current,
                            added,
                        ))?;
                        return Err(self.thrown_error(frame, operation, handle)?);
                    }
                    other => checked(other)?,
                };
                match slot {
                    ManagedSlot::Scalar(value) => Value::Scalar(value),
                    ManagedSlot::Unit => Value::Unit,
                    ManagedSlot::Reference(handle) => Value::Managed { ty: result, handle },
                    ManagedSlot::Opaque(_) => bail!("data intrinsic produced opaque result"),
                }
            }
            OperationKind::CoreLength { operand, kind } => {
                let Value::Managed { ty, handle } = get(*operand)? else {
                    bail!("length operand is not in Owner heap");
                };
                let length = match kind {
                    rinzu_compiler::CoreLengthKind::Utf8 => {
                        checked(self.data.string(&self.heap, *handle))?.len() as u64
                    }
                    rinzu_compiler::CoreLengthKind::Bytes => {
                        let length =
                            match self.schema.value.ty(*ty).map_err(|e| anyhow!("{e:?}"))? {
                                ValueType::Bytes(n) => *n,
                                ValueType::DynamicBytes => u64::MAX,
                                _ => bail!("not bytes"),
                            };
                        checked(self.data.byte_length(&self.heap, *handle, length))?
                    }
                };
                Value::Scalar(ScalarValue::from_bits(ScalarKind::U64, length).expect("CSize"))
            }
            OperationKind::BytesPad { operand, from, to } => {
                let ty = result_type()?;
                let Value::Managed { handle, .. } = get(*operand)? else {
                    bail!("padding operand is not managed");
                };
                let mut bytes = checked(self.data.bytes(&self.heap, *handle, *from))?;
                let length =
                    usize::try_from(*to).unwrap_or_else(|_| rinzu_runtime::allocation::fatal());
                bytes
                    .try_reserve_exact(length - bytes.len())
                    .unwrap_or_else(|_| rinzu_runtime::allocation::fatal());
                bytes.resize(length, 0);
                let handle = checked(self.data.create_bytes(&mut self.heap, *to, &bytes))?;
                self.allocated(ty, handle)
            }
            OperationKind::ErrorMessage { error } => {
                let ty = result_type()?;
                let Value::Managed { handle, .. } = get(*error)? else {
                    bail!("Error operand is not managed");
                };
                let handle = checked(self.data.error_message(&mut self.heap, *handle))?;
                Value::Managed { ty, handle }
            }
            OperationKind::Construct {
                arguments, variant, ..
            } => {
                let ty = result_type()?;
                let types = checked(self.schema.child_types(ty, *variant, arguments.len()))?;
                let slots = types
                    .into_iter()
                    .zip(arguments)
                    .map(|(ty, id)| self.slot(ty, get(*id)?))
                    .collect::<Result<Vec<_>>>()?;
                let handle =
                    checked(
                        self.data
                            .create_composite(&mut self.heap, ty, *variant, &slots),
                    )?;
                self.allocated(ty, handle)
            }
            OperationKind::StoreField {
                operand,
                field,
                value,
                ..
            } => {
                let Value::Managed { ty, handle } = get(*operand)? else {
                    bail!("state receiver is not managed")
                };
                let child = match self
                    .schema
                    .value
                    .ty(*ty)
                    .map_err(|e| anyhow!("state schema: {e:?}"))?
                {
                    rinzu_runtime::value::ValueType::State { fields, .. } => *fields
                        .get(*field as usize)
                        .ok_or_else(|| anyhow!("state field"))?,
                    _ => bail!("not state"),
                };
                let slot = self.slot(child, get(*value)?)?;
                checked(self.data.set_state_field(
                    &mut self.heap,
                    *handle,
                    *ty,
                    *field as usize,
                    slot,
                ))?;
                Value::Unit
            }
            OperationKind::ListElement { operand, index } => {
                let result = result_type()?;
                let Value::Managed { ty, handle } = get(*operand)? else {
                    bail!("List projection operand is not managed");
                };
                let index = get(*index)?
                    .scalar()
                    .ok_or_else(|| anyhow!("List index is not scalar"))?;
                let index = usize::try_from(index.bits() as i64)?;
                match checked(self.data.project(&mut self.heap, *handle, *ty, None, index))? {
                    ManagedSlot::Scalar(value) => Value::Scalar(value),
                    ManagedSlot::Unit => Value::Unit,
                    ManagedSlot::Reference(handle) => Value::Managed { ty: result, handle },
                    ManagedSlot::Opaque(bits)
                        if matches!(operation.result.as_ref().unwrap().ty, Type::Future(_)) =>
                    {
                        Value::Future(bits)
                    }
                    ManagedSlot::Opaque(_) => {
                        bail!("opaque List projection requires its typed executable backend")
                    }
                }
            }
            OperationKind::Field { operand, field, .. }
            | OperationKind::EnumPayload { operand, field, .. } => {
                let result = result_type()?;
                let Value::Managed { ty, handle } = get(*operand)? else {
                    bail!("projection operand is not managed");
                };
                let variant = match &operation.kind {
                    OperationKind::EnumPayload { variant, .. } => Some(*variant),
                    _ => None,
                };
                let projected =
                    self.data
                        .project(&mut self.heap, *handle, *ty, variant, *field as usize);
                let projected = match projected {
                    Err(DataError::Thrown(value)) => {
                        let handle = checked(self.data.create_error_value(&mut self.heap, &value))?;
                        return Err(self.thrown_error(frame, operation, handle)?);
                    }
                    other => checked(other)?,
                };
                match projected {
                    ManagedSlot::Scalar(value) => Value::Scalar(value),
                    ManagedSlot::Unit => Value::Unit,
                    ManagedSlot::Reference(handle) => Value::Managed { ty: result, handle },
                    ManagedSlot::Opaque(bits)
                        if matches!(operation.result.as_ref().unwrap().ty, Type::Future(_)) =>
                    {
                        Value::Future(bits)
                    }
                    ManagedSlot::Opaque(_) => {
                        bail!("opaque projection requires its typed executable backend")
                    }
                }
            }
            OperationKind::Binary {
                operator,
                left,
                right,
                ..
            } if matches!(get(*left)?, Value::Managed { .. }) => {
                let Value::Managed { ty, .. } = get(*left)? else {
                    unreachable!()
                };
                ensure!(
                    matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual),
                    "managed binary operation is not equality"
                );
                let left = self.slot(*ty, get(*left)?)?;
                let right = self.slot(*ty, get(*right)?)?;
                let equal = checked(self.data.values_equal(&self.heap, *ty, left, right))?;
                Value::Scalar(ScalarValue::from_bool(
                    equal ^ (*operator == BinaryOperator::NotEqual),
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
}

pub(super) struct OwnerFrame {
    pub owner: Rc<RefCell<OwnerValues>>,
    identity: u64,
}
impl OwnerFrame {
    pub(super) fn native_scope(&self) -> rinzu_runtime::transition::NativeScope {
        let owner = self.owner.borrow();
        owner
            .transitions
            .native_scope(rinzu_runtime::transition::RootPublication {
                stack_map_id: self.identity,
                root_count: owner.frames.values().map(Vec::len).sum(),
            })
            .unwrap_or_else(|_| native_fatal(86))
    }
    pub fn retire_scopes(&self, plan: &crate::source_failure::ScopeReleasePlan) -> Result<()> {
        self.owner.borrow_mut().retire_scopes(self.identity, plan)
    }
    pub fn close_scopes(&self) -> Result<()> {
        self.owner.borrow_mut().close_activation(self.identity)
    }
    pub(super) fn new(owner: &Rc<RefCell<OwnerValues>>, arguments: &[Value]) -> Self {
        let mut data = owner.borrow_mut();
        let identity = data.next_frame;
        data.next_frame += 1;
        data.frames.insert(
            identity,
            arguments
                .iter()
                .filter_map(|value| match value {
                    Value::Managed { handle, .. } => Some(*handle),
                    _ => None,
                })
                .collect(),
        );
        data.future_frames.insert(
            identity,
            arguments
                .iter()
                .filter_map(|value| {
                    if let Value::Future(id) = value {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect(),
        );
        data.evidence.peak_frames = data.evidence.peak_frames.max(data.frames.len());
        Self {
            owner: Rc::clone(owner),
            identity,
        }
    }
    pub fn hold(&self, value: &Value) {
        if let Value::Managed { handle, .. } = value {
            self.owner
                .borrow_mut()
                .frames
                .get_mut(&self.identity)
                .expect("active root frame")
                .push(*handle);
        } else if let Value::Future(future) = value {
            self.owner
                .borrow_mut()
                .future_frames
                .get_mut(&self.identity)
                .expect("active Future root frame")
                .push(*future);
        }
    }
}
impl Drop for OwnerFrame {
    fn drop(&mut self) {
        let mut owner = self.owner.borrow_mut();
        owner
            .close_activation(self.identity)
            .unwrap_or_else(|_| native_fatal(86));
        owner.frames.remove(&self.identity);
        owner.future_frames.remove(&self.identity);
    }
}
impl Frame {
    pub fn with_owner(mut self, owner: &Rc<RefCell<OwnerValues>>) -> Self {
        self.failure.set_tracer(owner.borrow().tracer.clone());
        self.data_roots = Some(OwnerFrame::new(owner, &self.arguments));
        self
    }
    pub(super) fn data_operation(
        &self,
        operation: &rinzu_compiler::Operation,
    ) -> Result<Option<Value>> {
        self.data_roots
            .as_ref()
            .map(|roots| {
                roots
                    .owner
                    .borrow_mut()
                    .operation(self, operation, roots.identity)
            })
            .transpose()
            .map(Option::flatten)
            .map_err(|error| error.context(format!("typed operation {:?}", operation.kind)))
    }
    pub(super) fn store_result(&mut self, id: ValueId, value: Value, ty: &Type) -> Result<()> {
        let value = match &self.data_roots {
            Some(roots) => roots.owner.borrow_mut().adopt(value, ty)?,
            None => value,
        };
        self.values.insert(id, value);
        Ok(())
    }
    pub(super) fn export(&self, value: &Value) -> Result<Value> {
        match &self.data_roots {
            Some(roots) => roots.owner.borrow().export(value),
            None => Ok(value.clone()),
        }
    }
}
