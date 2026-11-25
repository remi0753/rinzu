//! Allow dyld to check suitable, private bundle images during JIT compilation.
use crate::native_clang::TargetClang;
use crate::source_native_package::BuiltSourcePackage;
use object::{ExportTarget, FileFlags, Object, ObjectKind, ObjectSection, SectionFlags, macho};
use rinzu_bindgen::native_binding::NativeBindingPlan;
use rinzu_bindgen::source_package::{sha256, validate_source_descriptor};
use std::path::Path;

pub(super) fn during_compile<T>(
    built: &crate::native_package::BuiltNativePackages,
    bundle: &Path,
    toolchain: &TargetClang,
    publish: impl Fn() -> Result<(), String> + Sync,
    compile: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !built
        .source_packages
        .iter()
        .any(|package| package.artifact.library.is_some())
        || !std::thread::available_parallelism().is_ok_and(|count| count.get() > 1)
        || std::env::vars_os().any(|(name, _)| {
            let name = name.as_encoded_bytes();
            name.starts_with(b"DYLD_") || name.starts_with(b"LD_")
        })
    {
        publish()?;
        return compile();
    }
    std::thread::scope(|scope| {
        // Runtime generation uses the checked inputs in the build directory.
        // Verify and publish the copies before opening any of their images.
        let worker = std::thread::Builder::new()
            .name("rinzu-native-preload".into())
            .spawn_scoped(scope, || {
                publish()?;
                warm(&built.source_packages, bundle, &built.directory, toolchain);
                Ok::<_, String>(())
            });
        match worker {
            Ok(worker) => {
                let library = compile();
                let _wait = rinzu_compiler::timing::span("jit.runtime.preload.join");
                worker
                    .join()
                    .map_err(|_| "native bundle preparation panicked".to_owned())??;
                library
            }
            Err(_) => {
                publish()?;
                compile()
            }
        }
    })
}

fn warm(packages: &[BuiltSourcePackage], bundle: &Path, directory: &Path, toolchain: &TargetClang) {
    let mut timing = rinzu_compiler::timing::span("jit.runtime.native_preload");
    let mut images = Vec::new();
    for package in packages {
        let (Some(library), Some(digest)) =
            (&package.artifact.library, &package.artifact.facade_digest)
        else {
            continue;
        };
        let (Some(name), Ok(descriptor)) = (
            library.file_name(),
            validate_source_descriptor(&package.artifact.expected_wire),
        ) else {
            continue;
        };
        let root = bundle.join(crate::source_package_bundle::package_directory(
            &descriptor.interface.package.package,
        ));
        let facade = root.join(name);
        if has_passive_image(&facade, digest) {
            images.push(facade);
        }
        if let Ok(Some(plan)) =
            NativeBindingPlan::decode(&descriptor.interface.package.build_inputs)
        {
            for image in plan.images.iter().filter(|image| image.bundled) {
                let path = root.join(&image.path);
                if image
                    .sha256
                    .as_deref()
                    .is_some_and(|digest| has_passive_image(&path, digest))
                {
                    images.push(path);
                }
            }
        }
    }
    timing.set_counter("candidates", images.len() as u64);
    if images.is_empty() {
        return;
    }
    let source = directory.join("native-preload.c");
    let helper = directory.join("native-preload");
    if std::fs::write(&source, include_str!("native_preload.c")).is_err() {
        return;
    }
    let helper_link = rinzu_compiler::timing::span("jit.runtime.preload.link");
    let built = toolchain
        .executable_command()
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
        .arg(&source)
        .arg("-o")
        .arg(&helper)
        .output()
        .is_ok_and(|output| output.status.success());
    drop(helper_link);
    if !built || toolchain.verify().is_err() {
        return;
    }
    // Two helper processes can check separate files while clang builds the JIT
    // runtime. The real startup still dlopens and validates every table/provider.
    let _helpers = rinzu_compiler::timing::span("jit.runtime.preload.helpers");
    let groups = images.chunks(images.len().div_ceil(2));
    let mut children = Vec::new();
    for group in groups {
        if let Ok(child) = std::process::Command::new(&helper)
            .args(group)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            children.push((child, group.len() as u64));
        }
    }
    let mut loaded = 0;
    for (mut child, count) in children {
        if child.wait().is_ok_and(|status| status.success()) {
            loaded += count;
        }
    }
    timing.set_counter("warmed", loaded);
}

fn has_passive_image(path: &Path, expected: &str) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    std::fs::read(path).is_ok_and(|bytes| sha256(&bytes) == expected && passive_image(&bytes))
}

fn passive_image(bytes: &[u8]) -> bool {
    let Ok(object::File::MachO64(file)) = object::File::parse(bytes) else {
        return false;
    };
    if file.kind() != ObjectKind::Dynamic || file.architecture() != object::Architecture::Aarch64 {
        return false;
    }
    let FileFlags::MachO { flags } = file.flags() else {
        return false;
    };
    if !flags.contains(macho::MH_TWOLEVEL) || flags.contains(macho::MH_FORCE_FLAT) {
        return false;
    }
    // Only the resident system library can be a dependency. Exclude all
    // loader commands and sections that can have other startup effects.
    const SYSTEM: &[u8] = b"/usr/lib/libSystem.B.dylib";
    let Ok(mut commands) = file.macho_load_commands() else {
        return false;
    };
    let mut signed = false;
    loop {
        let command = match commands.next() {
            Ok(Some(command)) => command,
            Ok(None) => break,
            Err(_) => return false,
        };
        match command.cmd() {
            macho::LC_LOAD_DYLIB => {
                let Ok(Some(dylib)) = command.dylib() else {
                    return false;
                };
                if command.string(file.endian(), dylib.dylib.name) != Ok(SYSTEM) {
                    return false;
                }
            }
            macho::LC_CODE_SIGNATURE => signed = true,
            macho::LC_SEGMENT_64
            | macho::LC_ID_DYLIB
            | macho::LC_DYLD_CHAINED_FIXUPS
            | macho::LC_DYLD_EXPORTS_TRIE
            | macho::LC_DYLD_INFO_ONLY
            | macho::LC_SYMTAB
            | macho::LC_DYSYMTAB
            | macho::LC_UUID
            | macho::LC_BUILD_VERSION
            | macho::LC_SOURCE_VERSION
            | macho::LC_VERSION_MIN_MACOSX
            | macho::LC_FUNCTION_STARTS
            | macho::LC_DATA_IN_CODE => {}
            _ => return false,
        }
    }
    // Ordinary TLS storage stays inert in the helper. A TLS initialization
    // callback or any other unrecognized section always defers to startup.
    let passive_sections = file.sections().all(|section| {
        matches!(section.name(), Ok(
            "__text" | "__stubs" | "__stub_helper" | "__const" | "__cstring" |
            "__unwind_info" | "__eh_frame" | "__got" | "__la_symbol_ptr" | "__data" |
            "__bss" | "__common" | "__literal4" | "__literal8" | "__literal16" |
            "__thread_vars" | "__thread_data" | "__thread_bss"
        )) && matches!(section.flags(), SectionFlags::MachO { flags, .. } if matches!(flags.typ(),
            macho::S_REGULAR | macho::S_ZEROFILL | macho::S_CSTRING_LITERALS |
            macho::S_4BYTE_LITERALS | macho::S_8BYTE_LITERALS | macho::S_16BYTE_LITERALS |
            macho::S_NON_LAZY_SYMBOL_POINTERS | macho::S_LAZY_SYMBOL_POINTERS | macho::S_SYMBOL_STUBS |
            macho::S_THREAD_LOCAL_REGULAR | macho::S_THREAD_LOCAL_ZEROFILL | macho::S_THREAD_LOCAL_VARIABLES))
    });
    signed
        && passive_sections
        && file.imports().is_ok_and(|mut imports| {
            imports
                .all(|entry| entry.is_ok_and(|entry| entry.library() == SYSTEM && !entry.is_weak()))
        })
        && file.exports().is_ok_and(|mut exports| {
            exports.all(|entry| {
                entry.is_ok_and(|entry| {
                    matches!(
                        entry.target(),
                        ExportTarget::Address { .. } | ExportTarget::TlvDescriptor { .. }
                    ) && !entry.is_weak()
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::{has_passive_image, passive_image};
    use crate::native_clang::TargetClang;
    use rinzu_bindgen::source_package::sha256;

    #[test]
    fn only_passive_exact_images_can_be_loaded_early() {
        let directory =
            std::env::temp_dir().join(format!("rinzu-preload-image-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let _scratch = crate::verify_capture::TemporaryDirectory(directory.clone());
        let clang = TargetClang::resolve().unwrap();
        for (name, text, flags, eligible) in [
            ("plain", "int read_value(void) { return 1; }", &[][..], true),
            (
                "tls",
                "static _Thread_local int n; int read_value(void) { return ++n; }",
                &[][..],
                true,
            ),
            (
                "constructor",
                "static int n; __attribute__((constructor)) static void init(void) { n = 41; } int read_value(void) { return n; }",
                &[][..],
                false,
            ),
            (
                "framework",
                "#include <CoreFoundation/CoreFoundation.h>\nlong read_value(void) { return CFStringGetLength(CFSTR(\"value\")); }",
                &["-framework", "CoreFoundation"][..],
                false,
            ),
        ] {
            let source = directory.join(format!("{name}.c"));
            let image = directory.join(format!("{name}.dylib"));
            std::fs::write(&source, text).unwrap();
            let result = clang
                .shared_command(image.file_name().unwrap().to_str().unwrap())
                .arg(&source)
                .args(flags)
                .arg("-o")
                .arg(&image)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let bytes = std::fs::read(&image).unwrap();
            assert_eq!(passive_image(&bytes), eligible, "{name}");
            assert_eq!(
                has_passive_image(&image, &sha256(&bytes)),
                eligible,
                "{name}"
            );
            assert!(!has_passive_image(&image, &"0".repeat(64)), "{name}");
        }
    }
}
