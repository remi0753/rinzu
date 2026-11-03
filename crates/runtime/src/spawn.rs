//! Atomic admission for isolated CPU spawn operations.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::transport::{
    TransferDescriptor, TransferValue, TransportBudget, TransportBuffer, TransportCreditEvidence,
    TransportDirection, TransportError, TransportReservation, encode,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OriginDomainId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnLimits {
    pub worker_domains: usize,
    pub in_flight_per_origin: usize,
    pub process_transport_bytes: usize,
    pub operation_transport_bytes: usize,
}

impl Default for SpawnLimits {
    fn default() -> Self {
        Self {
            worker_domains: 8,
            in_flight_per_origin: 64,
            process_transport_bytes: 256 * 1024 * 1024,
            operation_transport_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnAdmissionSnapshot {
    pub worker_domains: usize,
    pub peak_worker_domains: usize,
    pub in_flight_for_origin: usize,
    pub peak_in_flight_per_origin: usize,
    pub transport_bytes: usize,
    pub peak_transport_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpawnAdmissionError {
    WorkerDomainLimit {
        used: usize,
        limit: usize,
    },
    InFlightLimit {
        origin: OriginDomainId,
        used: usize,
        limit: usize,
    },
    Transport(TransportError),
    InputAlreadySettled,
    InputNotSettled,
}

impl fmt::Display for SpawnAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerDomainLimit { used, limit } => {
                write!(
                    formatter,
                    "Worker Domain limit reached: used {used}, limit {limit}"
                )
            }
            Self::InFlightLimit {
                origin,
                used,
                limit,
            } => write!(
                formatter,
                "in-flight spawn limit reached for {origin:?}: used {used}, limit {limit}"
            ),
            Self::Transport(error) => error.fmt(formatter),
            Self::InputAlreadySettled => formatter.write_str("spawn input is already settled"),
            Self::InputNotSettled => {
                formatter.write_str("spawn result cannot reserve credit before input settles")
            }
        }
    }
}

impl std::error::Error for SpawnAdmissionError {}

impl From<TransportError> for SpawnAdmissionError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

#[derive(Debug)]
struct SpawnState {
    worker_domains: usize,
    peak_worker_domains: usize,
    in_flight: HashMap<OriginDomainId, usize>,
    peak_in_flight_per_origin: usize,
}

#[derive(Debug)]
struct SpawnAdmissionInner {
    limits: SpawnLimits,
    transport: TransportBudget,
    state: Mutex<SpawnState>,
}

#[derive(Clone, Debug)]
pub struct SpawnAdmissionController {
    inner: Arc<SpawnAdmissionInner>,
}

impl SpawnAdmissionController {
    #[must_use]
    pub fn new() -> Self {
        Self::from_target_profile(SpawnLimits::default())
    }

    #[must_use]
    pub fn from_target_profile(limits: SpawnLimits) -> Self {
        assert!(
            limits.worker_domains > 0,
            "Worker Domain limit must be non-zero"
        );
        assert!(
            limits.in_flight_per_origin > 0,
            "in-flight spawn limit must be non-zero"
        );
        Self {
            inner: Arc::new(SpawnAdmissionInner {
                transport: TransportBudget::new(
                    limits.process_transport_bytes,
                    limits.operation_transport_bytes,
                ),
                limits,
                state: Mutex::new(SpawnState {
                    worker_domains: 0,
                    peak_worker_domains: 0,
                    in_flight: HashMap::new(),
                    peak_in_flight_per_origin: 0,
                }),
            }),
        }
    }

    pub fn admit(
        &self,
        origin: OriginDomainId,
        input_bytes: usize,
    ) -> Result<AdmittedSpawn, SpawnAdmissionError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.worker_domains >= self.inner.limits.worker_domains {
            return Err(SpawnAdmissionError::WorkerDomainLimit {
                used: state.worker_domains,
                limit: self.inner.limits.worker_domains,
            });
        }
        let origin_used = state.in_flight.get(&origin).copied().unwrap_or(0);
        if origin_used >= self.inner.limits.in_flight_per_origin {
            return Err(SpawnAdmissionError::InFlightLimit {
                origin,
                used: origin_used,
                limit: self.inner.limits.in_flight_per_origin,
            });
        }
        // Slot checks and byte reservation are serialized by the admission
        // lock. No observer can see just one half of this transaction.
        let input = self
            .inner
            .transport
            .reserve(TransportDirection::Input, input_bytes)?;
        state.worker_domains += 1;
        state.peak_worker_domains = state.peak_worker_domains.max(state.worker_domains);
        let origin_in_flight = {
            let in_flight = state.in_flight.entry(origin).or_default();
            *in_flight += 1;
            *in_flight
        };
        state.peak_in_flight_per_origin = state.peak_in_flight_per_origin.max(origin_in_flight);
        drop(state);
        Ok(AdmittedSpawn {
            controller: Arc::clone(&self.inner),
            slots: Some(SpawnSlotReservation {
                controller: Arc::clone(&self.inner),
                origin,
                released: false,
                _accounting: crate::resource_accounting::Allocation::new(
                    crate::resource_accounting::Kind::TransportInFlight,
                    1,
                ),
            }),
            input: Some(input),
            input_settled: false,
            evidence_operation: None,
        })
    }

    #[must_use]
    pub fn snapshot(&self, origin: OriginDomainId) -> SpawnAdmissionSnapshot {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let transport = self.inner.transport.snapshot();
        SpawnAdmissionSnapshot {
            worker_domains: state.worker_domains,
            peak_worker_domains: state.peak_worker_domains,
            in_flight_for_origin: state.in_flight.get(&origin).copied().unwrap_or(0),
            peak_in_flight_per_origin: state.peak_in_flight_per_origin,
            transport_bytes: transport.used_bytes,
            peak_transport_bytes: transport.peak_bytes,
        }
    }

    pub fn enable_transport_evidence(&self) {
        self.inner.transport.enable_evidence();
    }

    #[must_use]
    pub fn transport_evidence(&self) -> Vec<TransportCreditEvidence> {
        self.inner.transport.evidence()
    }
}

impl Default for SpawnAdmissionController {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct SpawnSlotReservation {
    _accounting: crate::resource_accounting::Allocation,
    controller: Arc<SpawnAdmissionInner>,
    origin: OriginDomainId,
    released: bool,
}

impl Drop for SpawnSlotReservation {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut state = self
            .controller
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.worker_domains = state
            .worker_domains
            .checked_sub(1)
            .expect("Worker Domain slot releases exactly once");
        let remove_origin = {
            let in_flight = state
                .in_flight
                .get_mut(&self.origin)
                .expect("origin in-flight slot exists while spawn is admitted");
            *in_flight = in_flight
                .checked_sub(1)
                .expect("in-flight spawn slot releases exactly once");
            *in_flight == 0
        };
        if remove_origin {
            state.in_flight.remove(&self.origin);
        }
        self.released = true;
    }
}

#[derive(Debug)]
pub struct AdmittedSpawn {
    controller: Arc<SpawnAdmissionInner>,
    slots: Option<SpawnSlotReservation>,
    input: Option<TransportReservation>,
    input_settled: bool,
    evidence_operation: Option<u64>,
}

impl AdmittedSpawn {
    pub fn encode_typed_input(
        &mut self,
        descriptor: &crate::transport::typed::Descriptor,
        values: &[crate::value::Value],
    ) -> Result<TransportBuffer, SpawnAdmissionError> {
        let reservation = self
            .input
            .take()
            .ok_or(SpawnAdmissionError::InputAlreadySettled)?;
        Ok(descriptor.encode(values, reservation)?)
    }

    /// The receiver prepares every heap value/root while input credit is held.
    /// Result encoding is enabled only after that preparation and buffer release.
    pub fn prepare_typed_input<T>(
        &mut self,
        descriptor: &crate::transport::typed::Descriptor,
        buffer: TransportBuffer,
        prepare: impl FnOnce(Vec<crate::value::Value>) -> Result<T, TransportError>,
    ) -> Result<T, SpawnAdmissionError> {
        if self.input.is_some()
            || self.input_settled
            || buffer.direction() != TransportDirection::Input
        {
            return Err(SpawnAdmissionError::InputAlreadySettled);
        }
        let values = buffer.decode_typed(descriptor)?;
        let result = prepare(values)?;
        drop(buffer);
        self.input_settled = true;
        Ok(result)
    }

    pub fn encode_typed_result(
        &self,
        descriptor: &crate::transport::typed::Descriptor,
        values: &[crate::value::Value],
    ) -> Result<TransportBuffer, SpawnAdmissionError> {
        if !self.input_settled {
            return Err(SpawnAdmissionError::InputNotSettled);
        }
        let exact = descriptor.preflight(values)?;
        let reservation = self.controller.transport.reserve_for_operation(
            TransportDirection::Result,
            exact,
            self.evidence_operation,
        )?;
        Ok(descriptor.encode(values, reservation)?)
    }

    pub fn set_evidence_operation(&mut self, operation: u64) {
        assert!(self.evidence_operation.replace(operation).is_none());
    }

    pub fn encode_input(
        &mut self,
        descriptor: &TransferDescriptor,
        value: &TransferValue,
    ) -> Result<TransportBuffer, SpawnAdmissionError> {
        let reservation = self
            .input
            .take()
            .ok_or(SpawnAdmissionError::InputAlreadySettled)?;
        Ok(encode(descriptor, value, reservation)?)
    }

    pub fn materialize_input_buffer(
        &mut self,
        descriptor: &TransferDescriptor,
        buffer: TransportBuffer,
    ) -> Result<TransferValue, SpawnAdmissionError> {
        if self.input.is_some()
            || self.input_settled
            || buffer.direction() != TransportDirection::Input
        {
            return Err(SpawnAdmissionError::InputAlreadySettled);
        }
        let result = buffer.materialize(descriptor)?;
        self.input_settled = true;
        Ok(result)
    }

    pub fn materialize_input(
        &mut self,
        descriptor: &TransferDescriptor,
        value: &TransferValue,
    ) -> Result<TransferValue, SpawnAdmissionError> {
        let buffer = self.encode_input(descriptor, value)?;
        self.materialize_input_buffer(descriptor, buffer)
    }

    pub fn materialize_result(
        &self,
        descriptor: &TransferDescriptor,
        value: &TransferValue,
    ) -> Result<TransferValue, SpawnAdmissionError> {
        Ok(self
            .encode_result(descriptor, value)?
            .materialize(descriptor)?)
    }

    pub fn encode_result(
        &self,
        descriptor: &TransferDescriptor,
        value: &TransferValue,
    ) -> Result<TransportBuffer, SpawnAdmissionError> {
        if !self.input_settled {
            return Err(SpawnAdmissionError::InputNotSettled);
        }
        let exact = crate::transport::preflight(descriptor, value)?;
        let reservation = self.controller.transport.reserve_for_operation(
            TransportDirection::Result,
            exact,
            self.evidence_operation,
        )?;
        Ok(encode(descriptor, value, reservation)?)
    }

    #[must_use]
    pub fn input_settled(&self) -> bool {
        self.input_settled
    }
}

impl Drop for AdmittedSpawn {
    fn drop(&mut self) {
        self.input.take();
        self.slots.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{TransferDescriptor, TransferKind};

    #[test]
    fn input_and_result_buffers_keep_credit_on_the_receiving_side_until_decode() {
        use crate::scalar::{ScalarKind, ScalarValue};
        let descriptor = TransferDescriptor {
            kind: TransferKind::Scalar(ScalarKind::U8),
            source: None,
        };
        let value = TransferValue::Scalar(ScalarValue::from_integer(ScalarKind::U8, 255).unwrap());
        let controller = SpawnAdmissionController::new();
        controller.enable_transport_evidence();
        let origin = OriginDomainId(7);
        let mut admitted = controller.admit(origin, 1).unwrap();
        admitted.set_evidence_operation(19);
        let mut buffer = admitted.encode_input(&descriptor, &value).unwrap();
        buffer.set_evidence_operation(19);
        assert_eq!(controller.snapshot(origin).transport_bytes, 1);
        assert!(matches!(
            admitted.encode_result(&descriptor, &value),
            Err(SpawnAdmissionError::InputNotSettled)
        ));
        let worker_descriptor = descriptor.clone();
        let (buffer, admitted) = std::thread::spawn(move || {
            let value = admitted
                .materialize_input_buffer(&worker_descriptor, buffer)
                .unwrap();
            let result = admitted.encode_result(&worker_descriptor, &value).unwrap();
            (result, admitted)
        })
        .join()
        .unwrap();
        let snapshot = controller.snapshot(origin);
        assert_eq!(snapshot.transport_bytes, 1);
        assert_eq!(snapshot.worker_domains, 1);
        assert_eq!(snapshot.peak_transport_bytes, 1);
        assert_eq!(buffer.materialize(&descriptor).unwrap(), value);
        drop(admitted);
        let snapshot = controller.snapshot(origin);
        assert_eq!(snapshot.transport_bytes, 0);
        assert_eq!(snapshot.worker_domains, 0);
        assert_eq!(snapshot.in_flight_for_origin, 0);
        let evidence = controller.transport_evidence();
        assert_eq!(evidence.len(), 2);
        assert!(
            evidence
                .iter()
                .all(|credit| credit.operation == Some(19) && credit.bytes == 1)
        );
        assert_eq!(
            (evidence[0].reserved_at, evidence[0].released_at),
            (1, Some(2))
        );
        assert_eq!(
            (evidence[1].reserved_at, evidence[1].released_at),
            (3, Some(4))
        );
    }

    fn string_descriptor() -> TransferDescriptor {
        TransferDescriptor {
            kind: TransferKind::String,
            source: None,
        }
    }

    #[test]
    fn admission_is_all_or_nothing_and_releases_each_resource_once() {
        let limits = SpawnLimits {
            worker_domains: 1,
            in_flight_per_origin: 1,
            process_transport_bytes: 32,
            operation_transport_bytes: 24,
        };
        let controller = SpawnAdmissionController::from_target_profile(limits);
        let origin = OriginDomainId(7);
        let descriptor = string_descriptor();
        let input = TransferValue::String("input".into());
        let input_size = crate::transport::preflight(&descriptor, &input).unwrap();
        let mut admitted = controller.admit(origin, input_size).unwrap();
        let before_failure = controller.snapshot(origin);

        assert!(matches!(
            controller.admit(origin, input_size),
            Err(SpawnAdmissionError::WorkerDomainLimit { .. })
        ));
        assert_eq!(controller.snapshot(origin), before_failure);
        assert!(matches!(
            SpawnAdmissionController::from_target_profile(limits).admit(origin, 25),
            Err(SpawnAdmissionError::Transport(
                TransportError::OperationLimit { .. }
            ))
        ));

        assert_eq!(
            admitted.materialize_input(&descriptor, &input).unwrap(),
            input
        );
        assert!(admitted.input_settled());
        assert_eq!(controller.snapshot(origin).transport_bytes, 0);
        let result = TransferValue::String("result".into());
        assert_eq!(
            admitted.materialize_result(&descriptor, &result).unwrap(),
            result
        );
        assert_eq!(controller.snapshot(origin).transport_bytes, 0);

        drop(admitted);
        let final_snapshot = controller.snapshot(origin);
        assert_eq!(final_snapshot.worker_domains, 0);
        assert_eq!(final_snapshot.in_flight_for_origin, 0);
        assert_eq!(final_snapshot.transport_bytes, 0);
        assert_eq!(final_snapshot.peak_worker_domains, 1);
        assert_eq!(final_snapshot.peak_in_flight_per_origin, 1);
    }
}
