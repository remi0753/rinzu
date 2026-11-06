use super::*;

enum Write {
    Value {
        value: HeapRef,
        depth: u32,
    },
    Children {
        collection: HeapRef,
        count: usize,
        next: usize,
        depth: u32,
        object: bool,
    },
}

fn exceeded(kind: u32, limit: i64) -> Fault {
    core::Error {
        code: core::LIMIT,
        reason: kind,
        limit,
        offset: core::NONE,
        ..Default::default()
    }
    .into()
}

struct Output<P> {
    poll: P,
    limits: core::Limits,
    nodes: u64,
    buffer: core::Buffer,
}

impl<P: FnMut() -> Option<FailureCause>> Output<P> {
    fn check(&mut self, depth: u32, nodes: u32) -> Result<()> {
        if i64::from(depth) > self.limits.depth {
            return Err(exceeded(core::DEPTH, self.limits.depth));
        }
        self.nodes += u64::from(nodes);
        if self.nodes > self.limits.nodes as u64 {
            return Err(exceeded(core::NODES, self.limits.nodes));
        }
        Ok(())
    }
    fn append(&mut self, bytes: &[u8], escape: bool) -> Result<()> {
        if self
            .buffer
            .append(self.limits.bytes as usize, bytes, escape)
        {
            Ok(())
        } else {
            Err(exceeded(core::OUTPUT, self.limits.bytes))
        }
    }

    fn write(&mut self, bytes: &[u8], quote: bool) -> Result<()> {
        if quote {
            self.append(b"\"", false)?;
        }
        for (i, chunk) in bytes.chunks(64 * 1024).enumerate() {
            if i != 0
                && let Some(cause) = (self.poll)()
            {
                return Err(DataError::Interrupted(cause).into());
            }
            self.append(chunk, quote)?;
        }
        if quote {
            self.append(b"\"", false)?;
        }
        Ok(())
    }
}

impl ManagedData {
    pub(super) fn json_encode(
        &mut self,
        heap: &mut DomainHeap,
        call: &JsonCall<'_>,
        poll: &mut impl FnMut() -> Option<FailureCause>,
    ) -> Result<ManagedSlot> {
        let limits = self.json_limits(heap, call.slots.get(1).copied(), core::OUTPUT)?;
        let text = matches!(self.value_schema()?.ty(call.result)?, ValueType::String);
        let (value, _) = self.any_reference(heap, reference(call.slots[0])?)?;
        let mut out = Output {
            poll,
            limits,
            nodes: 0,
            buffer: core::Buffer::default(),
        };
        if let Repr::Parsed {
            source,
            index,
            node,
            tag,
        } = self.json_view(heap, value)?
        {
            let selected = document(heap, source, index)?.node(node);
            out.check(selected.depth, selected.nodes)?;
            let range = if node == 0 {
                0..bytes(heap, source)?.len()
            } else {
                selected.start as usize..selected.end as usize
            };
            if range.len() > limits.bytes as usize {
                return Err(exceeded(core::OUTPUT, limits.bytes));
            }
            if node == 0 && text == (tag == PARSED_STRING) {
                let kind = if text {
                    DataKind::String
                } else {
                    DataKind::Bytes(range.len() as u64)
                };
                return Ok(ManagedSlot::Reference(self.root(heap, source, kind)?));
            }
            crate::resource_accounting::record_copy(range.len() as u64);
            return self.json_copy(heap, source, range, text);
        }

        // The stack stores offsets into the same Owner heap. There are no
        // managed allocations or collections until output is complete.
        let mut pending = vec![Write::Value { value, depth: 0 }];
        let mut work = 0;
        let mut last_poll = 0;
        while let Some(action) = pending.pop() {
            work += 1;
            if work == 16_384 || out.buffer.bytes().len() - last_poll >= 64 * 1024 {
                if let Some(cause) = (out.poll)() {
                    return Err(DataError::Interrupted(cause).into());
                }
                work = 0;
                last_poll = out.buffer.bytes().len();
            }
            match action {
                Write::Value { value, depth } => match self.json_view(heap, value)? {
                    Repr::Parsed {
                        source,
                        index,
                        node,
                        ..
                    } => {
                        let info = document(heap, source, index)?.node(node);
                        out.check(depth + info.depth, info.nodes)?;
                        out.write(
                            &bytes(heap, source)?[info.start as usize..info.end as usize],
                            false,
                        )?;
                    }
                    Repr::Built { child, tag } => {
                        let is_container = matches!(tag, ARRAY | OBJECT);
                        out.check(depth + u32::from(is_container), 1)?;
                        match tag {
                            NULL => out.write(b"null", false)?,
                            BOOLEAN => {
                                let value = integer(child.as_ref())? != 0;
                                out.write(if value { b"true" } else { b"false" }, false)?;
                            }
                            TEXT | TOKEN => out.write(
                                bytes(heap, heap_reference(child.ok_or(DataError::WrongKind)?)?)?,
                                tag == TEXT,
                            )?,
                            INTEGER | FLOAT => {
                                let mut digits = [0; 32];
                                let token = core::format(
                                    integer(child.as_ref())? as u64,
                                    tag == FLOAT,
                                    &mut digits,
                                );
                                out.write(token, false)?;
                            }
                            ARRAY | OBJECT => {
                                let object = tag == OBJECT;
                                let collection =
                                    heap_reference(child.ok_or(DataError::WrongKind)?)?;
                                let fields = heap.header(collection)?.field_count as usize - 2;
                                let count = if object { fields / 2 } else { fields };
                                out.write(if object { b"{" } else { b"[" }, false)?;
                                pending.push(Write::Children {
                                    collection,
                                    count,
                                    next: 0,
                                    depth: depth + 1,
                                    object,
                                });
                            }
                            _ => return Err(DataError::WrongKind.into()),
                        }
                    }
                },
                Write::Children {
                    collection,
                    count,
                    next,
                    depth,
                    object,
                } => {
                    if next == count {
                        out.write(if object { b"}" } else { b"]" }, false)?;
                        continue;
                    }
                    if next != 0 {
                        out.write(b",", false)?;
                    }
                    let offset = if object { 2 + next * 2 } else { 2 + next };
                    if object {
                        out.check(depth, 1)?;
                        out.write(
                            bytes(heap, reference_field(heap, collection, offset)?)?,
                            true,
                        )?;
                        out.write(b":", false)?;
                    }
                    pending.push(Write::Children {
                        collection,
                        count,
                        next: next + 1,
                        depth,
                        object,
                    });
                    let value = reference_field(heap, collection, offset + usize::from(object))?;
                    pending.push(Write::Value { value, depth });
                }
            }
        }
        let bytes = out.buffer.bytes();
        crate::resource_accounting::record_copy(bytes.len() as u64);
        if text {
            Ok(self.json_string(heap, bytes)?)
        } else {
            Ok(ManagedSlot::Reference(self.create_bytes(
                heap,
                bytes.len() as u64,
                bytes,
            )?))
        }
    }
}
