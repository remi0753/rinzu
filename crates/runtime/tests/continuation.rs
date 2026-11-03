#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[test]
fn signed_native_continuations_preserve_main_thread_stacks_and_settle_mappings() {
    let directory = std::env::temp_dir().join(format!(
        "rinzu-continuation-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("continuation.c");
    let executable = directory.join("continuation");
    std::fs::write(&source, r#"
#include <assert.h>
#include "rinzu/continuation.h"
typedef struct {
  rinzu_continuation_t continuation;
  unsigned first, second, final;
  pthread_t main_thread;
  uint64_t fpcr, fpsr;
} Task;
static unsigned sequence;
static void recursive(Task *task, unsigned depth) {
  volatile uint64_t local[3] = {depth, UINT64_C(0x8000000000000000) + depth, UINT64_MAX - depth};
  if (depth) recursive(task, depth - 1);
  else {
    assert(pthread_equal(task->main_thread, pthread_self()));
    uint64_t magic = (uintptr_t)task;
    union { uint64_t bits; double value; } floating_magic = {.bits = magic};
    register uint64_t integer __asm__("x21") = magic;
    register double floating __asm__("d8") = floating_magic.value;
    __asm__ volatile("" : "+r"(integer), "+w"(floating));
    task->first = ++sequence;
    rinzu_continuation_yield(&task->continuation);
    __asm__ volatile("" : "+r"(integer), "+w"(floating));
    assert(integer == magic && floating == floating_magic.value);
    task->second = ++sequence;
    rinzu_continuation_yield(&task->continuation);
    task->final = ++sequence;
  }
  assert(local[0] == depth && local[1] == UINT64_C(0x8000000000000000) + depth && local[2] == UINT64_MAX - depth);
}
static void body(void *raw) {
  Task *task=raw; uint64_t fpcr,fpsr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  __asm__ volatile("mrs %0, fpsr" : "=r"(fpsr));
  assert(fpcr==task->fpcr && fpsr==task->fpsr);
  recursive(raw, 12);
}
static void never_started(void *raw) { (void)raw; assert(0); }
static void *foreign_resume(void *raw) { rinzu_continuation_owner(raw); return NULL; }
typedef struct {
  rinzu_continuation_t continuation;
  rinzu_continuation_accounting_t *accounting;
  unsigned steps;
} Nested;
static void child_body(void *raw) {
  Nested *child = raw; child->steps += 1;
  rinzu_continuation_yield(&child->continuation); child->steps += 1;
}
static void parent_body(void *raw) {
  Nested *parent = raw, child = {0};
  rinzu_continuation_init(&child.continuation, 64 * 1024, child_body, &child, parent->accounting);
  rinzu_continuation_resume(&parent->continuation, &child.continuation);
  assert(child.steps == 1); parent->steps += 1;
  rinzu_continuation_yield(&parent->continuation);
  rinzu_continuation_resume(&parent->continuation, &child.continuation);
  assert(child.steps == 2 && child.continuation.finished);
  rinzu_continuation_dispose(&child.continuation); parent->steps += 1;
}
int main(int argc, char **argv) {
  rinzu_continuation_t root; rinzu_continuation_init_root(&root);
  rinzu_continuation_accounting_t accounting = {0};
  Task first = {0}, second = {0}; first.main_thread = second.main_thread = pthread_self();
  uint64_t original_fpcr,original_fpsr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(original_fpcr));
  __asm__ volatile("mrs %0, fpsr" : "=r"(original_fpsr));
  first.fpcr=original_fpcr ^ (UINT64_C(1)<<22); first.fpsr=original_fpsr ^ UINT64_C(16);
  second.fpcr=original_fpcr ^ (UINT64_C(1)<<23); second.fpsr=original_fpsr;
  __asm__ volatile("msr fpcr, %0\nmsr fpsr, %1" :: "r"(first.fpcr),"r"(first.fpsr));
  rinzu_continuation_init(&first.continuation, 64 * 1024, body, &first, &accounting);
  __asm__ volatile("msr fpcr, %0\nmsr fpsr, %1" :: "r"(second.fpcr),"r"(second.fpsr));
  rinzu_continuation_init(&second.continuation, 64 * 1024, body, &second, &accounting);
  __asm__ volatile("msr fpcr, %0\nmsr fpsr, %1" :: "r"(original_fpcr),"r"(original_fpsr));
  assert(accounting.live == 2 && accounting.peak_live == 2 && !accounting.mapped_bytes);
  assert(!first.continuation.mapping && !second.continuation.mapping);
  if (argc>1 && !strcmp(argv[1],"cache-live")) rinzu_continuation_cache_dispose(&accounting);
  if (argc>1 && !strcmp(argv[1],"foreign-unstarted")) {
    pthread_t thread; assert(!pthread_create(&thread,NULL,foreign_resume,&second.continuation));
    pthread_join(thread,NULL); return 1;
  }
  rinzu_continuation_resume(&root, &first.continuation); assert(first.first == 1 && !first.second); ++sequence;
  if (argc > 1) {
    if (!strcmp(argv[1], "pending-dispose")) rinzu_continuation_dispose(&first.continuation);
    else if (!strcmp(argv[1], "foreign")) {
      pthread_t thread; assert(!pthread_create(&thread, NULL, foreign_resume, &first.continuation));
      pthread_join(thread, NULL);
    } else if (!strcmp(argv[1], "guard")) *(volatile unsigned char *)first.continuation.mapping = 1;
    else if (!strcmp(argv[1],"upper-guard"))
      *((volatile unsigned char *)first.continuation.mapping+first.continuation.mapping_size-1)=1;
    return 1;
  }
  rinzu_continuation_resume(&root, &second.continuation); assert(second.first == 3); ++sequence;
  assert(first.continuation.mapping != second.continuation.mapping && accounting.allocations==2);
  assert(accounting.mapped_bytes > 128*1024);
  rinzu_continuation_resume(&root, &first.continuation); assert(first.second == 5); ++sequence;
  rinzu_continuation_resume(&root, &second.continuation); assert(second.second == 7); ++sequence;
  rinzu_continuation_resume(&root, &second.continuation); assert(second.final == 9 && second.continuation.finished); ++sequence;
  rinzu_continuation_resume(&root, &first.continuation); assert(first.final == 11 && first.continuation.finished);
  assert(root.running && !first.continuation.running && !second.continuation.running);
  rinzu_continuation_dispose(&first.continuation); rinzu_continuation_dispose(&second.continuation);
  assert(!accounting.live && accounting.mapped_bytes == accounting.cached_bytes);
  rinzu_continuation_cache_dispose(&accounting);
  assert(!accounting.live && !accounting.mapped_bytes);
  Nested nested = {0}; nested.accounting = &accounting;
  rinzu_continuation_init(&nested.continuation, 64 * 1024, parent_body, &nested, &accounting);
  rinzu_continuation_resume(&root, &nested.continuation);
  assert(nested.steps == 1 && accounting.live == 2);
  rinzu_continuation_resume(&root, &nested.continuation);
  assert(nested.steps == 2 && accounting.live == 1);
  rinzu_continuation_dispose(&nested.continuation);
  size_t allocations=accounting.allocations;
  for (unsigned i = 0; i < 256; ++i) {
    rinzu_continuation_t unstarted;
    rinzu_continuation_init(&unstarted, 64 * 1024, never_started, NULL, &accounting);
    rinzu_continuation_dispose(&unstarted);
  }
  assert(accounting.allocations==allocations);
  assert(!accounting.live && accounting.mapped_bytes == accounting.cached_bytes);
  rinzu_continuation_cache_dispose(&accounting);
  assert(!accounting.live && !accounting.mapped_bytes && accounting.peak_live == 2);
  return 0;
}
"#).unwrap();
    let output = Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .arg("-I")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let output = Command::new(&executable).output().unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    for rejected in [
        "pending-dispose",
        "foreign",
        "foreign-unstarted",
        "cache-live",
    ] {
        let output = Command::new(&executable).arg(rejected).output().unwrap();
        assert_eq!(output.status.code(), Some(70), "{rejected}: {output:?}");
    }
    use std::os::unix::process::ExitStatusExt;
    for mode in ["guard", "upper-guard"] {
        let guard = Command::new(&executable).arg(mode).output().unwrap();
        assert!(guard.status.signal().is_some(), "{guard:?}");
    }
    let probe = directory.join("cpu-probe");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = Command::new("clang")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-DRINZU_EXPECT_DEFERRED_STACK=1",
        ])
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("crates/runtime/examples/continuation_cpu_probe.c"))
        .arg("-o")
        .arg(&probe)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let result = Command::new(&probe).arg("verify").output().unwrap();
    assert!(result.status.success(), "{result:?}");
    std::fs::remove_dir_all(directory).unwrap();
}
