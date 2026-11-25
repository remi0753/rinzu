"""Verify that the configured tools reject bad C before accepting a clean tree."""

import json
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import native_quality as quality


class NativeQualityTests(unittest.TestCase):
    def test_platform_contexts_keep_linux_architecture_headers_on_linux(self):
        header = quality.ROOT / "include/rinzu/arch/x86_64/continuation.h"
        self.assertIn(header, quality.native_files())
        self.assertNotIn(header, quality.compilation_files("aarch64-apple-darwin"))
        self.assertIn(header, quality.compilation_files("x86_64-unknown-linux-gnu"))
        toolchain = json.loads((quality.DATABASE / "toolchain.json").read_text())
        if toolchain["target"] == "x86_64-unknown-linux-gnu":
            commands = json.loads(
                (quality.DATABASE / "compile_commands.json").read_text()
            )
            command = next(entry for entry in commands if entry["file"] == str(header))
            self.assertIn(
                str(quality.ROOT / "include/rinzu/continuation.h"), command["arguments"]
            )

    def test_formatter_rejects_and_repairs_style_violations(self):
        formatter = quality.tool("clang-format")
        flags = [
            formatter,
            "--style=file",
            "--assume-filename=" + str(quality.ROOT / "probe.c"),
        ]
        invalid = "int probe( void){return 1;}\n"
        result = subprocess.run(
            flags + ["--dry-run", "--Werror"],
            input=invalid,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0, result.stderr)
        formatted = subprocess.run(
            flags, input=invalid, capture_output=True, text=True, check=True
        ).stdout
        self.assertNotEqual(formatted, invalid)
        result = subprocess.run(
            flags + ["--dry-run", "--Werror"],
            input=formatted,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def tidy(self, source):
        toolchain = json.loads((quality.DATABASE / "toolchain.json").read_text())
        with tempfile.TemporaryDirectory(dir=quality.DATABASE) as directory:
            file = Path(directory) / "probe.c"
            file.write_text(source)
            result = subprocess.run(
                [
                    quality.tool("clang-tidy"),
                    "--quiet",
                    "--config-file=" + str(quality.ROOT / ".clang-tidy"),
                    str(file),
                    "--",
                    *toolchain["arguments"],
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-I",
                    str(quality.ROOT / "include"),
                ],
                capture_output=True,
                text=True,
            )
            result.stdout = quality.remap_diagnostics(result.stdout, file)
            result.stderr = quality.remap_diagnostics(result.stderr, file)
            return result

    def test_static_analyzer_rejects_division_by_zero(self):
        result = self.tidy(
            "int probe(int value) { int zero = 0; return value / zero; }\n"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("clang-analyzer-core.DivideZero", result.stdout + result.stderr)

    def test_compiler_diagnostics_are_errors(self):
        result = self.tidy("int probe(void) { return undeclared_value; }\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("undeclared_value", result.stdout + result.stderr)

    def test_generated_diagnostic_points_to_original_fragment(self):
        file = quality.ROOT / "probe_fragment.c"
        result = self.tidy(
            f'#line 123 "{file}"\n'
            "int probe(int value) { int zero = 0; return value / zero; }\n"
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"{file}:123:", result.stdout + result.stderr)
        self.assertIn("clang-analyzer-core.DivideZero", result.stdout + result.stderr)

    def test_clean_c_passes(self):
        result = self.tidy("int probe(void) { return 7; }\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_symbolic_scalar_multiplication_is_analyzable(self):
        result = self.tidy(
            '#include "rinzu/scalar.h"\n'
            "rinzu_scalar_result_t probe(unsigned kind, uint64_t left, uint64_t right) {\n"
            "  return rinzu_scalar_arithmetic(kind, 3, left, right);\n}\n"
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_database_covers_all_native_files_and_full_fragment_units(self):
        commands = json.loads((quality.DATABASE / "compile_commands.json").read_text())
        by_file = {entry["file"]: entry for entry in commands}
        manifest = json.loads((quality.DATABASE / "units.json").read_text())
        self.assertTrue(
            {str(file) for file in quality.compilation_files(manifest["target"])}
            <= by_file.keys()
        )
        self.assertTrue(
            {unit for consumers in manifest["contexts"].values() for unit in consumers}
            <= set(manifest["units"])
        )
        for file, consumers in manifest["contexts"].items():
            for context in consumers:
                text = Path(context).read_text()
                if Path(context).name in {f"lint-{name}" for name in quality.JIT_UNITS}:
                    text += (
                        quality.DATABASE / f"lint-{quality.JIT_HEADER}"
                    ).read_text()
                if str(file) not in text:
                    entry = by_file[context]
                    result = subprocess.run(
                        [
                            *entry["arguments"][:-2],
                            "-MM",
                            "-MG",
                            "-MT",
                            "dependencies",
                            context,
                        ],
                        cwd=entry["directory"],
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    inputs = (
                        result.stdout.partition(":")[2]
                        .replace("\\\n", "")
                        .replace("$$", "$")
                    )
                    dependencies = {
                        (Path(entry["directory"]) / name).resolve()
                        for name in shlex.split(inputs)
                    }
                    self.assertIn(Path(file).resolve(), dependencies)
        # Exercise both a generated prefix and substitutions inside a fixture.
        for name in ("list_update_guards.c", "poll_state_matrix.c"):
            file = str(quality.ROOT / "crates/driver/tests/fixtures" / name)
            self.assertIn(file, manifest["contexts"])
            entry = by_file[file]
            result = subprocess.run(
                [*entry["arguments"][:-2], "-fsyntax-only", file],
                cwd=entry["directory"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_split_jit_units_and_both_header_modes_have_real_compile_contexts(self):
        commands = json.loads((quality.DATABASE / "compile_commands.json").read_text())
        manifest = json.loads((quality.DATABASE / "units.json").read_text())
        units = {str(quality.DATABASE / f"lint-{name}") for name in quality.JIT_UNITS}
        self.assertTrue(units <= set(manifest["units"]))
        prelude = str(quality.ROOT / "crates/driver/src/compiled_async.c")
        self.assertTrue(units <= set(manifest["contexts"][prelude]))
        self.assertTrue(
            units
            <= set(
                quality.lint_units(
                    {Path(prelude)}, manifest["units"], manifest["contexts"], commands
                )
            )
        )
        entry = str(quality.ROOT / "crates/driver/src/compiled_async_entry.c")
        self.assertIn(
            str(quality.DATABASE / "lint-compiled-async.runtime.c"),
            manifest["contexts"][entry],
        )
        for name in ("compiled_async_shared.h", "compiled_async_program.h"):
            file = str(quality.ROOT / "crates/driver/src" / name)
            self.assertEqual(set(manifest["contexts"][file]), units)
            entries = [entry for entry in commands if entry["file"] == file]
            self.assertEqual(len(entries), 2)
            self.assertEqual(
                sum(
                    "-DRINZU_RUNTIME_IMPLEMENTATION=1" in entry["arguments"]
                    for entry in entries
                ),
                1,
            )
            self.assertTrue(
                units
                <= set(
                    quality.lint_units(
                        {Path(file)}, manifest["units"], manifest["contexts"], commands
                    )
                )
            )
        selected = units | {
            str(quality.ROOT / "crates/driver/src" / name)
            for name in ("compiled_async_shared.h", "compiled_async_program.h")
        }
        for entry in commands:
            if entry["file"] in selected:
                result = subprocess.run(
                    [*entry["arguments"][:-2], "-fsyntax-only", entry["file"]],
                    cwd=entry["directory"],
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                name = Path(entry["file"]).name
                if name in {f"lint-{name}" for name in quality.JIT_UNITS}:
                    plain = quality.DATABASE / name.removeprefix("lint-")
                    result = subprocess.run(
                        [*entry["arguments"][:-2], "-fsyntax-only", str(plain)],
                        cwd=entry["directory"],
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
        runtime = (quality.DATABASE / "lint-compiled-async.runtime.c").read_text()
        program = (quality.DATABASE / "lint-compiled-async.program.c").read_text()
        self.assertIn("#define RINZU_RUNTIME_IMPLEMENTATION 1", runtime)
        self.assertNotIn("#define RINZU_RUNTIME_IMPLEMENTATION 1", program)
        header = quality.DATABASE / f"lint-{quality.JIT_HEADER}"
        text = header.read_text()
        original = quality.ROOT / "crates/driver/src/compiled_async_program.h"
        declaration = "RINZU_PROGRAM_API void rinzu_jit_publish(void);"
        self.assertEqual(
            quality.source_location(text, text.index(declaration), header),
            (
                str(original),
                1
                + original.read_text()[: original.read_text().index(declaration)].count(
                    "\n"
                ),
            ),
        )
        entry = quality.ROOT / "crates/driver/src/compiled_async_entry.c"
        statement = "  rinzu_jit_used = 1;"
        self.assertEqual(
            quality.source_location(
                runtime,
                runtime.index(statement),
                quality.DATABASE / "lint-compiled-async.runtime.c",
            ),
            (
                str(entry),
                1 + entry.read_text()[: entry.read_text().index(statement)].count("\n"),
            ),
        )

    def test_private_sdk_headers_use_their_real_include_contexts(self):
        manifest = json.loads((quality.DATABASE / "units.json").read_text())
        commands = json.loads((quality.DATABASE / "compile_commands.json").read_text())
        by_file = {entry["file"]: entry for entry in commands}
        for file, chain in quality.included_header_contexts(manifest["target"]).items():
            consumer = str(chain[0])
            self.assertEqual(manifest["contexts"][str(file)], [consumer])
            self.assertNotIn(str(file), manifest["units"])
            self.assertIn(consumer, manifest["units"])
            self.assertIn(
                consumer,
                quality.lint_units(
                    {file}, manifest["units"], manifest["contexts"], commands
                ),
            )
            entry = by_file[str(file)]
            result = subprocess.run(
                [*entry["arguments"][:-2], "-fsyntax-only", str(file)],
                cwd=entry["directory"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_scoped_header_lint_checks_transitive_consumers_only(self):
        with tempfile.TemporaryDirectory(dir=quality.DATABASE) as directory:
            path = Path(directory)
            header = path / "selected header.h"
            header.write_text("int selected(void);\n")
            (path / "middle.h").write_text('#include "selected header.h"\n')
            consumer = path / "consumer.c"
            consumer.write_text('#include "middle.h"\n')
            other = path / "other.c"
            other.write_text("int other(void);\n")
            units = [str(consumer), str(other)]
            commands = [quality.entry(unit, []) for unit in units]
            self.assertEqual(
                quality.lint_units({header}, units, {}, commands),
                sorted([str(header), str(consumer)]),
            )


if __name__ == "__main__":
    unittest.main()
