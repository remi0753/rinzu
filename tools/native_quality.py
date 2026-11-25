#!/usr/bin/env python3
"""C/Objective-C quality checks and clangd compilation contexts (stdlib only)."""

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
DATABASE = (
    ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "native-quality"
).resolve()
LLVM_MAJOR = 21
EXTENSIONS = {".c", ".h", ".m", ".cpp", ".mm"}
JIT_UNITS = ("compiled-async.runtime.c", "compiled-async.program.c")
JIT_HEADER = "compiled-async.shared.h"
MARKERS = {
    "SOURCE_NATIVE_RECORD_LAYOUTS": "RINZU_TOOLING_RECORD_LAYOUTS",
    "SOURCE_NATIVE_PLANS": "RINZU_TOOLING_PLANS",
    "SOURCE_NATIVE_STARTUP": "RINZU_TOOLING_STARTUP",
    "JIT_ENTRY_COUNTS": None,
    "CURRENT_ASYNC_HELPERS": "RINZU_TOOLING_POLL_ASYNC_HELPERS",
    "CURRENT_POLL_BODY": "RINZU_TOOLING_POLL_BODY",
}
MACOS_ONLY = (
    "crates/driver/src/aot_supplant.c",
    "crates/driver/tests/fixtures/native_capsule_sdk",
    "crates/driver/tests/fixtures/native_dispatcher_sdk",
    "crates/driver/tests/fixtures/native_foreign_sdk",
    "crates/driver/tests/fixtures/native_performance_sdk",
    "crates/driver/tests/fixtures/native_supplant_sdk",
    "crates/driver/tests/fixtures/native_termination_sdk",
    "crates/driver/tests/fixtures/native_thread_sdk",
    "crates/runtime/src/owner_dispatcher.c",
    "crates/runtime/tests/fixtures/activity_clock.c",
    "crates/runtime/tests/fixtures/owner_dispatcher.c",
    "sdk/io/native/src/file_metadata_macos.h",
    "sdk/io/tests/whole_file.c",
    "sdk/socket/native/src/readiness_macos.h",
    "sdk/socket/bench/observer.c",
    "sdk/socket/tests/boundaries.c",
    "sdk/socket/tests/lookup.c",
    "sdk/socket/tests/wake-coalescing.c",
    "tests/fixtures/programs/source-callback",
    "tests/fixtures/trampoline",
)
LINUX_ONLY = (
    "crates/runtime/src/platform/linux",
    "include/rinzu/arch/x86_64",
    "include/rinzu/elf_image.h",
    "sdk/io/native/src/file_metadata_linux.h",
    "sdk/socket/native/src/readiness_linux.h",
)


def run(arguments, **options):
    return subprocess.run(arguments, cwd=ROOT, check=True, **options)


def tool(name):
    override = os.environ.get("RINZU_" + name.upper().replace("-", "_"))
    candidates = (
        [override]
        if override
        else [
            f"/opt/homebrew/opt/llvm@{LLVM_MAJOR}/bin/{name}",
            f"/usr/local/opt/llvm@{LLVM_MAJOR}/bin/{name}",
            shutil.which(f"{name}-{LLVM_MAJOR}"),
            shutil.which(name),
        ]
    )
    failures = []
    for candidate in filter(None, candidates):
        if not Path(candidate).is_file():
            continue
        result = subprocess.run(
            [candidate, "--version"], capture_output=True, text=True
        )
        version = result.stdout + result.stderr
        if result.returncode == 0 and re.search(rf"version {LLVM_MAJOR}\.", version):
            return str(Path(candidate).resolve())
        failures.append(version.strip())
    detail = "\n".join(failures)
    raise RuntimeError(
        f"{name} {LLVM_MAJOR} is required (macOS: brew install llvm@{LLVM_MAJOR}). "
        f"Set RINZU_{name.upper().replace('-', '_')} to its executable.\n{detail}"
    )


def native_files():
    result = run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
    )
    return sorted(
        {
            ROOT / os.fsdecode(path)
            for path in result.stdout.split(b"\0")
            if path
            and Path(os.fsdecode(path)).suffix in EXTENSIONS
            and not Path(os.fsdecode(path)).is_relative_to("include/rinzu/vendor")
            and (ROOT / os.fsdecode(path)).is_file()
        }
    )


def compilation_files(target):
    macos = target == "aarch64-apple-darwin"
    excluded = LINUX_ONLY if macos else MACOS_ONLY
    return [
        file
        for file in native_files()
        if (macos or file.suffix not in {".m", ".mm"})
        and not any(file.is_relative_to(ROOT / path) for path in excluded)
    ]


def included_header_contexts(target):
    """Private SDK fragments are parsed after their real include predecessors."""
    platform = "macos" if target == "aarch64-apple-darwin" else "linux"
    io = ROOT / "sdk/io/native/src"
    socket = ROOT / "sdk/socket/native/src"
    metadata = io / f"file_metadata_{platform}.h"
    readiness = socket / f"readiness_{platform}.h"
    return {
        io / "file_metadata.h": (io / "whole_file.c", metadata, io / "file_metadata.h"),
        metadata: (io / "whole_file.c", metadata),
        readiness: (socket / "socket.c", readiness),
    }


def prefix_before_include(file, included):
    source = file.read_text()
    match = re.search(
        r'^[ \t]*#include "' + re.escape(included.name) + r'"[ \t]*$',
        source,
        flags=re.MULTILINE,
    )
    if not match:
        raise RuntimeError(f"no include of {included} in {file}")
    prefix = source[: match.start()]
    depth = 0
    for directive in re.findall(
        r"^[ \t]*#[ \t]*(if|ifdef|ifndef|endif)\b", prefix, flags=re.MULTILINE
    ):
        depth += -1 if directive == "endif" else 1
    return prefix + "\n" + "#endif\n" * depth


def selected_files(paths, files=None):
    if files is None:
        files = native_files()
    if not paths:
        return files
    selected = set()
    for argument in paths:
        path = Path(argument).resolve()
        matching = [file for file in files if file == path or path in file.parents]
        if not matching:
            raise RuntimeError(
                f"no repository C/header/Objective-C files at {argument}"
            )
        selected.update(matching)
    return sorted(selected)


def format_files(paths, write=False):
    files = selected_files(paths)
    run(
        [
            tool("clang-format"),
            "--style=file",
            "--fallback-style=none",
            *(["-i"] if write else ["--dry-run", "--Werror"]),
            *map(str, files),
        ]
    )
    print(
        f"C format {'written' if write else 'passed'}: {len(files)} files", flush=True
    )


def fragment_match(template, source):
    """Locate a real generated fragment, retaining the generator's substitutions."""
    parts = re.split(r"/\* (" + "|".join(MARKERS) + r") \*/", template)
    expression = re.escape(parts[0])
    for marker, text in zip(parts[1::2], parts[2::2]):
        expression += rf"(?P<{marker}>.*?)" + re.escape(text)
    return re.search(expression, source, flags=re.DOTALL)


def entry(file, flags, compiler="clang"):
    return {
        "directory": str(ROOT),
        "file": str(file),
        "arguments": [compiler, *flags, "-c", str(file)],
    }


def source_location(source, offset, unit):
    file, line = str(unit), 1
    for text in source[:offset].splitlines(keepends=True):
        directive = re.fullmatch(r'#line (\d+) "([^"]+)"\n?', text)
        if directive:
            line, file = int(directive[1]), directive[2]
        else:
            line += text.count("\n")
    return file, line


def compilation_database(driver=None):
    DATABASE.mkdir(parents=True, exist_ok=True)
    command = (
        [str(driver.resolve())]
        if driver
        else ["cargo", "run", "--quiet", "--package", "rinzu-driver", "--"]
    )
    run([*command, "native-quality-sources", str(DATABASE)])
    toolchain = json.loads((DATABASE / "toolchain.json").read_text())
    compiler = toolchain["compiler"]
    target = toolchain["target"]
    flags = [
        *toolchain["arguments"],
        "-std=c11",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-Wno-unused-function",
        "-pthread",
        "-I",
        str(ROOT / "include"),
    ]
    sdl_arguments = toolchain["sdl2"]
    sdl_flags = []
    for option, value in zip(sdl_arguments[::2], sdl_arguments[1::2]):
        if option in {"-I", "-D", "-F"}:
            sdl_flags.extend([option, value])
    guard_names = json.loads((DATABASE / "fixture-sources.json").read_text())
    sources = {
        DATABASE / name: (DATABASE / name).read_text()
        for name in [
            "source-platform.runtime.c",
            "compiled-async.c",
            "async-wait.c",
            "typed-worker.c",
            "json.runtime.c",
            *JIT_UNITS,
            JIT_HEADER,
            *guard_names,
        ]
    }
    # The ready-snapshot test also accepts unreachable labels left by lowering.
    generated_flags = {"ready_snapshot_guards.c": ["-Wno-unused-label"]}
    files = compilation_files(target)
    included_headers = included_header_contexts(target)
    fragments = [file for file in files if file.parent == ROOT / "crates/driver/src"]
    fragments += [
        ROOT / "crates/driver/tests/fixtures" / name
        for name in ["async_wait_cycle.c", "typed_worker_resources.c", *guard_names]
    ]
    commands, units, contexts = [], [], {}
    # clangd parses the original fragment after a generated prefix. Conditional
    # includes supply the same schema/plan substitutions as the real generator.
    # clang-tidy analyzes the complete generated TU, including its static callees.
    for file in fragments:
        template = file.read_text()
        candidates = sources.items()
        if file.name in {"compiled_async_shared.h", "compiled_async_program.h"}:
            candidates = [(DATABASE / JIT_HEADER, sources[DATABASE / JIT_HEADER])]
        for unit, source in candidates:
            match = fragment_match(template, source)
            if match:
                break
        else:
            raise RuntimeError(
                f"no generated compile context for {file.relative_to(ROOT)}"
            )
        directory = DATABASE / "contexts" / file.relative_to(ROOT)
        directory.mkdir(parents=True, exist_ok=True)
        prefix = directory / "prefix.h"
        # A prefix extracted from the generated header opens its outer guard.
        # Close that guard while retaining its declarations for clangd.
        prefix.write_text(
            source[: match.start()] + ("\n#endif\n" if unit.name == JIT_HEADER else "")
        )
        # Later fragments consume these globals. The complete TU still checks
        # unused variables; the editor's prefix-only view cannot do that.
        extra = [
            "-include",
            str(prefix),
            "-Wno-undefined-internal",
            "-Wno-unused-variable",
        ]
        for marker, replacement in match.groupdict().items():
            macro = MARKERS[marker]
            if macro:
                header = directory / f"{marker}.h"
                header.write_text(replacement)
                extra.append(f'-D{macro}="{header}"')
            else:
                extra += [
                    f"-D{name}={value}"
                    for name, value in re.findall(
                        r"#define (RINZU_JIT_\w+) (\d+)", replacement
                    )
                ]
        commands.append(
            entry(file, flags + generated_flags.get(unit.name, []) + extra, compiler)
        )
        if unit.name == JIT_HEADER:
            commands.append(
                entry(
                    file, flags + extra + ["-DRINZU_RUNTIME_IMPLEMENTATION=1"], compiler
                )
            )
            contexts[str(file)] = [str(DATABASE / name) for name in JIT_UNITS]
        else:
            contexts[str(file)] = [str(unit)]

    for file in files:
        if file in fragments or file in included_headers:
            continue
        extra = ["-I", str(file.parent)]
        if file.suffix == ".cpp" and (
            '"llvm/' in file.read_text() or file.name == "closed.cpp"
        ):
            llvm_config = Path(tool("clang")).parent / "llvm-config"
            llvm_include = run(
                [str(llvm_config), "--includedir"], capture_output=True, text=True
            ).stdout.strip()
            extra += ["-isystem", llvm_include]
        # Source SDK compilation contexts follow manifest include directories.
        # Test translation units inherit their enclosing SDK's context too.
        for parent in file.parents:
            manifest = parent / "rinzu.toml"
            if manifest.is_file():
                inputs = tomllib.loads(manifest.read_text()).get("native", {})
                platform = "macos" if target == "aarch64-apple-darwin" else "linux"
                platform_inputs = inputs.get("platform", {}).get(platform, {})
                for directory in inputs.get("include-dirs", []) + platform_inputs.get(
                    "include-dirs", []
                ):
                    extra += ["-I", str(parent / directory)]
                break
        if "sdl2" in file.parts:
            extra += sdl_flags + ["-I", str(ROOT / "sdk/sdl2/native/include")]
        if file.suffix in {".m", ".mm"}:
            extra += ["-fobjc-arc"]
        if file.suffix == ".h":
            extra += ["-x", "c-header"]
        if file.name in {
            "owner_dispatcher.c",
            "foreign_queue.c",
            "native_diagnostics.c",
        }:
            extra += ["-DRINZU_DISPATCHER_TESTING"]
        if file.name in {"callback_entry.c", "callback_frame.c", "foreign_callback.c"}:
            extra += ["-DRINZU_CALLBACK_TESTING"]
        if file.name == "native_diagnostics.c":
            extra += ["-DRINZU_ACTIVITY_TESTING"]
        if file.name in {"storage.c", "storage.h"}:
            extra += [
                "-include",
                str(ROOT / "crates/driver/tests/fixtures/native_record_sdk/api.h"),
            ]
            if file.suffix == ".c":
                extra += [
                    "-include",
                    str(ROOT / "crates/driver/tests/fixtures/native_record_sdk/api.c"),
                ]
        if file.name == "native_abi_aot.c":
            extra += ["-include", str(file.parent / "native_abi.c")]
        if file.relative_to(ROOT).as_posix() == "sdk/socket/tests/accounting-check.h":
            extra += ["-include", str(ROOT / "sdk/socket/native/src/socket.c")]
        if (
            file.relative_to(ROOT).as_posix()
            == "include/rinzu/arch/x86_64/continuation.h"
        ):
            extra += ["-include", str(ROOT / "include/rinzu/continuation.h")]
        language_flags = (
            [flag for flag in flags if flag != "-std=c11"] + ["-std=c++17"]
            if file.suffix in {".cpp", ".mm"}
            else flags
        )
        commands.append(entry(file, language_flags + extra, compiler))
        units.append(str(file))

    by_file = {command["file"]: command for command in commands}
    for file, chain in included_headers.items():
        if file not in files:
            continue
        directory = DATABASE / "contexts" / file.relative_to(ROOT)
        directory.mkdir(parents=True, exist_ok=True)
        prefix = directory / "prefix.h"
        prefix.write_text(
            "\n".join(
                prefix_before_include(parent, child)
                for parent, child in zip(chain, chain[1:])
            )
        )
        consumer = chain[0]
        consumer_flags = by_file[str(consumer)]["arguments"][1:-2]
        commands.append(
            entry(
                file,
                consumer_flags
                + [
                    "-x",
                    "c-header",
                    "-include",
                    str(prefix),
                    "-Wno-undefined-internal",
                    "-Wno-unused-variable",
                ],
                compiler,
            )
        )
        contexts[str(file)] = [str(consumer)]

    # Map generated lines back to the source templates so CLI diagnostics name
    # editable files. Keep substituted code in the TU rather than mocking it.
    for unit, source in sources.items():
        if unit.name in {*JIT_UNITS, JIT_HEADER}:
            source = (DATABASE / f"mapped-{unit.name}").read_text()
        for file in fragments:
            template = file.read_text()
            # Some templates occur inside another template's substituted marker.
            # Re-match the current text to retain both mappings and valid offsets.
            match = fragment_match(template, source)
            if not match:
                continue
            parts = re.split(r"/\* (" + "|".join(MARKERS) + r") \*/", template)
            mapped = f'#line 1 "{file}"\n{parts[0]}'
            line = 1 + parts[0].count("\n")
            for marker, text in zip(parts[1::2], parts[2::2]):
                generated_file, generated_line = source_location(
                    source, match.start(marker), unit
                )
                mapped += (
                    f'\n#line {generated_line} "{generated_file}"\n{match.group(marker)}'
                    f'\n#line {line} "{file}"\n{text}'
                )
                line += text.count("\n")
            generated_file, generated_line = source_location(source, match.end(), unit)
            mapped += f'\n#line {generated_line} "{generated_file}"\n'
            source = source[: match.start()] + mapped + source[match.end() :]
        output = DATABASE / f"lint-{unit.name}"
        if unit.name in JIT_UNITS:
            source = source.replace(
                f'#include "{JIT_HEADER}"', f'#include "lint-{JIT_HEADER}"'
            )
        output.write_text(source)
        if unit.name == JIT_HEADER:
            continue
        # Split templates retain #line origins even after linkage/declarations
        # change. A scoped lint must include every actual consumer, while
        # clangd can keep its one complete prefix for the original fragment.
        origins = set(re.findall(r'^#line \d+ "([^"]+)"$', source, flags=re.MULTILINE))
        if unit.name in JIT_UNITS:
            origins.update(
                re.findall(
                    r'^#line \d+ "([^"]+)"$',
                    (DATABASE / f"mapped-{JIT_HEADER}").read_text(),
                    flags=re.MULTILINE,
                )
            )
        for file in origins & contexts.keys():
            if str(unit) not in contexts[file]:
                contexts[file].append(str(unit))
        commands.append(
            entry(
                output,
                [flag for flag in flags if flag != "-Wno-unused-function"]
                + generated_flags.get(unit.name, []),
                compiler,
            )
        )
        units.append(str(output))
        contexts = {
            file: [
                str(output) if context == str(unit) else context
                for context in consumers
            ]
            for file, consumers in contexts.items()
        }
    (DATABASE / "compile_commands.json").write_text(
        json.dumps(commands, indent=2) + "\n"
    )
    (DATABASE / "units.json").write_text(
        json.dumps(
            {
                "target": target,
                "units": units,
                "contexts": contexts,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"C compilation database: {len(commands)} entries in {DATABASE}", flush=True)
    return units, contexts, commands


def remap_diagnostics(output, unit):
    """clang-tidy reports physical lines; honor the generated #line mappings."""
    locations = {}
    file, line = str(unit), 1
    for physical, text in enumerate(Path(unit).read_text().splitlines(), 1):
        directive = re.fullmatch(r'#line (\d+) "([^"]+)"', text)
        if directive:
            line, file = int(directive[1]), directive[2]
        else:
            locations[physical] = (file, line)
            line += 1

    def replace(match):
        file, line = locations.get(int(match[1]), (str(unit), int(match[1])))
        return f"{file}:{line}:"

    return re.sub(re.escape(str(unit)) + r":(\d+):", replace, output)


def lint_units(selected, units, contexts, commands):
    chosen = {
        context for file in selected for context in contexts.get(str(file), [str(file)])
    }
    headers = {file.resolve() for file in selected if file.suffix == ".h"}
    if headers:
        # Use the same include paths and defines as Clang's real translation
        # units. A scoped header check also analyzes its transitive consumers.
        candidates = set(units) - chosen
        for entry in commands:
            unit = entry["file"]
            if unit not in candidates:
                continue
            arguments = entry["arguments"]
            dependencies = subprocess.run(
                [*arguments[:-2], "-MM", "-MG", "-MT", "rinzu-dependencies", unit],
                cwd=entry["directory"],
                capture_output=True,
                text=True,
            )
            _, separator, inputs = dependencies.stdout.partition(":")
            if dependencies.returncode or not separator:
                chosen.add(unit)
                continue
            files = shlex.split(inputs.replace("\\\n", "").replace("$$", "$"))
            if any(
                (Path(entry["directory"]) / file).resolve() in headers for file in files
            ):
                chosen.add(unit)
    return sorted(chosen)


def lint_files(paths, driver=None):
    tidy = tool("clang-tidy")
    config = f"--config-file={ROOT / '.clang-tidy'}"
    units, contexts, commands = compilation_database(driver)
    if paths:
        target = json.loads((DATABASE / "toolchain.json").read_text())["target"]
        selected = selected_files(paths, compilation_files(target))
        units = lint_units(selected, units, contexts, commands)
    for unit in units:
        print(f"C lint: {os.path.relpath(unit, ROOT)}", flush=True)
        # Analyze header paths with the definitions and callers in their real
        # consumers. Standalone headers get compiler and AST-based tidy checks.
        checks = ["--checks=-clang-analyzer-*"] if Path(unit).suffix == ".h" else []
        result = subprocess.run(
            [tidy, "--quiet", config, *checks, "-p", str(DATABASE), unit],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        print(remap_diagnostics(result.stdout, unit), end="", flush=True)
        print(
            remap_diagnostics(result.stderr, unit), end="", file=sys.stderr, flush=True
        )
        result.check_returncode()
    print(f"C lint passed: {len(units)} translation units", flush=True)


def main():
    # Forward LSP flags verbatim; argparse must not consume clangd's options.
    if len(sys.argv) > 1 and sys.argv[1] == "clangd":
        executable = tool("clangd")
        os.execv(executable, [executable, *sys.argv[2:]])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command", choices=["fmt", "fmt-write", "lint", "db", "versions"]
    )
    parser.add_argument(
        "--rinzu", type=Path, help="use a prebuilt CLI for lint/db source generation"
    )
    parser.add_argument("paths", nargs="*")
    arguments = parser.parse_args()
    if arguments.command.startswith("fmt"):
        format_files(arguments.paths, arguments.command == "fmt-write")
    elif arguments.command == "lint":
        lint_files(arguments.paths, arguments.rinzu)
    elif arguments.command == "db":
        compilation_database(arguments.rinzu)
    else:
        for name in ["clang-format", "clang-tidy", "clangd"]:
            run([tool(name), "--version"])


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(
            f"native quality: {Path(error.cmd[0]).name} exited with status {error.returncode}",
            file=sys.stderr,
        )
        sys.exit(1)
    except (RuntimeError, OSError) as error:
        print(f"native quality: {error}", file=sys.stderr)
        sys.exit(1)
