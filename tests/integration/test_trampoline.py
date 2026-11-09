#!/usr/bin/env python3
"""Reproducible signed AOT target gate; never enables JIT entitlements."""
import pathlib
import platform
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parents[2]
assert platform.system() == "Darwin" and platform.machine() == "arm64"
with tempfile.TemporaryDirectory(prefix="rinzu-trampoline-") as directory:
    binary = pathlib.Path(directory) / "spike"
    def run(*args):
        result = subprocess.run(args, text=True, capture_output=True)
        print(result.stdout + result.stderr, end="")
        assert result.returncode == 0, (args, result.returncode)
        return result
    run("sw_vers")
    run("clang", "--version")
    run("clang", "-Wall", "-Wextra", "-Werror", "-O2",
        str(root / "tests/fixtures/trampoline/spike.c"),
        str(root / "tests/fixtures/trampoline/template.S"), "-o", str(binary))
    run("codesign", "--force", "--sign", "-", "--options", "runtime", str(binary))
    run("codesign", "--verify", "--strict", "--verbose=2", str(binary))
    signature = run("codesign", "-d", "--verbose=4", str(binary))
    assert "(adhoc,runtime)" in signature.stderr
    entitlements = run("codesign", "-d", "--entitlements", "-", str(binary))
    assert not entitlements.stdout.strip(), "entitlements must be absent"
    run(str(binary))
    for mode, status in [("closed", 86), ("resource", 71), ("image", 78), ("permission", 78)]:
        result = subprocess.run([str(binary), mode], text=True, capture_output=True)
        print(f"{mode}: exit={result.returncode} {result.stderr.strip()}")
        assert result.returncode == status
        assert "PASS" not in result.stdout
    print("PASS signed AOT; no entitlements; late gate and failure categories")
