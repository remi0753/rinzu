use super::*;

impl ManagedData {
    pub(super) fn json_parse(
        &mut self,
        heap: &mut DomainHeap,
        schema: &core::Schema,
        call: &JsonCall<'_>,
        poll: &mut impl FnMut() -> Option<FailureCause>,
    ) -> Result<ManagedSlot> {
        let limits = self.json_limits(heap, call.slots.get(1).copied(), core::INPUT)?;
        let input = reference(call.slots[0])?;
        let (source, kind) = self.any_reference(heap, input)?;
        let length = bytes(heap, source)?.len();
        let typed = matches!(
            call.operation,
            DataOperation::JsonDecode | DataOperation::JsonDecodeWithLimits
        );
        let mut parser = core::Parser::new(length, limits, typed.then_some((schema, call.result)));
        let mut batch = typed.then(core::Batch::default);
        let mut builder = Builder::default();
        let outcome = (|| {
            loop {
                let (source, _) = self.any_reference(heap, input)?;
                let progress = parser.step(bytes(heap, source)?, batch.as_mut())?;
                for &event in progress.events {
                    builder.consume(self, heap, schema, source, event, progress.decoded)?;
                }
                if progress.done {
                    break;
                }
                if let Some(cause) = poll() {
                    return Err(DataError::Interrupted(cause).into());
                }
                // Input and every provisional typed child have registered
                // roots. A continuation only retains offsets and handles.
                if heap.gc_stress() && heap.collection_due() {
                    heap.collect(crate::heap::CollectionKind::Minor)?;
                }
            }
            if typed {
                return builder
                    .root
                    .take()
                    .ok_or_else(|| DataError::InvalidEncoding.into());
            }
            let tape = parser.index();
            crate::resource_accounting::record_copy(tape.bytes().len() as u64);
            let index = self.create_bytes(heap, tape.bytes().len() as u64, tape.bytes())?;
            let tag = if kind == DataKind::String {
                PARSED_STRING
            } else {
                PARSED_BYTES
            };
            let slots =
                [call.slots[0], ManagedSlot::Reference(index), int(0)].map(DecodedSlot::Value);
            let result = self.json_value(heap, call.result, tag, &slots);
            self.release(heap, index)?;
            result
        })();
        self.json_release(heap, builder.slots.into_iter().chain(builder.root))?;
        outcome
    }
}

#[derive(Default)]
struct Builder {
    frames: Vec<Frame>,
    slots: Vec<ManagedSlot>,
    root: Option<ManagedSlot>,
}
struct Frame {
    start: usize,
    base: usize,
    field: Option<usize>,
}

fn base_type(schema: &core::Schema, mut ty: usize) -> usize {
    while schema.types[ty].kind == core::OPTION_TY {
        ty = schema.types[ty].child as usize;
    }
    ty
}

fn wrap(
    data: &mut ManagedData,
    heap: &mut DomainHeap,
    schema: &core::Schema,
    ty: usize,
    base: usize,
    value: ManagedSlot,
) -> Result<ManagedSlot> {
    if ty == base {
        return Ok(value);
    }
    let inner = wrap(
        data,
        heap,
        schema,
        schema.types[ty].child as usize,
        base,
        value,
    )?;
    Ok(data.json_option_owned(heap, ty, Some(inner))?)
}

fn event_string(
    data: &mut ManagedData,
    heap: &mut DomainHeap,
    source: HeapRef,
    event: core::Event,
    decoded: &[u8],
) -> Result<ManagedSlot> {
    if event.is_decoded() {
        Ok(data.json_string(heap, &decoded[event.text_range()])?)
    } else {
        data.json_copy(heap, source, event.text_range(), true)
    }
}

impl Builder {
    fn value(
        &mut self,
        data: &mut ManagedData,
        heap: &mut DomainHeap,
        value: ManagedSlot,
    ) -> Result<()> {
        if let Some(frame) = self.frames.last_mut() {
            if let Some(field) = frame.field.take() {
                let previous = std::mem::replace(&mut self.slots[frame.start + field], value);
                data.json_release(heap, [previous])?;
            } else {
                self.slots.push(value);
            }
        } else if self.root.replace(value).is_some() {
            return Err(DataError::InvalidEncoding.into());
        }
        Ok(())
    }

    fn consume(
        &mut self,
        data: &mut ManagedData,
        heap: &mut DomainHeap,
        schema: &core::Schema,
        source: HeapRef,
        event: core::Event,
        decoded: &[u8],
    ) -> Result<()> {
        let ty = event.ty as usize;
        match event.event {
            core::KEY => {
                let frame = self.frames.last_mut().ok_or(DataError::InvalidEncoding)?;
                if event.field == core::NONE {
                    self.slots
                        .push(event_string(data, heap, source, event, decoded)?);
                } else {
                    frame.field = Some(event.field as usize);
                }
            }
            core::SCALAR if event.kind == core::NULL => {
                let value = data.json_option(heap, ty, None)?;
                self.value(data, heap, value)?;
            }
            core::SCALAR => {
                let base = base_type(schema, ty);
                let value = match schema.types[base].kind {
                    core::BOOL_TY => scalar(ScalarKind::Bool, event.bits),
                    core::INT_TY => scalar(ScalarKind::I64, event.bits),
                    core::FLOAT_TY => scalar(ScalarKind::F64, event.bits),
                    core::STRING_TY => event_string(data, heap, source, event, decoded)?,
                    _ => return Err(DataError::WrongKind.into()),
                };
                let value = wrap(data, heap, schema, ty, base, value)?;
                self.value(data, heap, value)?;
            }
            core::OPEN => {
                let base = base_type(schema, ty);
                let start = self.slots.len();
                self.frames.push(Frame {
                    start,
                    base,
                    field: None,
                });
                if schema.types[base].kind == core::RECORD_TY {
                    self.slots
                        .resize(start + schema.types[base].count as usize, ManagedSlot::Unit);
                }
            }
            core::CLOSE => {
                let frame = self.frames.pop().ok_or(DataError::InvalidEncoding)?;
                let layout = schema.types[frame.base];
                let slots = &mut self.slots[frame.start..];
                if layout.kind == core::RECORD_TY {
                    for (i, slot) in slots.iter_mut().enumerate() {
                        if *slot == ManagedSlot::Unit {
                            let field = schema.fields[layout.first as usize + i];
                            *slot = data.json_option(heap, field.ty as usize, None)?;
                        }
                    }
                }
                let value = data.create_composite(heap, frame.base, None, slots);
                data.json_release(heap, self.slots.drain(frame.start..))?;
                let value = wrap(
                    data,
                    heap,
                    schema,
                    ty,
                    frame.base,
                    ManagedSlot::Reference(value?),
                )?;
                self.value(data, heap, value)?;
            }
            _ => return Err(DataError::InvalidEncoding.into()),
        }
        Ok(())
    }
}
