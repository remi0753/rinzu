# CLI commands

Use `rinzu help` for the complete command list and `rinzu --version` for the toolchain version. Explicit paths make file/package selection clear.

| Command | Purpose |
| --- | --- |
| `doctor` | Report the host target and execution support |
| `check [file or directory]` | Validate source, packages, and supplied native inputs |
| `run [file or directory]` | Execute with JIT |
| `build [file or directory] --output path` | Produce AOT output |
| `verify-aot [file or directory]` | Compare supported JIT/AOT behavior and captured output |
| `fmt paths... --check` / `--write` | Check or apply source formatting |
| `test [package]` | Execute the package's main |
| `test [package] --unit` | Discover and run test functions |
| `test [package] --doc file.md` | Run explicit documentation examples |
| `lock [package]` / `lock [package] --check` | Create/update or check a package lock |
| `api [sdk] --output file.json --markdown file.md` | Export a checked public API |
| `dev [package]` | Recheck and restart an application after edits |
| `bundle [package] --output app.app` | Build a macOS application bundle |
| `bundle-verify app.app` | Verify a generated bundle |
| `toolchain-bundle --output new-directory` | Export the toolchain and bundled resources |
| `diagnose report.json` | Analyze a captured execution report |
| `editor --stdio [package]` | Start the versioned editor protocol |

The `release`, `verify-appkit`, `verify-async`, `run-empty`, and `native-quality-sources` commands are specialized implementation/development entry points. Use build or bundle for ordinary application distribution.

## Shared inputs

File inputs to check/run/build/verify-aot accept `--language 0.5` and the host `--target`. Explicit files do not inherit a nearby manifest. Additional program arguments and cross-compilation are unavailable.

Native input options are `-L path`, `-l name`, `--native-object path`, and optional `--native-header path`. Repeat the same inputs for checking, locks, execution, and builds. See [Native interoperability](../language/native-interop.md).

`run --trace` enables execution trace output. `run` and plain package `test` support `--resource-report file` and `--resource-interval-ms n`. The default interval is 100 ms; zero records start/end only. `run --diagnostic-report file` writes a diagnostic report. See [Diagnostics](diagnostics.md).

## Test selection

`--unit --list` lists cases; `--filter text` selects them; `--format json` produces machine-readable results; `--timeout-ms n` bounds a worker run. `--doc` accepts explicit Markdown files. See [Testing](testing.md) for case discovery and isolation.

## Output paths

An explicit `--output` makes application output placement predictable. Default package builds use a build directory under the package. SDK builds generate a directory of descriptors, facades, and related inputs. Keep generated output out of source control unless it is an intentional published artifact.

Diagnostics normally go to stderr, while program output stays on stdout. Command failures return a nonzero exit status. A program's explicit successful exit status is returned only after required cleanup.
