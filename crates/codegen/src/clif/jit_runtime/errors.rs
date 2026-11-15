use super::*;
use rinzu_runtime::data::ManagedSlot;
use rinzu_runtime::value::ValueType;

pub(super) extern "C" fn error_data(
    context: *mut BaselineRuntime,
    operation: i64,
    ty: i64,
    value: i64,
    argument: i64,
    types: *const u64,
    count: i64,
) -> i64 {
    with_runtime(context, 0, |runtime| {
        let before = runtime.heap.object_count();
        let result = (|| -> std::result::Result<u64, DataError> {
            let ty = usize::try_from(ty).map_err(|_| DataError::InvalidEncoding)?;
            let handle =
                || ManagedHandle::from_bits(value as u64).ok_or(DataError::InvalidEncoding);
            let index = || usize::try_from(argument).map_err(|_| DataError::InvalidEncoding);
            match operation {
                8 => {
                    runtime.error_groups.enter(argument as u64);
                    Ok(0)
                }
                9 => {
                    let failure = runtime
                        .pending
                        .observe_error()
                        .ok_or(DataError::NotRecoverable)?;
                    runtime
                        .error_groups
                        .record(argument as u64, None, failure)
                        .map_err(|_| DataError::InvalidEncoding)?;
                    Ok(0)
                }
                10 => {
                    let errors = runtime
                        .error_groups
                        .finish(argument as u64)
                        .map_err(|_| DataError::InvalidEncoding)?;
                    let incoming = runtime
                        .pending
                        .replace_cleanup_incoming(None)
                        .map_err(|_| DataError::InvalidEncoding)?;
                    let (errors, incoming) =
                        rinzu_runtime::error_groups::settle(errors, incoming, value & 1 != 0);
                    runtime
                        .pending
                        .replace_cleanup_incoming(incoming)
                        .map_err(|_| DataError::InvalidEncoding)?;
                    runtime
                        .caught_errors
                        .extend(errors.iter().map(Failure::diagnostics_only));
                    Ok(runtime
                        .data
                        .catch_error_list(&mut runtime.heap, ty, &errors, value & 2 != 0)?
                        .bits())
                }
                11 => {
                    if value == 0 {
                        runtime.pending.fail(Diagnostic {
                            cause: FailureCause::Error(ErrorKind::UnboundValue),
                            source: runtime.current_source,
                        });
                    }
                    Ok(0)
                }
                0 => {
                    let handle = handle()?;
                    let actual = runtime.data.error_record_type(&runtime.heap, handle)?;
                    if !matches!(runtime.data_schema.value.ty(ty)?, ValueType::Error)
                        && actual != Some(ty)
                    {
                        return Err(DataError::WrongKind);
                    }
                    Ok(handle.bits())
                }
                1 => {
                    match runtime
                        .data
                        .error_metadata(&mut runtime.heap, handle()?, index()?, ty)?
                    {
                        ManagedSlot::Reference(handle) => Ok(handle.bits()),
                        _ => Err(DataError::WrongKind),
                    }
                }
                2 => {
                    let count = usize::try_from(count).map_err(|_| DataError::InvalidEncoding)?;
                    if count > isize::MAX as usize / 8 || (count != 0 && types.is_null()) {
                        return Err(DataError::InvalidEncoding);
                    }
                    let types = if count == 0 {
                        &[][..]
                    } else {
                        // SAFETY: verified lowering owns this aligned array for the call.
                        unsafe { std::slice::from_raw_parts(types, count) }
                    };
                    let expected = types
                        .iter()
                        .map(|ty| usize::try_from(*ty).map_err(|_| DataError::InvalidEncoding))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    let handle = handle()?;
                    let list = runtime.data.error_list_type(&runtime.heap, handle)?;
                    Ok(u64::from(runtime.data.match_error_list(
                        &runtime.heap,
                        handle,
                        list,
                        &expected,
                        argument != 0,
                    )?))
                }
                3 => {
                    let handle = handle()?;
                    let list = runtime.data.error_list_type(&runtime.heap, handle)?;
                    let ManagedSlot::Reference(handle) =
                        runtime
                            .data
                            .project(&mut runtime.heap, handle, list, None, index()?)?
                    else {
                        return Err(DataError::WrongKind);
                    };
                    let actual = runtime.data.error_record_type(&runtime.heap, handle)?;
                    if !matches!(runtime.data_schema.value.ty(ty)?, ValueType::Error)
                        && actual != Some(ty)
                    {
                        return Err(DataError::WrongKind);
                    }
                    Ok(handle.bits())
                }
                4 => {
                    let handle = handle()?;
                    let list = runtime.data.error_list_type(&runtime.heap, handle)?;
                    if list != ty {
                        return Err(DataError::WrongKind);
                    }
                    Ok(runtime
                        .data
                        .error_list_rest(&mut runtime.heap, handle, list, index()?)?
                        .bits())
                }
                5 => {
                    let failure = runtime
                        .pending
                        .observe_error()
                        .ok_or(DataError::NotRecoverable)?;
                    runtime.caught_errors.push(failure.diagnostics_only());
                    Ok(runtime
                        .data
                        .catch_error_list(&mut runtime.heap, ty, &[failure], argument != 0)?
                        .bits())
                }
                12 => {
                    let count = usize::try_from(count).map_err(|_| DataError::InvalidEncoding)?;
                    if count > isize::MAX as usize / 8 || (count != 0 && types.is_null()) {
                        return Err(DataError::InvalidEncoding);
                    }
                    let indices = if count == 0 {
                        &[][..]
                    } else {
                        // SAFETY: verified lowering provides the aligned selection array.
                        unsafe { std::slice::from_raw_parts(types, count) }
                    };
                    let indices = indices
                        .iter()
                        .map(|index| u32::try_from(*index).map_err(|_| DataError::InvalidEncoding))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    let tail = if argument == -1 {
                        None
                    } else {
                        Some(u32::try_from(argument).map_err(|_| DataError::InvalidEncoding)?)
                    };
                    let failure = runtime.data.rethrow_caught(
                        &mut runtime.heap,
                        handle()?,
                        &indices,
                        tail,
                    )?;
                    runtime.pending.propagate(failure);
                    Ok(0)
                }
                7 => Ok(u64::from(runtime.data.error_is(
                    &runtime.heap,
                    handle()?,
                    index()?,
                )?)),
                6 => {
                    throw_error(runtime, handle()?)?;
                    Ok(0)
                }
                _ => Err(DataError::InvalidEncoding),
            }
        })();
        let Some(result) = runtime.data_result(result) else {
            return 0;
        };
        let allocated = runtime.heap.object_count().saturating_sub(before);
        if allocated != 0 {
            runtime.data_allocated(allocated);
        }
        result as i64
    })
}

// Shared by explicit throw and checked data operations.
pub(super) fn throw_error(
    runtime: &mut BaselineRuntime,
    handle: ManagedHandle,
) -> std::result::Result<(), DataError> {
    let first = runtime.data.error_source(&runtime.heap, handle)?.is_none();
    let source =
        runtime
            .data
            .first_error_source(&mut runtime.heap, handle, runtime.current_source)?;
    let error = runtime.data.lease_error(&runtime.heap, handle)?;
    let trace = runtime
        .data
        .error_trace(&runtime.heap, handle)?
        .or_else(|| {
            first
                .then(|| runtime.pending.tracer().capture(source))
                .flatten()
        });
    let mut failure = Failure::with_managed(
        Diagnostic {
            cause: FailureCause::Error(ErrorKind::User),
            source,
        },
        error,
    );
    if let Some(trace) = trace {
        let trace = runtime
            .data
            .first_error_trace(&mut runtime.heap, handle, trace)?;
        failure.traces.insert(0, trace);
    }
    runtime.pending.propagate(failure);
    Ok(())
}
