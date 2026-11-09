//! OS-backed resource measurements for the architecture-freeze gate.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod platform {
    use std::fs::File;
    use std::mem::{size_of, zeroed};
    use std::path::Path;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, ensure};
    use rinzu_codegen::with_jit_spike_inspection;
    use rinzu_runtime::blocking::BlockingPool;
    use rinzu_runtime::resources::{ResourceKind, ResourceLedger, ResourceLimits};
    use rinzu_runtime::spawn::{OriginDomainId, SpawnAdmissionController, SpawnLimits};
    use rinzu_runtime::worker::CpuWorkerPool;
    use serde_json::{Value, json};

    const TRANSPORT_LIMIT: usize = 16 * 1024 * 1024;
    const OPERATION_LIMIT: usize = 4 * 1024 * 1024;
    const WORKER_LIMIT: usize = 4;
    const CODE_LIMIT: usize = 4 * 1024 * 1024;
    const RSS_DELTA_LIMIT: u64 = (TRANSPORT_LIMIT + 32 * 1024 * 1024) as u64;
    const VIRTUAL_DELTA_LIMIT: u64 = 256 * 1024 * 1024;
    const THREAD_DELTA_LIMIT: u64 = 12;
    const FD_DELTA_LIMIT: u64 = 8;

    const VM_REGION_BASIC_INFO_64: i32 = 9;
    const VM_PROT_EXECUTE: i32 = 0x04;
    const KERN_SUCCESS: i32 = 0;

    #[repr(C)]
    struct VmRegionBasicInfo64 {
        protection: i32,
        max_protection: i32,
        inheritance: u32,
        shared: i32,
        reserved: i32,
        offset: u64,
        behavior: i32,
        user_wired_count: u16,
    }

    unsafe extern "C" {
        static mach_task_self_: libc::mach_port_t;

        fn mach_vm_region(
            task: libc::mach_port_t,
            address: *mut libc::mach_vm_address_t,
            size: *mut libc::mach_vm_size_t,
            flavor: i32,
            info: *mut i32,
            count: *mut libc::mach_msg_type_number_t,
            object_name: *mut libc::mach_port_t,
        ) -> libc::kern_return_t;

        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }

    #[derive(Clone, Copy, Debug)]
    struct Metrics {
        rss_bytes: u64,
        virtual_bytes: u64,
        threads: u64,
        file_descriptors: u64,
        executable_bytes: u64,
    }

    impl Metrics {
        fn json(self) -> Value {
            json!({
                "rssBytes": self.rss_bytes,
                "virtualBytes": self.virtual_bytes,
                "threads": self.threads,
                "fileDescriptors": self.file_descriptors,
                "codeMemoryBytes": self.executable_bytes,
            })
        }

        fn delta_from(self, baseline: Self) -> Self {
            Self {
                rss_bytes: self.rss_bytes.saturating_sub(baseline.rss_bytes),
                virtual_bytes: self.virtual_bytes.saturating_sub(baseline.virtual_bytes),
                threads: self.threads.saturating_sub(baseline.threads),
                file_descriptors: self
                    .file_descriptors
                    .saturating_sub(baseline.file_descriptors),
                executable_bytes: self
                    .executable_bytes
                    .saturating_sub(baseline.executable_bytes),
            }
        }
    }

    pub fn run(output: Option<&Path>) -> Result<Value> {
        let idle = snapshot().context("measure idle resources")?;

        let cpu_pool = CpuWorkerPool::new();
        let blocking_pool = BlockingPool::new();
        let descriptor = File::open("Cargo.toml").context("open resource-probe descriptor")?;

        let limits = SpawnLimits {
            worker_domains: WORKER_LIMIT,
            in_flight_per_origin: WORKER_LIMIT,
            process_transport_bytes: TRANSPORT_LIMIT,
            operation_transport_bytes: OPERATION_LIMIT,
        };
        let controller = SpawnAdmissionController::from_target_profile(limits);
        let origin = OriginDomainId(7);
        let mut admissions = Vec::with_capacity(WORKER_LIMIT);
        let mut buffers = Vec::with_capacity(WORKER_LIMIT);

        admissions.push(controller.admit(origin, OPERATION_LIMIT)?);
        buffers.push(touched_bytes(OPERATION_LIMIT));

        let ledger_limits = ResourceLimits {
            live_tasks: 32,
            pending_operations: 32,
            registrations: 8,
            queue_slots: 64,
            control_slots: 4,
        };
        let mut ledger = ResourceLedger::new(ledger_limits);
        let reservations = [
            (ResourceKind::LiveTask, ledger_limits.live_tasks),
            (
                ResourceKind::PendingOperation,
                ledger_limits.pending_operations,
            ),
            (ResourceKind::Registration, ledger_limits.registrations),
            (ResourceKind::QueueSlot, ledger_limits.queue_slots),
            (ResourceKind::ControlSlot, ledger_limits.control_slots),
        ];
        let (jit, (load, overload, spawn_rejected, ledger_rejected)) =
            with_jit_spike_inspection(|_| {
                let load = snapshot().context("measure loaded resources")?;

                for _ in 1..WORKER_LIMIT {
                    admissions.push(controller.admit(origin, OPERATION_LIMIT)?);
                    buffers.push(touched_bytes(OPERATION_LIMIT));
                }
                let spawn_rejected = controller.admit(origin, 1).is_err();
                ensure!(spawn_rejected, "spawn hard limit accepted excess work");
                let spawn_peak = controller.snapshot(origin);
                ensure!(
                    spawn_peak.worker_domains == WORKER_LIMIT
                        && spawn_peak.in_flight_for_origin == WORKER_LIMIT
                        && spawn_peak.transport_bytes == TRANSPORT_LIMIT,
                    "spawn ledger did not reach each configured hard limit"
                );

                ledger.reserve(&reservations)?;
                let ledger_rejected = ledger.reserve(&[(ResourceKind::QueueSlot, 1)]).is_err();
                ensure!(
                    ledger_rejected,
                    "resource ledger accepted work above its hard limit"
                );
                let overload = snapshot().context("measure overloaded resources")?;
                Ok((load, overload, spawn_rejected, ledger_rejected))
            })?;
        ensure!(
            jit.executable_bytes <= CODE_LIMIT,
            "JIT code allocation {} exceeds {CODE_LIMIT}",
            jit.executable_bytes
        );

        let load_delta = load.delta_from(idle);
        let overload_delta = overload.delta_from(idle);
        check_envelope("load", load_delta)?;
        check_envelope("overload", overload_delta)?;
        ensure!(
            overload_delta.executable_bytes >= u64::try_from(jit.executable_bytes)?,
            "OS did not report the published RX code mapping"
        );

        ledger.release(&reservations)?;
        ensure!(
            ledger.snapshot().used.is_baseline(),
            "ledger did not settle"
        );
        drop(admissions);
        ensure!(
            controller.snapshot(origin).worker_domains == 0
                && controller.snapshot(origin).in_flight_for_origin == 0
                && controller.snapshot(origin).transport_bytes == 0,
            "spawn admissions did not settle"
        );
        drop(buffers);
        drop(descriptor);
        drop(blocking_pool);
        drop(cpu_pool);

        let shutdown = measure_shutdown(idle.threads)?;
        let shutdown_delta = shutdown.delta_from(idle);
        ensure!(
            shutdown_delta.rss_bytes <= RSS_DELTA_LIMIT,
            "shutdown RSS remained above the target envelope"
        );
        ensure!(
            shutdown_delta.virtual_bytes <= 32 * 1024 * 1024,
            "shutdown virtual memory remained above the target envelope"
        );
        ensure!(
            shutdown.threads <= idle.threads,
            "shutdown leaked OS threads: {} remained, baseline was {}",
            shutdown.threads,
            idle.threads,
        );
        ensure!(
            shutdown.file_descriptors <= idle.file_descriptors,
            "shutdown leaked file descriptors"
        );
        ensure!(
            shutdown.executable_bytes <= idle.executable_bytes,
            "shutdown leaked executable code mappings"
        );

        let report = json!({
            "schema": "rinzu.resource-profile",
            "version": 1,
            "target": rinzu_runtime::current_target(),
            "states": {
                "idle": idle.json(),
                "load": load.json(),
                "overload": overload.json(),
                "shutdown": shutdown.json(),
            },
            "deltaFromIdle": {
                "load": load_delta.json(),
                "overload": overload_delta.json(),
                "shutdown": shutdown_delta.json(),
            },
            "hardLimits": {
                "workerDomains": WORKER_LIMIT,
                "inFlightPerOrigin": WORKER_LIMIT,
                "transportBytes": TRANSPORT_LIMIT,
                "operationTransportBytes": OPERATION_LIMIT,
                "codeMemoryBytes": CODE_LIMIT,
                "rssDeltaBytes": RSS_DELTA_LIMIT,
                "virtualMemoryDeltaBytes": VIRTUAL_DELTA_LIMIT,
                "threadDelta": THREAD_DELTA_LIMIT,
                "fileDescriptorDelta": FD_DELTA_LIMIT,
            },
            "verification": {
                "jitExecutableMappings": jit.executable_mappings,
                "jitExecutableBytes": jit.executable_bytes,
                "spawnLimitRejected": spawn_rejected,
                "resourceLedgerLimitRejected": ledger_rejected,
                "spawnLedgerReturnedToBaseline": true,
                "resourceLedgerReturnedToBaseline": true,
                "osEnvelopeSatisfied": true,
            },
        });
        if let Some(path) = output {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(path, serde_json::to_vec_pretty(&report)?)
                .with_context(|| format!("write {}", path.display()))?;
        }
        Ok(report)
    }

    fn touched_bytes(length: usize) -> Vec<u8> {
        let mut bytes = vec![0_u8; length];
        for page in bytes.chunks_mut(4096) {
            page[0] = 0xa5;
        }
        bytes
    }

    fn check_envelope(state: &str, delta: Metrics) -> Result<()> {
        ensure!(
            delta.rss_bytes <= RSS_DELTA_LIMIT,
            "{state} RSS delta {} exceeds {RSS_DELTA_LIMIT}",
            delta.rss_bytes
        );
        ensure!(
            delta.virtual_bytes <= VIRTUAL_DELTA_LIMIT,
            "{state} virtual-memory delta {} exceeds {VIRTUAL_DELTA_LIMIT}",
            delta.virtual_bytes
        );
        ensure!(
            delta.threads <= THREAD_DELTA_LIMIT,
            "{state} thread delta {} exceeds {THREAD_DELTA_LIMIT}",
            delta.threads
        );
        ensure!(
            delta.file_descriptors <= FD_DELTA_LIMIT,
            "{state} file-descriptor delta {} exceeds {FD_DELTA_LIMIT}",
            delta.file_descriptors
        );
        ensure!(
            delta.executable_bytes <= CODE_LIMIT as u64,
            "{state} code-memory delta {} exceeds {CODE_LIMIT}",
            delta.executable_bytes
        );
        Ok(())
    }

    fn measure_shutdown(idle_threads: u64) -> Result<Metrics> {
        // Joined pthreads can still appear in the kernel's task snapshot briefly.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let metrics = snapshot().context("measure shutdown resources")?;
            if metrics.threads <= idle_threads || Instant::now() >= deadline {
                return Ok(metrics);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn snapshot() -> Result<Metrics> {
        // SAFETY: proc_taskinfo is a plain C output record initialized before use.
        let mut task_info: libc::proc_taskinfo = unsafe { zeroed() };
        let task_info_size = i32::try_from(size_of::<libc::proc_taskinfo>())?;
        // SAFETY: the buffer is valid for exactly task_info_size bytes.
        let returned = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                core::ptr::from_mut(&mut task_info).cast(),
                task_info_size,
            )
        };
        ensure!(
            returned == task_info_size,
            "proc_pidinfo(PROC_PIDTASKINFO) failed"
        );
        Ok(Metrics {
            rss_bytes: task_info.pti_resident_size,
            virtual_bytes: task_info.pti_virtual_size,
            threads: u64::try_from(task_info.pti_threadnum)?,
            file_descriptors: open_file_descriptors()?,
            executable_bytes: executable_bytes()?,
        })
    }

    fn open_file_descriptors() -> Result<u64> {
        // SAFETY: getdtablesize returns the process descriptor-table capacity.
        let capacity = unsafe { libc::getdtablesize() };
        ensure!(capacity > 0, "getdtablesize failed");
        let capacity = usize::try_from(capacity)?;
        // SAFETY: proc_fdinfo is a plain output record and zero is a valid bit pattern.
        let mut entries: Vec<libc::proc_fdinfo> =
            (0..capacity).map(|_| unsafe { zeroed() }).collect();
        let bytes = entries
            .len()
            .checked_mul(size_of::<libc::proc_fdinfo>())
            .context("descriptor buffer size overflow")?;
        let bytes = i32::try_from(bytes)?;
        // SAFETY: entries exposes a writable buffer of exactly bytes bytes.
        let returned = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                entries.as_mut_ptr().cast(),
                bytes,
            )
        };
        ensure!(returned >= 0, "proc_pidinfo(PROC_PIDLISTFDS) failed");
        Ok(u64::try_from(returned)? / u64::try_from(size_of::<libc::proc_fdinfo>())?)
    }

    fn executable_bytes() -> Result<u64> {
        // SAFETY: this reads the libSystem task port for the current process.
        let current_task = unsafe { mach_task_self_ };
        let mut address = 0_u64;
        let mut total = 0_u64;
        loop {
            let mut length = 0_u64;
            // SAFETY: the kernel initializes every field selected by the flavor.
            let mut info: VmRegionBasicInfo64 = unsafe { zeroed() };
            let mut count = libc::mach_msg_type_number_t::try_from(
                size_of::<VmRegionBasicInfo64>() / size_of::<i32>(),
            )?;
            let mut object_name = 0;
            // SAFETY: all output pointers refer to initialized, live local storage.
            let status = unsafe {
                mach_vm_region(
                    current_task,
                    &mut address,
                    &mut length,
                    VM_REGION_BASIC_INFO_64,
                    core::ptr::from_mut(&mut info).cast(),
                    &mut count,
                    &mut object_name,
                )
            };
            if status != KERN_SUCCESS {
                break;
            }
            if info.protection & VM_PROT_EXECUTE != 0 {
                total = total.saturating_add(length);
            }
            if object_name != 0 {
                // SAFETY: mach_vm_region returned this send right to the caller.
                unsafe { mach_port_deallocate(current_task, object_name) };
            }
            let Some(next) = address.checked_add(length) else {
                break;
            };
            if next <= address {
                break;
            }
            address = next;
        }
        ensure!(total > 0, "Mach did not report executable regions");
        Ok(total)
    }
}

pub fn run_resource_probe(output: Option<&Path>) -> Result<Value> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        platform::run(output)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = output;
        anyhow::bail!("resource probe requires the aarch64-apple-darwin v1 target")
    }
}
