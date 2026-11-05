#![cfg(unix)]
use rinzu_runtime::allocation::FatalAllocator;
use rinzu_runtime::data::ManagedData;
use rinzu_runtime::failure::PendingFailure;
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::time::{Duration, Instant};

thread_local! {static FAIL: Cell<bool> = const {Cell::new(false)};static FAILED: Cell<bool> = const {Cell::new(false)};}
thread_local! {
    static COLLECTION_ALLOCATIONS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}
fn record_collection_allocation(bytes: usize) {
    if let Some((calls, total)) = COLLECTION_ALLOCATIONS.get() {
        COLLECTION_ALLOCATIONS.set(Some((calls + 1, total + bytes)));
    }
}
struct ControlledSystem;
fn fail() -> bool {
    if !FAIL.with(Cell::get) {
        return false;
    }
    if FAILED.with(|value| value.replace(true)) {
        // SAFETY: a second allocation after the injected failure violates the gate.
        unsafe { libc::_exit(90) }
    }
    true
}
// SAFETY: successful operations retain System's pointer/layout contracts; the
// private thread-local switch injects exactly one NULL before FatalAllocator exits.
unsafe impl GlobalAlloc for ControlledSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        record_collection_allocation(layout.size());
        // SAFETY: unchanged valid layout is delegated to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        record_collection_allocation(layout.size());
        // SAFETY: preserve System's zeroing and alignment contract.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        record_collection_allocation(size);
        // SAFETY: System receives its live pointer/layout and requested new size.
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: free the original System allocation using its exact layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: FatalAllocator<ControlledSystem> = FatalAllocator(ControlledSystem);

#[test]
fn growing_collection_storage_uses_geometric_allocation() {
    use rinzu_abi::data_operation::DataOperation::*;
    use rinzu_runtime::data::ManagedSlot;
    use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
    use rinzu_runtime::value::{ValueSchema, ValueType};

    let schema = Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::I64),
            ValueType::List(0),
            ValueType::Map { key: 0, value: 0 },
        ])
        .unwrap(),
    );
    for n in [1024, 2048] {
        for (ty, operation) in [(1, ListBufferAppend), (2, MapBufferInsert)] {
            let mut heap = DomainHeap::new(1);
            let mut data = ManagedData::new(1);
            data.configure_values(&heap, Arc::clone(&schema)).unwrap();
            let empty = data.create_composite(&mut heap, ty, None, &[]).unwrap();
            let mut handle = empty;
            COLLECTION_ALLOCATIONS.set(Some((0, 0)));
            for index in 0..n {
                let item =
                    ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, index).unwrap());
                let slots = [ManagedSlot::Reference(handle), item, item];
                let ManagedSlot::Reference(next) = data
                    .operation(
                        &mut heap,
                        operation,
                        ty,
                        &[ty, 0, 0][..operation.arity()],
                        &slots[..operation.arity()],
                    )
                    .unwrap()
                else {
                    panic!("collection update");
                };
                if next != handle && handle != empty {
                    data.release(&mut heap, handle).unwrap();
                }
                handle = next;
            }
            if operation == MapBufferInsert {
                let before = COLLECTION_ALLOCATIONS.get();
                let key = ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, 3).unwrap());
                for index in 0..n {
                    let value = ManagedSlot::Scalar(
                        ScalarValue::from_bits(ScalarKind::I64, index).unwrap(),
                    );
                    let slots = [ManagedSlot::Reference(handle), key, value];
                    assert_eq!(
                        data.operation(&mut heap, operation, ty, &[ty, 0, 0], &slots)
                            .unwrap(),
                        slots[0]
                    );
                }
                assert_eq!(COLLECTION_ALLOCATIONS.get(), before);
            }
            let _ = data
                .operation(
                    &mut heap,
                    CollectionFinish,
                    ty,
                    &[ty],
                    &[ManagedSlot::Reference(handle)],
                )
                .unwrap();
            let (calls, bytes) = COLLECTION_ALLOCATIONS.replace(None).unwrap();
            assert!(
                calls < 32 && bytes < n as usize * 1024,
                "{operation:?}: {calls} calls, {bytes} bytes"
            );
            eprintln!("{operation:?} {n}: {calls} allocator requests for {bytes} bytes");
        }
    }
}

fn seal_stderr() {
    // SAFETY: child-private pipe remains unread, filled with nonblocking writes;
    // restoring blocking mode models a stuck stderr without consuming OS memory.
    unsafe {
        let mut pipe = [0; 2];
        assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        let flags = libc::fcntl(pipe[1], libc::F_GETFL);
        assert!(flags >= 0);
        assert_eq!(
            libc::fcntl(pipe[1], libc::F_SETFL, flags | libc::O_NONBLOCK),
            0
        );
        let bytes = [0u8; 4096];
        while libc::write(pipe[1], bytes.as_ptr().cast(), bytes.len()) > 0 {}
        assert_eq!(libc::fcntl(pipe[1], libc::F_SETFL, flags), 0);
        assert_eq!(
            libc::dup2(pipe[1], libc::STDERR_FILENO),
            libc::STDERR_FILENO
        );
    }
}
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        // SAFETY: static marker detects any erroneous unwind/normal cleanup.
        unsafe {
            libc::write(1, b"CLEANUP_RAN\n".as_ptr().cast(), 12);
        }
    }
}

#[test]
fn allocation_failure_child() {
    let Ok(mode) = std::env::var("RINZU_TEST_FATAL_ALLOCATION") else {
        return;
    };
    let ready = Arc::new(Barrier::new(2));
    let blocked = Arc::new((Mutex::new(false), Condvar::new()));
    let child_ready = Arc::clone(&ready);
    let child_blocked = Arc::clone(&blocked);
    let _worker = std::thread::spawn(move || {
        let (gate, condition) = &*child_blocked;
        let mut stopped = gate.lock().unwrap();
        child_ready.wait();
        while !*stopped {
            stopped = condition.wait(stopped).unwrap();
        }
        panic!("OOM must not ask the blocked Worker to cooperate");
    });
    ready.wait();
    let _cleanup = Cleanup;
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    let object = heap
        .allocate(1, &[HeapValue::Integer(42)], FinalizerPolicy::None)
        .unwrap();
    if mode == "gc" {
        heap.create_root(Some(object)).unwrap();
    }
    let layout = Layout::from_size_align(64, 16).unwrap();
    let pointer = if mode == "realloc" {
        // SAFETY: valid nonzero layout; failure policy is not armed yet.
        unsafe { ALLOCATOR.alloc(layout) }
    } else {
        std::ptr::null_mut()
    };
    let mut pending = PendingFailure::default();
    let transitions = Arc::new(rinzu_runtime::transition::TransitionMachine::new());
    transitions.activate_managed().unwrap();
    seal_stderr();
    FAILED.with(|value| value.set(false));
    FAIL.with(|value| value.set(true));
    match mode.as_str() {
        "native" => {
            let _ = transitions.native_scope(rinzu_runtime::transition::RootPublication {
                stack_map_id: 42,
                root_count: 0,
            });
        }
        // SAFETY: each direct probe supplies the required valid layout/pointer.
        "alloc" => unsafe {
            std::hint::black_box(ALLOCATOR.alloc(layout));
        },
        // SAFETY: the layout is valid and requires zeroed initialized bytes.
        "zeroed" => unsafe {
            std::hint::black_box(ALLOCATOR.alloc_zeroed(layout));
        },
        // SAFETY: pointer/layout identify the live prepared allocation.
        "realloc" => unsafe {
            std::hint::black_box(ALLOCATOR.realloc(pointer, layout, 128));
        },
        "managed" => {
            let _ = std::hint::black_box(data.create_string(&mut heap, "normal managed value"));
        }
        "root" => {
            let _ = std::hint::black_box(heap.create_root(Some(object)));
        }
        "gc" => {
            let _ = std::hint::black_box(heap.collect(CollectionKind::Full));
        }
        "diagnostic" => {
            pending.begin_cleanup();
        }
        "explicit" => rinzu_runtime::allocation::fatal(),
        "io" => {
            let _ = rinzu_runtime::allocation::checked_io::<()>(Err(
                std::io::Error::from_raw_os_error(libc::ENOMEM),
            ));
        }
        _ => panic!("unknown child probe"),
    }
    FAIL.with(|value| value.set(false));
    panic!("allocation failure incorrectly returned to managed execution");
}

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("fatal path waited for output or Worker cooperation");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn rust_allocation_failure_exits_without_output_unwind_or_blocked_worker_cooperation() {
    for mode in [
        "alloc",
        "zeroed",
        "realloc",
        "managed",
        "root",
        "gc",
        "diagnostic",
        "explicit",
        "io",
        "native",
    ] {
        let output = bounded_output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "allocation_failure_child", "--nocapture"])
                .env("RINZU_TEST_FATAL_ALLOCATION", mode),
        );
        assert_eq!(output.status.code(), Some(71), "{mode}: {output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CLEANUP_RAN"));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn os_errors_preserve_success_and_non_memory_failures() {
    assert!(rinzu_runtime::allocation::checked_io(Ok(())).is_ok());
    for code in [libc::ENOENT, libc::EIO, libc::EINVAL, libc::EAGAIN] {
        let error = std::io::Error::from_raw_os_error(code);
        assert_eq!(
            rinzu_runtime::allocation::checked_io::<()>(Err(error))
                .unwrap_err()
                .raw_os_error(),
            Some(code)
        );
    }
}

#[test]
fn c_allocation_fatal_exits_with_stderr_full_and_a_native_thread_blocked() {
    let root =
        std::env::temp_dir().join(format!("rinzu-c-fatal-allocation-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("test.c");
    std::fs::write(&path,r#"
#include <assert.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdlib.h>
#include <unistd.h>
static void *controlled_calloc(size_t count,size_t size){(void)count;(void)size;return NULL;}
#define calloc controlled_calloc
#include "rinzu/failure.h"
static pthread_mutex_t gate=PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t condition=PTHREAD_COND_INITIALIZER;
static int ready;
static void *blocked(void *unused){(void)unused;assert(!pthread_mutex_lock(&gate));ready=1;assert(!pthread_cond_signal(&condition));for(;;)assert(!pthread_cond_wait(&condition,&gate));return NULL;}
static void cleanup(void){write(1,"CLEANUP_RAN\n",12);}
int main(void){
  pthread_t worker;assert(!pthread_create(&worker,NULL,blocked,NULL));
  assert(!pthread_mutex_lock(&gate));while(!ready)assert(!pthread_cond_wait(&condition,&gate));assert(!pthread_mutex_unlock(&gate));
  assert(!atexit(cleanup));int pipes[2];assert(!pipe(pipes));int flags=fcntl(pipes[1],F_GETFL);assert(flags>=0);
  assert(!fcntl(pipes[1],F_SETFL,flags|O_NONBLOCK));unsigned char bytes[4096]={0};while(write(pipes[1],bytes,sizeof(bytes))>0){}
  assert(!fcntl(pipes[1],F_SETFL,flags));assert(dup2(pipes[1],2)==2);
  rinzu_failure_allocate(64);return 0;
}
"#).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "ordinary" });
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-O1", "-g", "-pthread", "-Werror"])
            .arg(&path)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"));
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = command.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = bounded_output(Command::new(binary).env("ASAN_OPTIONS", "detect_leaks=0"));
        assert_eq!(output.status.code(), Some(71), "{output:?}");
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}
