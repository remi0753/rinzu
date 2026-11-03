//! Process-wide retention snapshots, separate from a run's temporary ledger.
//! Concurrent runs may contribute to these totals; they are not leak predicates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeProcessEvidence {
    pub callback_pool: CallbackPoolEvidence,
    pub userdata: rinzu_runtime::data::NativeUserDataProcessStats,
    pub callback_returns: crate::CallbackReturnEvidence,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackPoolEvidence {
    pub entries: usize,
    pub blocks: usize,
    pub virtual_bytes: usize,
    pub control_bytes: usize,
    pub closed_entries: usize,
    /// A subset of control_bytes, rather than an additional allocation.
    pub tombstone_bytes: usize,
    pub block_metadata_bytes: usize,
    pub live_connections: usize,
    pub in_flight: usize,
    pub live_diagnostics: usize,
    pub diagnostic_bytes: usize,
    pub retired_entries: usize,
    /// A subset of the RW reservation already included in control_bytes.
    pub retired_slot_bytes: usize,
    pub released_control_bytes: usize,
}

impl NativeProcessEvidence {
    pub fn capture() -> Self {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let pool = rinzu_runtime::callback_entry::stats();
            let returns = rinzu_runtime::callback_return::stats();
            Self {
                callback_pool: CallbackPoolEvidence {
                    entries: pool.entries,
                    blocks: pool.blocks,
                    virtual_bytes: pool.virtual_bytes,
                    control_bytes: pool.control_bytes,
                    closed_entries: pool.closed_entries,
                    tombstone_bytes: pool.tombstone_bytes,
                    block_metadata_bytes: pool.block_metadata_bytes,
                    live_connections: pool.live_connections,
                    in_flight: pool.in_flight,
                    live_diagnostics: pool.live_diagnostics,
                    diagnostic_bytes: pool.diagnostic_bytes,
                    retired_entries: pool.retired_entries,
                    retired_slot_bytes: pool.retired_slot_bytes,
                    released_control_bytes: pool.released_control_bytes,
                },
                userdata: rinzu_runtime::data::userdata_process_stats(),
                callback_returns: crate::CallbackReturnEvidence {
                    normal: returns.normal,
                    fallback: returns.fallback,
                    bytes: returns.bytes,
                    metadata_bytes: returns.metadata_bytes,
                },
            }
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        Self {
            userdata: rinzu_runtime::data::userdata_process_stats(),
            ..Default::default()
        }
    }
}
