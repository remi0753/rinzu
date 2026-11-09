"""Exercise the editor protocol against unsaved C text without changing files."""
import json
import queue
import subprocess
import tempfile
import threading
import time
import unittest

from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import native_quality as quality


class NativeEditorTests(unittest.TestCase):
    def test_editor_diagnoses_unsaved_text_and_formats_it(self):
        file = quality.ROOT / "crates/runtime/src/stdio.c"
        original = file.read_bytes()
        uri = file.as_uri()
        messages = queue.Queue()
        with tempfile.TemporaryFile() as log:
            process = subprocess.Popen([str(quality.ROOT / "tools/clangd"), "--clang-tidy",
                                        f"--compile-commands-dir={quality.DATABASE}",
                                        "--background-index=false", "--log=error"],
                                       cwd=quality.ROOT, stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=log)

            def read_messages():
                try:
                    while True:
                        headers = {}
                        while True:
                            line = process.stdout.readline()
                            if not line:
                                return
                            if line == b"\r\n":
                                break
                            name, value = line.decode().split(":", 1)
                            headers[name.lower()] = value.strip()
                        messages.put(json.loads(process.stdout.read(int(headers["content-length"]))))
                except (OSError, ValueError):
                    return

            reader = threading.Thread(target=read_messages, daemon=True)
            reader.start()

            def send(message):
                body = json.dumps({"jsonrpc": "2.0", **message}).encode()
                process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
                process.stdin.flush()

            def receive(predicate):
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    try:
                        message = messages.get(timeout=deadline - time.monotonic())
                    except queue.Empty:
                        break
                    if predicate(message):
                        return message
                log.seek(0)
                self.fail("clangd response timed out: " + log.read().decode(errors="replace"))

            try:
                send({"id": 1, "method": "initialize", "params": {
                    "processId": None, "rootUri": quality.ROOT.as_uri(), "capabilities": {}}})
                receive(lambda message: message.get("id") == 1)
                send({"method": "initialized", "params": {}})
                text = "int probe( void){return undeclared_value;}\n"
                send({"method": "textDocument/didOpen", "params": {"textDocument": {
                    "uri": uri, "languageId": "c", "version": 1, "text": text}}})
                diagnostic = receive(lambda message: message.get("method") == "textDocument/publishDiagnostics"
                                     and message["params"].get("uri") == uri
                                     and message["params"].get("diagnostics"))
                self.assertTrue(any("undeclared_value" in item["message"]
                                    for item in diagnostic["params"]["diagnostics"]))
                send({"id": 2, "method": "textDocument/formatting", "params": {
                    "textDocument": {"uri": uri}, "options": {"tabSize": 2, "insertSpaces": True}}})
                edits = receive(lambda message: message.get("id") == 2)["result"]
                self.assertTrue(edits)
                lines = text.splitlines(keepends=True)
                for edit in reversed(edits):
                    start, end = edit["range"]["start"], edit["range"]["end"]
                    begin = sum(len(line) for line in lines[:start["line"]]) + start["character"]
                    finish = sum(len(line) for line in lines[:end["line"]]) + end["character"]
                    text = text[:begin] + edit["newText"] + text[finish:]
                self.assertIn("int probe(void) {", text)
                self.assertIn("\n  return undeclared_value;\n", text)
                send({"method": "textDocument/didChange", "params": {
                    "textDocument": {"uri": uri, "version": 2},
                    "contentChanges": [{"text": text.replace("undeclared_value", "7")}]}})
                receive(lambda message: message.get("method") == "textDocument/publishDiagnostics"
                        and message["params"].get("uri") == uri
                        and not message["params"].get("diagnostics"))
                text = ('#include <string.h>\n'
                        'int probe(const char *left, const char *right) {\n'
                        '  if (strcmp(left, right)) return 1;\n  return 0;\n}\n')
                send({"method": "textDocument/didChange", "params": {
                    "textDocument": {"uri": uri, "version": 3},
                    "contentChanges": [{"text": text}]}})
                diagnostic = receive(lambda message: message.get("method") == "textDocument/publishDiagnostics"
                                     and message["params"].get("uri") == uri
                                     and message["params"].get("diagnostics"))
                self.assertTrue(any(item.get("code") == "bugprone-suspicious-string-compare"
                                    for item in diagnostic["params"]["diagnostics"]), diagnostic)
                send({"method": "textDocument/didChange", "params": {
                    "textDocument": {"uri": uri, "version": 4},
                    "contentChanges": [{"text": text.replace("if (strcmp(left, right))",
                                                             "if (strcmp(left, right) != 0)")}]}})
                receive(lambda message: message.get("method") == "textDocument/publishDiagnostics"
                        and message["params"].get("uri") == uri
                        and not message["params"].get("diagnostics"))
                header = quality.ROOT / "include/rinzu/scalar.h"
                header_text = header.read_text()
                send({"method": "textDocument/didOpen", "params": {"textDocument": {
                    "uri": header.as_uri(), "languageId": "c", "version": 1, "text": header_text}}})
                diagnostic = receive(lambda message: message.get("method") == "textDocument/publishDiagnostics"
                                     and message["params"].get("uri") == header.as_uri())
                self.assertFalse(diagnostic["params"]["diagnostics"], diagnostic)
                self.assertEqual(header.read_text(), header_text)
                self.assertEqual(file.read_bytes(), original)
                send({"id": 3, "method": "shutdown", "params": None})
                receive(lambda message: message.get("id") == 3)
                send({"method": "exit", "params": None})
                process.wait(timeout=5)
                self.assertEqual(process.returncode, 0)
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait()
                process.stdin.close()
                process.stdout.close()
                reader.join(timeout=1)

if __name__ == "__main__":
    unittest.main()
