#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "../../../sdk/sdl2/tests/platform.rs"]
mod platform;
use std::path::Path;
use std::process::{Command, Output};

fn baseline(output: &Output) {
    platform::success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("RINZU_FAILURE=none") && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("RINZU_STARTED_CHILDREN=1,1") && stdout.contains("RINZU_STARTED_TIMERS=2"),
        "{stdout}"
    );
    let calls = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_NATIVE_CALLS="))
        .unwrap()
        .split(',')
        .map(|field| field.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert!(
        calls[0] > 20 && calls[..6].iter().all(|count| *count == calls[0]) && calls[6..] == [0; 5],
        "{stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("runtime error:"),
        "{output:?}"
    );
}

#[test]
fn sdl_poll_normalizes_the_real_union_and_releases_window_renderer_and_video() {
    let root = std::env::temp_dir().join(format!("rinzu-sdl-source-poll-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/sdl2");
    let config = |flag| {
        let output = Command::new("sdl2-config").arg(flag).output().unwrap();
        platform::success(&output);
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .args(config("--cflags"))
            .arg("-I")
            .arg(sdk.join("native/include"))
            .arg(sdk.join("native/src/sdl2_source.c"))
            .arg(sdk.join("tests/poll.c"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        platform::success(&platform::bounded(
            clang.args(config("--libs")).arg("-o").arg(&binary),
        ));
        if !sanitized {
            platform::sign(&binary, &root);
        }
        platform::success(&platform::bounded(
            Command::new(&binary)
                .env("SDL_VIDEODRIVER", "dummy")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        ));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdl_source_sdk_uses_managed_poll_timer_loop_io_spawn_and_gc_in_jit_aot_signed_and_sanitizers() {
    let root =
        std::env::temp_dir().join(format!("rinzu-sdl-source-platform-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let app = platform::ordinary_import(&root);
    let cli = |action| {
        platform::bounded(
            Command::new(frozen_cli::path())
                .arg(action)
                .arg(&app)
                .env("SDL_VIDEODRIVER", "dummy"),
        )
    };
    platform::success(&cli("check"));
    let jit = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .arg("--trace")
            .env("SDL_VIDEODRIVER", "dummy"),
    );
    platform::success(&jit);
    let trace = String::from_utf8_lossy(&jit.stderr);
    assert!(
        trace.contains("compiled JIT continuation runtime")
            && trace.contains("\"operation\": \"timer\"")
            && trace.contains("\"resources\": \"baseline\""),
        "{trace}"
    );
    platform::success(&cli("build"));
    let binary = app.join("build/sdl_source_app");
    baseline(&platform::bounded(
        Command::new(&binary).env("SDL_VIDEODRIVER", "dummy"),
    ));
    let sanitized = platform::sanitize(&binary);
    baseline(&platform::bounded(
        Command::new(&sanitized)
            .env("SDL_VIDEODRIVER", "dummy")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    baseline(&platform::bounded(
        Command::new(&binary).env("SDL_VIDEODRIVER", "dummy"),
    ));
    platform::success(&cli("verify-aot"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdl_cocoa_backend_runs_an_actual_main_window_without_native_loop_hooks() {
    let root = std::env::temp_dir().join(format!("rinzu-sdl-source-window-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let app = platform::ordinary_import(&root);
    let jit = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .env("SDL_VIDEODRIVER", "cocoa"),
    );
    platform::success(&jit);
    assert!(String::from_utf8_lossy(&jit.stderr).contains("compiled JIT continuation runtime"));
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/sdl_source_app");
    platform::sign(&binary, &root);
    baseline(&platform::bounded(
        Command::new(&binary).env("SDL_VIDEODRIVER", "cocoa"),
    ));
    let sanitized = platform::sanitize(&binary);
    baseline(&platform::bounded(
        Command::new(&sanitized)
            .env("SDL_VIDEODRIVER", "cocoa")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdl_invalid_native_record_writeback_exits_86_before_catch_or_finally() {
    let root =
        std::env::temp_dir().join(format!("rinzu-sdl-source-storage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for path in [
        "app/src",
        "sdl2/src",
        "sdl2/native/src",
        "sdl2/native/include",
    ] {
        std::fs::create_dir_all(root.join(path)).unwrap();
    }
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/sdl2");
    for path in [
        "rinzu.toml",
        "src/sdl2.rinzu",
        "native/src/sdl2_source.c",
        "native/include/rinzu_sdl2_source.h",
    ] {
        std::fs::copy(sdk.join(path), root.join("sdl2").join(path)).unwrap();
    }
    let io = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/io")
        .canonicalize()
        .unwrap();
    let depth = root
        .join("sdl2")
        .canonicalize()
        .unwrap()
        .components()
        .count()
        - 1;
    let relative = format!(
        "{}{}",
        "../".repeat(depth),
        io.strip_prefix("/").unwrap().display()
    );
    let manifest = root.join("sdl2/rinzu.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("path = \"../io\"", &format!("path = \"{relative}\"")),
    )
    .unwrap();
    let source = root.join("sdl2/native/src/sdl2_source.c");
    let native = std::fs::read_to_string(&source).unwrap();
    let create = concat!(
        "state->window = window;\n  state->renderer = renderer;\n",
        "  state->running = 1;\n  active = state;\n  return state;"
    );
    let poll = concat!(
        "*output = (struct Event){event.type, window, !!quit, ++state->sequence};\n",
        "  return 1;"
    );
    assert_eq!(native.matches(create).count(), 1);
    assert_eq!(native.matches(poll).count(), 1);
    let native = native
        .replace(
            create,
            concat!(
                "state->window = window;\n  state->renderer = renderer;\n",
                "  state->running = 1;\n  active = state;\n",
                "  SDL_Event injected;\n  SDL_zero(injected);\n",
                "  injected.type = SDL_USEREVENT;\n",
                "  if (SDL_PushEvent(&injected) != 1) _Exit(86);\n  return state;"
            ),
        )
        .replace(
            poll,
            concat!(
                "*output = (struct Event){event.type, window, !!quit, ++state->sequence};\n",
                "  static int invalid_once = 1;\n  if (invalid_once) {\n",
                "    invalid_once = 0;\n    *((unsigned char *)&output->shouldQuit) = 2;\n",
                "  }\n  return 1;"
            ),
        );
    std::fs::write(source, native).unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"sdl_record_fault\"\nlanguage=\"0.2\"\n[dependencies]\n\"rinzu.sdl2\"={path=\"../sdl2\"}\n").unwrap();
    std::fs::write(
        root.join("app/src/main.rinzu"),
        r#"
import rinzu.sdl2;
import rinzu.stdio;
fn event(window:rinzu.sdl2.WindowHandle,value:rinzu.sdl2.Event) {}
fn frame(window:rinzu.sdl2.WindowHandle,value:Int) {}
async fn main() {
    try {let value=await rinzu.sdl2.run("invalid native record",event,frame,0);}
    catch errors {panic("catch resumed");}
    finally {panic("finally resumed");}
}
"#,
    )
    .unwrap();
    let app = root.join("app");
    let check = |output: Output| {
        assert_eq!(output.status.code(), Some(86), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    };
    check(platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .env("SDL_VIDEODRIVER", "dummy"),
    ));
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .env("SDL_VIDEODRIVER", "dummy"),
    ));
    let binary = app.join("build/sdl_record_fault");
    check(platform::bounded(
        Command::new(&binary).env("SDL_VIDEODRIVER", "dummy"),
    ));
    let sanitized = platform::sanitize(&binary);
    check(platform::bounded(
        Command::new(sanitized)
            .env("SDL_VIDEODRIVER", "dummy")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    check(platform::bounded(
        Command::new(&binary).env("SDL_VIDEODRIVER", "dummy"),
    ));
    std::fs::remove_dir_all(root).unwrap();
}
