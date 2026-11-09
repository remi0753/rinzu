#!/usr/bin/env python3
"""Exercise the HTTP SDK against disposable loopback HTTP and TLS servers."""
from contextlib import contextmanager, suppress
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import ssl
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
SDK = ROOT / "sdk/http"
BINARY = bytes(range(256)) * 32


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body=b"", length=None, headers=()):
        with suppress(BrokenPipeError, ConnectionResetError):
            self.send_response(status)
            self.send_header("Content-Length", str(len(body) if length is None else length))
            for key, value in headers:
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body)

    def do_GET(self):
        if self.path == "/slow":
            time.sleep(0.15)
            self.reply(200, BINARY)
        elif self.path == "/binary":
            self.reply(200, BINARY)
        elif self.path == "/disconnect":
            self.reply(200, BINARY[:7], length=len(BINARY))
        elif self.path == "/huge":
            self.reply(200, BINARY * 9)
        elif self.path == "/redirect":
            self.reply(302, headers=[("Location", "/binary")])
        elif self.path == "/status":
            self.reply(503)
        else:
            self.reply(404)

    def do_POST(self):
        if self.path != "/echo":
            self.reply(404)
            return
        self.reply(200, self.rfile.read(int(self.headers["Content-Length"])))


@contextmanager
def server(tls=None):
    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as listener:
        if tls is not None:
            listener.socket = tls.wrap_socket(listener.socket, server_side=True)
        thread = threading.Thread(target=listener.serve_forever)
        thread.start()
        try:
            scheme = "http" if tls is None else "https"
            yield f"{scheme}://127.0.0.1:{listener.server_port}"
        finally:
            listener.shutdown()
            thread.join()


def checked(args, work, marker=None, status=0):
    env = dict(os.environ, TMPDIR=str(work), RINZU_CACHE_DIR=str(work / "cache"))
    result = subprocess.run(args, cwd=work, env=env, capture_output=True, text=True, timeout=120)
    if result.returncode != status or (marker is not None and marker not in result.stdout):
        raise AssertionError((args, result.returncode, result.stdout, result.stderr))
    return result


def replay(rinzu, work):
    cert = work / "server.pem"
    key = work / "server.key"
    config = work / "cert.cnf"
    config.write_text(
        "[req]\ndistinguished_name=dn\nprompt=no\nx509_extensions=server\n"
        "[dn]\nCN=127.0.0.1\n[server]\nsubjectAltName=IP:127.0.0.1\n"
    )
    checked(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-days", "1", "-config", config, "-keyout", key, "-out", cert], work)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(cert, key)

    with server() as http, server(tls) as https:
        native = work / "native"
        checked(["clang", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread",
                 "-I", ROOT / "include", "-I", SDK / "native/include",
                 SDK / "tests/fault.c", "-lcurl", "-o", native], work)
        mismatched = https.replace("127.0.0.1", "localhost")
        checked([native, f"{https}/binary", f"{mismatched}/binary", cert], work, "http-fault-ok")
        for name, marker, status in [("main", "http-basic-ok", 0), ("stress", "http-stress-ok", 1)]:
            source = work / f"{name}.rinzu"
            original = (SDK / f"tests/{name}.rinzu").read_text()
            source.write_text(original.replace("SSLBASE", https).replace("BASE", http))
            binary = work / name
            checked([rinzu, "build", source, "--output", binary], work)
            for command in ([rinzu, "run", source], [binary]):
                result = checked(command, work, marker, status)
                if status == 1:
                    assert "RINZU_FAILURE=cancelled" in result.stderr, result


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="rinzu-http-sdk-") as directory:
        replay(Path(sys.argv[1]).resolve(), Path(directory))
