//! Non-owning native object references. Reservation is an internal rooted object,
//! never a source value; receiving a pointer neither reads nor retains its target.
use super::*;

#[derive(Debug)]
pub struct OpaqueReservation {
    handle: Option<ManagedHandle>,
    ty: usize,
}
impl OpaqueReservation {
    /// Only for call-ledger rooting before native entry. This handle cannot be
    /// used as an opaque value until successful reception initializes it.
    pub fn root(&self) -> Option<ManagedHandle> {
        self.handle
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeOpaqueError {
    Null,
    Data(DataError),
}
impl From<DataError> for NativeOpaqueError {
    fn from(error: DataError) -> Self {
        Self::Data(error)
    }
}
impl From<HeapError> for NativeOpaqueError {
    fn from(error: HeapError) -> Self {
        Self::Data(DataError::Heap(error))
    }
}

impl ManagedData {
    /// Reserve all wrapper/identity/root storage before external entry. The
    /// closed executable type catalog supplies the nominal type index.
    pub fn reserve_native_opaque(
        &mut self,
        heap: &mut DomainHeap,
        ty: usize,
    ) -> Result<OpaqueReservation, DataError> {
        self.require_domain(heap)?;
        let reference = resource_checked(
            heap.allocate(
                OPAQUE_TYPE,
                &[HeapValue::Integer(ty as i64), HeapValue::Integer(0)],
                FinalizerPolicy::None,
            )
            .map_err(DataError::from),
        )?;
        let handle = resource_checked(self.root(heap, reference, DataKind::ReservedOpaque(ty)))?;
        Ok(OpaqueReservation {
            handle: Some(handle),
            ty,
        })
    }

    /// Finish after physical return with no allocation, pointer dereference,
    /// native cleanup or address deduplication. NULL discards the unused root
    /// and reports the ordinary recoverable NullNativeValue condition.
    pub fn receive_native_opaque(
        &mut self,
        heap: &mut DomainHeap,
        reservation: &mut OpaqueReservation,
        pointer: u64,
    ) -> Result<ManagedHandle, NativeOpaqueError> {
        let handle = reservation.handle.ok_or(DataError::UnknownHandle)?;
        let reference = self.reference(heap, handle, DataKind::ReservedOpaque(reservation.ty))?;
        if heap.header(reference)?.type_id != OPAQUE_TYPE
            || heap.integer_field(reference, 0)? as usize != reservation.ty
            || heap.integer_field(reference, 1)? != 0
        {
            return Err(DataError::InvalidEncoding.into());
        }
        if pointer == 0 {
            self.release(heap, handle)?;
            reservation.handle = None;
            return Err(NativeOpaqueError::Null);
        }
        heap.set_integer_field(reference, 1, pointer as i64)?;
        self.roots
            .get_mut(&handle)
            .ok_or(DataError::UnknownHandle)?
            .1 = DataKind::NativeOpaque(reservation.ty);
        reservation.handle = None;
        Ok(handle)
    }

    /// Cancel or fail before entry without constructing a value or performing
    /// any operation on the external object.
    pub fn discard_native_opaque(
        &mut self,
        heap: &mut DomainHeap,
        reservation: &mut OpaqueReservation,
    ) -> Result<(), DataError> {
        let handle = reservation.handle.ok_or(DataError::UnknownHandle)?;
        self.reference(heap, handle, DataKind::ReservedOpaque(reservation.ty))?;
        self.release(heap, handle)?;
        reservation.handle = None;
        Ok(())
    }

    /// Recover the one physical object pointer only after nominal/Domain checks.
    pub fn native_opaque_pointer(
        &self,
        heap: &DomainHeap,
        handle: ManagedHandle,
        ty: usize,
    ) -> Result<u64, DataError> {
        let reference = self.reference(heap, handle, DataKind::NativeOpaque(ty))?;
        if heap.header(reference)?.type_id != OPAQUE_TYPE
            || heap.integer_field(reference, 0)? as usize != ty
        {
            return Err(DataError::InvalidEncoding);
        }
        let pointer = heap.integer_field(reference, 1)? as u64;
        if pointer == 0 {
            return Err(DataError::InvalidEncoding);
        }
        Ok(pointer)
    }
}

fn resource_checked<T>(result: Result<T, DataError>) -> Result<T, DataError> {
    match result {
        Err(
            DataError::AddressSpaceExhausted | DataError::Heap(HeapError::AddressSpaceExhausted),
        ) => {
            // SAFETY: exhausted reservation/identity resources are process fatal,
            // with no further allocation, external entry or managed cleanup.
            crate::native_fatal::resource()
        }
        other => other,
    }
}
