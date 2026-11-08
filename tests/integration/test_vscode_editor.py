"""Run the Rinzu integration tests in an installed VS Code desktop host."""

import argparse
from contextlib import suppress
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "tools/vscode-rinzu"
CATCH_SOURCE = """error WriteFailed {}

error CloseFailed {}

fn persist() {
    try {
        throw WriteFailed(message: "disk full");
    }
    finally {
        throw CloseFailed(message: "close failed");
    }
}

fn main() {
    try {
        persist();
    }
    catch [first: WriteFailed] {}
}
"""


def default_code():
    if sys.platform == "darwin":
        return Path("/Applications/Visual Studio Code.app/Contents/MacOS/Code")
    executable = shutil.which("code")
    return Path(executable) if executable else None


def write_fixture(workspace, profile, rinzu):
    def write(name, text):
        path = workspace / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode("utf-8"))

    write("rinzu.toml", '[package]\nname = "host"\nlanguage = "0.5"\n')
    write("src/main.rinzu", '\ufeff// 😀日本\r\nimport model;fn main(){let n:Int=model.value();}')
    write("src/model.rinzu", 'fn value()->String{return "disk";}fn hidden()->Int{return 7;}export value;')
    write("src/logic.rinzu", "fn logic() {}\n")
    write("coverage/rinzu.toml", '[package]\nname = "coverage"\nlanguage = "0.5"\n')
    write("coverage/src/main.rinzu", CATCH_SOURCE)
    write(".vscode/settings.json", json.dumps({"rinzu.executable": str(rinzu)}))

    settings = profile / "User/settings.json"
    settings.parent.mkdir(parents=True)
    settings.write_text(json.dumps({
        "chat.disableAIFeatures": True,
        "extensions.autoCheckUpdates": False,
        "extensions.autoUpdate": False,
        "telemetry.telemetryLevel": "off",
        "update.mode": "none",
        "workbench.startupEditor": "none",
    }), encoding="utf-8")


def run_host(code, rinzu, root, timeout):
    workspace = root / "workspace"
    profile = root / "profile"
    evidence = root / "results.json"
    write_fixture(workspace, profile, rinzu)
    command = [
        str(code),
        f"--extensionDevelopmentPath={EXTENSION}",
        f"--extensionTestsPath={EXTENSION / 'tests.cjs'}",
        f"--user-data-dir={profile}",
        f"--extensions-dir={root / 'extensions'}",
        "--disable-extensions",
        "--disable-workspace-trust",
        "--skip-welcome",
        "--skip-release-notes",
        str(workspace),
    ]
    environment = dict(os.environ, RINZU_EDITOR_HOST_EVIDENCE=str(evidence))
    with (root / "host.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen(
            command,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            status = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(f"VS Code host exceeded {timeout} seconds") from error
        finally:
            if process.poll() is None:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    with suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    if status:
        if sys.platform == "darwin" and status == -signal.SIGABRT and not evidence.exists():
            raise RuntimeError(
                "VS Code aborted before reporting tests. Run from a macOS desktop session "
                "with permission to register applications."
            )
        raise RuntimeError(f"VS Code host exited with status {status}")

    result = json.loads(evidence.read_text(encoding="utf-8"))
    if result.get("schema") != "rinzu-editor-host-tests/1" or not result.get("cases"):
        raise RuntimeError("VS Code did not report its integration test cases")
    for case in result["cases"]:
        print(f"{case['status']}  {case['name']}")
    if any(case["status"] != "passed" for case in result["cases"]):
        raise RuntimeError("VS Code reported failed integration tests")
    print(f"{len(result['cases'])} cases passed in VS Code {result['editorVersion']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--code", type=Path, default=default_code(), help="installed VS Code executable")
    parser.add_argument("--rinzu", type=Path, default=ROOT / "target/debug/rinzu")
    parser.add_argument("--timeout", type=int, default=180, help="host timeout in seconds")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("the desktop host runner requires macOS or Linux")
    if args.code is None or not args.code.is_file() or not os.access(args.code, os.X_OK):
        parser.error("pass the executable of an installed desktop VS Code with --code")
    if not args.rinzu.is_file() or not os.access(args.rinzu, os.X_OK):
        parser.error("build the Rinzu driver and pass its executable with --rinzu")
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    with tempfile.TemporaryDirectory(prefix="rz-vscode-") as temporary:
        root = Path(temporary).resolve()
        try:
            run_host(args.code.resolve(), args.rinzu.resolve(), root, args.timeout)
        except (OSError, RuntimeError, ValueError) as error:
            log = root / "host.log"
            if log.exists():
                print(log.read_text(encoding="utf-8", errors="replace")[-20000:], file=sys.stderr)
            print(error, file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
