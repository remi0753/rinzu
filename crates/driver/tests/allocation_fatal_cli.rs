#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_callback_return_sdk/faults.rs"]
mod faults;
#[path = "fixtures/native_callback_return_sdk/setup.rs"]
mod fixture;

fn cli(app: &Path, command: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(command)
        .arg(app)
        .output()
        .unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
fn fatal(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="),
        "{stdout}"
    );
}

#[test]
fn oversized_managed_allocation_is_fatal_in_jit_source_workers_and_callback_bodies() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-allocation-fatal-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let constant = "let TOO_LARGE:CSize=18446744073709551615;";
    let body = "try {let bytes:Bytes(TOO_LARGE)=b\"\";let length=rinzu.core.byteLength(bytes);if length==0 {panic(\"invalid\");}}catch failure {panic(\"wrong\");}finally {let ordinaryCleanup=1;}";
    // Synchronous baseline JIT uses the Rust allocator. Source SDK async CLI
    // now shares AOT's C allocator and its existing immediate-fatal exit code.
    for (main, run_code) in [
        (
            format!("import rinzu.core;{constant}fn main(){{{body}}}"),
            71,
        ),
        (
            format!("import rinzu.core;{constant}async fn main(){{{body}}}"),
            70,
        ),
        (
            format!(
                "import rinzu.core;{constant}fn fail()->Unit {{{body}}}async fn main(){{let returned=await spawn fail();}}"
            ),
            70,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        success(&cli(&app, "build"));
        fatal(&cli(&app, "run"), run_code);
        fatal(
            &Command::new(app.join("build/return_app")).output().unwrap(),
            70,
        );
    }
    std::fs::write(root.join("returns/api.c"), faults::source()).unwrap();
    let sdk = root.join("returns/src/api.rinzu");
    let source = std::fs::read_to_string(&sdk).unwrap();
    let label = source.find("fn label(").unwrap();
    let body = source[label..].replacen(
        "    try {\n",
        "    try {\n        let impossible:Bytes(TOO_LARGE)=b\"\";let size=rinzu.core.byteLength(impossible);\n",
        1,
    );
    let source = format!("{}{body}", &source[..label]);
    let source = source.replacen(
        "import rinzu.core;",
        &format!("import rinzu.core;{constant}"),
        1,
    );
    std::fs::write(sdk, source).unwrap();
    for (main, run_code) in [
        (
            "import returns;fn main(){let returned=returns.exercise();}",
            71,
        ),
        (
            "import returns;async fn main(){let returned=returns.exercise();}",
            70,
        ),
        (
            "import returns;async fn main(){let returned=await spawn returns.exercise();}",
            70,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        success(&cli(&app, "build"));
        for (output, code) in [
            (cli(&app, "run"), run_code),
            (
                Command::new(app.join("build/return_app")).output().unwrap(),
                70,
            ),
        ] {
            fatal(&output, code);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("RETURN_NATIVE_STRING_ENTER") && !stderr.contains("RETURN_FINALLY"),
                "{stderr}"
            );
            assert_eq!(stderr.matches("RETURN_NATIVE_STRING_ENTER").count(), 1);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_and_worker_os_memory_failure_is_fatal_but_missing_artifact_and_admission_are_distinct() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-enomem-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(root.join("returns/api.c"), faults::source()).unwrap();
    let app = root.join("app");
    success(&cli(&app, "build"));
    let runtime = std::fs::read_to_string(app.join("build/return_app.runtime.c")).unwrap();
    let prelude = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <dlfcn.h>
#include <sys/stat.h>
#include <pthread.h>
static inline void *rinzu_async_thread(void *);
static inline void *rinzu_activity_watch(void *);
static int testing(const char *phase){const char *test=getenv("RINZU_TEST_OS_MEMORY");return test && !strcmp(test,phase);}
static FILE *memory_open(const char *path,const char *mode){if(testing("open")||testing("missing")){errno=testing("open")?ENOMEM:ENOENT;return NULL;}return fopen(path,mode);}
static size_t memory_read(void *bytes,size_t size,size_t count,FILE *file){if(testing("read")){errno=ENOMEM;return 0;}return fread(bytes,size,count,file);}
static int memory_error(FILE *file){if(testing("read"))return 1;return ferror(file);}
static int memory_close(FILE *file){int status=fclose(file);if(testing("close")){errno=ENOMEM;return EOF;}return status;}
static void *memory_load(const char *path,int mode){if(testing("load")){errno=ENOMEM;return NULL;}return dlopen(path,mode);}
static void *memory_symbol(void *handle,const char *name){static int calls;calls++;if(testing("symbol")||(testing("import")&&calls>1)){errno=ENOMEM;return NULL;}return dlsym(handle,name);}
static int memory_stat(const char *path,struct stat *value){if(testing("stat")){errno=ENOMEM;return -1;}return stat(path,value);}
static char *memory_resolve(const char *path,char *resolved){if(testing("resolve")){errno=ENOMEM;return NULL;}return realpath(path,resolved);}
static int memory_thread(pthread_t *thread,const pthread_attr_t *attributes,void *(*body)(void *),void *context){if(testing("watchdog") && body==rinzu_activity_watch)return ENOMEM;if(body==rinzu_async_thread){if(testing("thread"))return ENOMEM;if(testing("admit"))return EAGAIN;}return pthread_create(thread,attributes,body,context);}
#define fopen memory_open
#define fread memory_read
#define ferror memory_error
#define fclose memory_close
#define dlopen memory_load
#define dlsym memory_symbol
#define stat(path,value) memory_stat(path,value)
#define realpath memory_resolve
#define pthread_create memory_thread
"#;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let path = app.join(if sanitized {
            "build/os-memory-sanitized.c"
        } else {
            "build/os-memory.c"
        });
        let binary = path.with_extension("");
        std::fs::write(&path, format!("{prelude}\n{runtime}")).unwrap();
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
            .arg(app.join("build/return_app.o"))
            .arg(path)
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        success(&command.arg("-o").arg(&binary).output().unwrap());
        for phase in [
            "open", "read", "close", "load", "symbol", "import", "stat", "resolve", "thread",
            "missing", "admit", "watchdog",
        ] {
            let output = Command::new(&binary)
                .env("RINZU_TEST_OS_MEMORY", phase)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stderr);
            let code = match phase {
                "thread" => 70,
                "missing" => 78,
                "admit" => 1,
                _ => 71,
            };
            assert_eq!(
                output.status.code(),
                Some(code),
                "{phase}/{sanitized}: {stderr}\n{stdout}"
            );
            if phase == "admit" {
                assert!(
                    stdout.contains("RINZU_FAILURE=overloaded")
                        && stdout.contains("RINZU_NATIVE_CALLS=70,70,70,70,70,70,56,160,0,0,0"),
                    "{stdout}"
                );
            } else {
                assert!(
                    !stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="),
                    "{stdout}"
                );
            }
            if phase == "thread" || phase == "admit" {
                assert_eq!(stderr.matches("RETURN_NATIVE_STRING_ENTER").count(), 5);
            } else {
                assert!(!stderr.contains("RETURN_NATIVE_STRING_ENTER"), "{stderr}");
            }
            assert!(
                !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
