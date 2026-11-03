#!/usr/bin/env python3
"""Real AppKit sheet receipts, cancellation and Source JIT/AOT gates."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time
from sdk_return_probe import delayed_native_return

ROOT = Path(__file__).resolve().parents[2]
HOOK = (ROOT / "sdk/macos/tests/sheets/hooks.rinzu.in").read_text()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rinzu", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native-only", action="store_true")
    parser.add_argument("--source-only", action="store_true")
    parser.add_argument("--sanitize", action="store_true")
    parser.add_argument("--tsan", action="store_true")
    parser.add_argument("--mode", default="all", choices=("all", "results", "cancel", "stop", "scope"))
    parser.add_argument("--keep", type=Path)
    parser.add_argument("--return-cancel", action="store_true", help="also cancel after native panel admission but before Source receives its result")
    parser.add_argument("--cancel-repeats", type=int, default=0, help="repeat AOT idle cancellation with a 10 second deadline")
    args = parser.parse_args()
    if args.native_only and args.source_only:
        parser.error("choose native-only or source-only")
    if not 0 <= args.cancel_repeats <= 1000:
        parser.error("cancel-repeats must be 0..1000")
    evidence = {"schema": "rinzu.macos-sheets-v1", "platform": platform.platform(),
                "baseline": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "cliSha256": hashlib.sha256(args.rinzu.read_bytes()).hexdigest(),
                "passed": False, "inputs": {}, "cases": []}
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def save():
        args.output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n")

    try:
        with tempfile.TemporaryDirectory(prefix="rinzu-sheets-") as directory:
            scratch = Path(directory)
            sdk = scratch / "sdk/macos"
            shutil.copytree(ROOT / "sdk/macos", sdk, ignore=shutil.ignore_patterns("build", ".rinzu"))
            shutil.copytree(ROOT / "include", scratch / "include")
            cli = args.rinzu.resolve()
            for path in [*sdk.rglob("*"), Path(__file__)]:
                if path.is_file() and (path.suffix in (".rinzu", ".toml", ".m", ".h", ".py") or path.name.endswith(".rinzu.in")):
                    name = str(path.relative_to(scratch)) if path.is_relative_to(scratch) else "tools/" + path.name
                    evidence["inputs"][name] = hashlib.sha256(path.read_bytes()).hexdigest()
            env = dict(os.environ, RINZU_CACHE_DIR=str(scratch / "cache"), ASAN_OPTIONS="detect_leaks=0",
                       UBSAN_OPTIONS="halt_on_error=1", TSAN_OPTIONS="halt_on_error=1")
            env.pop("RINZU_TOOLCHAIN_ROOT", None)

            def run(name, command, expected=0, extra=None, markers=(), timeout=600):
                row = {"name": name, "command": list(map(str, command))}
                evidence["cases"].append(row)
                save()
                started = time.monotonic()
                try:
                    result = subprocess.run(command, cwd=scratch, env=dict(env, **(extra or {})),
                                            capture_output=True, text=True, timeout=timeout)
                    row.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                except subprocess.TimeoutExpired as problem:
                    row.update(timeout=True, stdout=(problem.stdout or b"").decode(errors="replace"),
                               stderr=(problem.stderr or b"").decode(errors="replace"))
                    raise
                finally:
                    row["seconds"] = time.monotonic() - started
                    save()
                assert result.returncode == expected, row
                if expected == 1:
                    assert "RINZU_FAILURE=cancelled" in result.stderr, row
                assert all(marker in result.stdout for marker in markers), row
                assert not any(item in result.stderr for item in
                               ("ERROR: AddressSanitizer", "runtime error:", "WARNING: ThreadSanitizer")), row
                report = (extra or {}).get("RINZU_RESOURCE_REPORT")
                if report:
                    row["resourcesEnd"] = json.loads(Path(report).read_text().splitlines()[-1])
                    for category in row["resourcesEnd"]["categories"]:
                        if category["id"] in ("managed", "task", "future", "pending_operation", "transport", "native_calls"):
                            assert category["live"] == 0, (name, category)
                        if category["id"] in ("callback_control", "callback_entries"):
                            assert category["allocated"] == 0, (name, category)
                row["passed"] = True
                save()
                print(name + ": passed", flush=True)
                return row

            if not args.source_only:
                flags = ["-std=c11", "-fobjc-arc", "-Wall", "-Wextra", "-Werror", "-I", scratch / "include",
                         "-I", sdk / "native/include", sdk / "tests/sheets.m", "-framework", "AppKit",
                         "-framework", "Foundation", "-framework", "Metal", "-framework", "QuartzCore", "-framework", "ImageIO", "-framework", "CoreGraphics", "-framework", "CoreText"]
                variants = [("native", [])]
                if args.sanitize:
                    variants += [("asan", ["-O1", "-g", "-fsanitize=address,undefined"])]
                if args.tsan:
                    variants += [("tsan", ["-O1", "-g", "-fsanitize=thread"])]
                for variant, options in variants:
                    binary = scratch / variant
                    run(variant + "-build", ["xcrun", "clang", *flags, *options, "-o", binary])
                    for mode in ("alert", "open", "save", "open-no-selection", "save-accept", "cancel", "pre-attach", "cancel-idle", "window", "two", "stop", "external-stop"):
                        run(variant + "-" + mode, [binary, mode], markers=["PASS:sheets:"], timeout=35)
                    state_binary = scratch / (variant + "-state")
                    state_flags = [sdk / "tests/state.m" if item == sdk / "tests/sheets.m" else item for item in flags]
                    run(variant + "-state-build", ["xcrun", "clang", *state_flags, *options, "-o", state_binary])
                    run(variant + "-state", [state_binary], markers=["PASS:state:"], timeout=20)
            if not args.native_only:
                native = sdk / "native/src/macos.m"
                native.write_text(native.read_text() + "\n" + (sdk / "tests/sheets/probe.m").read_text())
                source = sdk / "src/macos.rinzu"
                source.write_text(source.read_text() + HOOK)
                header = sdk / "native/include/rinzu_macos.h"
                header.write_text(header.read_text() +
                    "\nint32_t rinzu_macos_test_sheet_reply(void *, int32_t, const uint8_t *, size_t);\n"
                    "int32_t rinzu_macos_test_sheet_visible(void *);\n")
                app = sdk / "tests/sheets"
                (app / "rinzu.toml").write_text('[package]\nname="macos_sheets_tests"\nlanguage="0.5"\n[dependencies]\n"rinzu.macos"={path="../.."}\n')
                evidence["transformedInputs"] = {str(path.relative_to(scratch)): hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in (native, source, header, app / "rinzu.toml")}
                run("source-check", [cli, "check", app])
                binary = scratch / "app"
                run("source-build", [cli, "build", app, "--output", binary])
                if args.keep:
                    args.keep.mkdir(parents=True, exist_ok=True)
                    for item in (binary, Path(str(binary) + ".metadata.json")):
                        shutil.copy2(item, args.keep / item.name)
                    shutil.copytree(Path(str(binary) + ".source-packages"), args.keep / "app.source-packages")
                    shutil.copytree(sdk, args.keep / "sdk/macos")
                    evidence["retainedProgram"] = str(args.keep / "app")
                    evidence["programSha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
                path = scratch / "selected 日本 e\u0301 😀.txt"
                path.write_text("owned file path\n")
                modes = ("results", "cancel", "stop", "scope") if args.mode == "all" else (args.mode,)
                for mode in modes:
                    outputs = []
                    for backend in ("aot", "jit"):
                        report = scratch / (mode + "-" + backend + ".jsonl")
                        command = [cli, "run", app, "--resource-report", report, "--resource-interval-ms", "0"] if backend == "jit" else [binary]
                        outputs.append(run("source-" + mode + "-" + backend, command, 1 if mode == "cancel" else 0,
                            extra={"RINZU_SHEET_TEST": mode, "RINZU_SHEET_PATH": str(path),
                                   "RINZU_SHEET_DIRECTORY": str(scratch),
                                   "RINZU_RESOURCE_REPORT": str(report), "RINZU_RESOURCE_INTERVAL_MS": "0"},
                            markers=["SHEETS:closed"] + ([] if mode == "cancel" else [f"SHEETS:{mode}:ok"]),
                            timeout=30 if backend == "aot" else 300))
                    assert outputs[0]["stdout"] == outputs[1]["stdout"], outputs
                    evidence["cases"].append({"name": "source-" + mode + "-equivalence", "passed": True,
                        "comparison": "exit, stdout, terminal runtime live=0"})
                    save()
                for iteration in range(args.cancel_repeats):
                    report = scratch / ("cancel-repeat-" + str(iteration) + ".jsonl")
                    run("cancel-repeat-" + str(iteration), [binary], 1,
                        extra={"RINZU_SHEET_TEST": "cancel", "RINZU_RESOURCE_REPORT": str(report),
                               "RINZU_RESOURCE_INTERVAL_MS": "0"}, markers=["SHEETS:closed"], timeout=10)
                if args.return_cancel:
                    acquired_sdk = scratch / "sdk/macos-acquisition"
                    shutil.copytree(sdk, acquired_sdk)
                    acquired_api = acquired_sdk / "src/macos.rinzu"
                    acquired_api.write_text(delayed_native_return(acquired_api.read_text(), "rawPanelBegin", "SHEETS:ACQUIRED"))
                    acquired_app = acquired_sdk / "tests/sheets"
                    acquired_binary = scratch / "acquired"
                    run("return-cancel-build", [cli, "build", acquired_app, "--output", acquired_binary])
                    report = scratch / "return-cancel-aot.jsonl"
                    run("return-cancel-aot", [acquired_binary], 1,
                        extra={"RINZU_SHEET_TEST": "acquisition", "RINZU_CANCEL_AFTER_POLLS": "5000",
                               "RINZU_RESOURCE_REPORT": str(report), "RINZU_RESOURCE_INTERVAL_MS": "0"},
                        markers=["SHEETS:ACQUIRED", "SHEETS:closed"])
                    # `run` intentionally ignores the standalone AOT cancel
                    # environment variable. The verifier supplies the typed
                    # JIT's Root control through its supported options instead.
                    run("return-cancel-equivalence", [cli, "verify-aot", acquired_app, "--cancel-after-polls", "5000"],
                        extra={"RINZU_SHEET_TEST": "acquisition"},
                        markers=["JIT/AOT equivalent:", "outcome=cancelled", "resources=baseline"], timeout=600)
            evidence["passed"] = True
    finally:
        save()


if __name__ == "__main__":
    main()
