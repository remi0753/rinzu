//! Resolve generation-local runtime numbers in the current JIT module.
use super::*;
use cranelift_codegen::ir::{GlobalValue, MemFlagsData};
use cranelift_module::DataId;
use std::cell::{Cell, RefCell};

/// Each kind has its own numbering. Equality within a kind identifies the
/// same resolved value when assigning a function's slots in first-use order.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum LinkValue {
    Function(i64),
    Reference(i64),
    Type(i64),
    NativePlan(i64),
    Cleanup(i64),
    SourceFile(i64),
    SourceSpan(i64),
    Scope(i64),
}

impl LinkValue {
    pub(super) fn value(self) -> i64 {
        match self {
            Self::Function(value)
            | Self::Reference(value)
            | Self::Type(value)
            | Self::NativePlan(value)
            | Self::Cleanup(value)
            | Self::SourceFile(value)
            | Self::SourceSpan(value)
            | Self::Scope(value) => value,
        }
    }
}

#[derive(Default)]
struct Slots {
    offsets: HashMap<LinkValue, usize>,
    bytes: Vec<u8>,
}

/// One read-only table per managed function. The code refers to its table
/// and slot offsets. The current generation supplies the numbers separately.
pub(super) struct LinkValues {
    data: DataId,
    symbol: GlobalValue,
    address: Cell<Option<Value>>,
    slots: RefCell<Slots>,
}

impl LinkValues {
    pub(super) fn declare<M: Module>(
        module: &mut M,
        function: &mut cranelift_codegen::ir::Function,
    ) -> Result<Self> {
        let data = module.declare_anonymous_data(false, false)?;
        Ok(Self {
            data,
            symbol: module.declare_data_in_func(data, function),
            address: Cell::new(None),
            slots: RefCell::default(),
        })
    }

    pub(super) fn bind(&self, builder: &mut FunctionBuilder<'_>) {
        let address = builder.ins().symbol_value(types::I64, self.symbol);
        self.address.set(Some(address));
    }

    pub(super) fn load(&self, builder: &mut FunctionBuilder<'_>, value: LinkValue) -> Value {
        let Slots { offsets, bytes } = &mut *self.slots.borrow_mut();
        let offset = *offsets.entry(value).or_insert_with(|| {
            let offset = bytes.len();
            bytes.extend_from_slice(&value.value().to_le_bytes());
            offset
        });
        let address = self.address.get().expect("bound at the function entry");
        let (address, offset) = if let Ok(offset) = i32::try_from(offset) {
            (address, offset)
        } else {
            (builder.ins().iadd_imm_s(address, offset as i64), 0)
        };
        builder.ins().load(
            types::I64,
            MemFlagsData::trusted().with_readonly(),
            address,
            offset,
        )
    }

    pub(super) fn define<M: Module>(self, module: &mut M) -> Result<()> {
        let mut description = DataDescription::new();
        description.align = Some(8);
        description.define(self.slots.into_inner().bytes.into_boxed_slice());
        module.define_data(self.data, &description)?;
        Ok(())
    }
}
