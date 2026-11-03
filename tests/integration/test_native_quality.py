"""Verify that the configured tools reject bad C before accepting a clean tree."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import native_quality as quality


class NativeQualityTests(unittest.TestCase):
    def test_formatter_rejects_and_repairs_style_violations(self):
        formatter = quality.tool("clang-format")
        flags = [formatter, "--style=file", "--assume-filename=" + str(quality.ROOT / "probe.c")]
        invalid = "int probe( void){return 1;}\n"
        result = subprocess.run(flags + ["--dry-run", "--Werror"], input=invalid,
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0, result.stderr)
        formatted = subprocess.run(flags, input=invalid, capture_output=True, text=True, check=True).stdout
        self.assertNotEqual(formatted, invalid)
        result = subprocess.run(flags + ["--dry-run", "--Werror"], input=formatted,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def tidy(self, source):
        with tempfile.TemporaryDirectory(dir=quality.DATABASE) as directory:
            file = Path(directory) / "probe.c"
            file.write_text(source)
            result = subprocess.run([quality.tool("clang-tidy"), "--quiet",
                                     "--config-file=" + str(quality.ROOT / ".clang-tidy"),
                                     str(file), "--", "-std=c11", "-Wall", "-Wextra", "-Werror",
                                     "-I", str(quality.ROOT / "include")],
                                    capture_output=True, text=True)
            result.stdout = quality.remap_diagnostics(result.stdout, file)
            result.stderr = quality.remap_diagnostics(result.stderr, file)
            return result

    def test_static_analyzer_rejects_division_by_zero(self):
        result = self.tidy("int probe(int value) { int zero = 0; return value / zero; }\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("clang-analyzer-core.DivideZero", result.stdout + result.stderr)

    def test_compiler_diagnostics_are_errors(self):
        result = self.tidy("int probe(void) { return undeclared_value; }\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("undeclared_value", result.stdout + result.stderr)

    def test_generated_diagnostic_points_to_original_fragment(self):
        file = quality.ROOT / "probe_fragment.c"
        result = self.tidy(f'#line 123 "{file}"\n'
                           'int probe(int value) { int zero = 0; return value / zero; }\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"{file}:123:", result.stdout + result.stderr)
        self.assertIn("clang-analyzer-core.DivideZero", result.stdout + result.stderr)

    def test_clean_c_passes(self):
        result = self.tidy("int probe(void) { return 7; }\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_symbolic_scalar_multiplication_is_analyzable(self):
        result = self.tidy(
            '#include "rinzu/scalar.h"\n'
            'rinzu_scalar_result_t probe(unsigned kind, uint64_t left, uint64_t right) {\n'
            '  return rinzu_scalar_arithmetic(kind, 3, left, right);\n}\n')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_database_covers_all_native_files_and_full_fragment_units(self):
        commands = json.loads((quality.DATABASE / "compile_commands.json").read_text())
        files = {entry["file"] for entry in commands}
        self.assertTrue({str(file) for file in quality.native_files()} <= files)
        manifest = json.loads((quality.DATABASE / "units.json").read_text())
        self.assertTrue(set(manifest["contexts"].values()) <= set(manifest["units"]))
        for file, context in manifest["contexts"].items():
            self.assertIn(str(file), Path(context).read_text())

if __name__ == "__main__":
    unittest.main()
