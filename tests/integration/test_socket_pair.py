#!/usr/bin/env python3
"""Run the independent Socket SDK client and server against each other."""
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TESTS = ROOT / "sdk/socket/tests"
CLEANUP = "CLEANUP:0:0:0:0:"


def replay(rinzu, work):
    env = dict(os.environ, SOCKET_MODE="echo", TMPDIR=str(work), RINZU_CACHE_DIR=str(work / "cache"))
    for side in ("server", "client"):
        source = work / f"{side}-source"
        shutil.copytree(TESTS / side, source, ignore=shutil.ignore_patterns("build", ".rinzu"))
        subprocess.run([rinzu, "build", source, "--output", work / side],
                       cwd=work, env=env, check=True, timeout=120)
    with tempfile.TemporaryFile(dir=work) as errors:
        with subprocess.Popen([work / "server"], cwd=work, env=env,
                              stdout=subprocess.PIPE, stderr=errors) as server:
            try:
                ready, _, _ = select.select([server.stdout], [], [], 20)
                line = server.stdout.readline().decode() if ready else ""
                if not line.startswith("READY:"):
                    errors.seek(0)
                    raise AssertionError((line, errors.read().decode(errors="replace")))
                port = int(line.removeprefix("READY:"))
                assert 0 < port <= 65535, port
                client = subprocess.run([work / "client"], cwd=work,
                                        env=dict(env, SOCKET_PORT=str(port)),
                                        capture_output=True, text=True, timeout=30)
                assert client.returncode == 0 and CLEANUP in client.stdout, client
                stdout, _ = server.communicate(timeout=30)
                errors.seek(0)
                assert server.returncode == 0 and CLEANUP.encode() in stdout, (
                    server.returncode, stdout, errors.read()
                )
            finally:
                if server.poll() is None:
                    server.kill()
                    server.wait()


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="rinzu-socket-pair-") as directory:
        replay(Path(sys.argv[1]).resolve(), Path(directory))
