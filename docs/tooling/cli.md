# CLI commands

Use `rinzu help` for the complete command list and `rinzu --version` for the toolchain version. Explicit paths make file/package selection clear.

| Command                                              | Purpose                                                                  |
| ---------------------------------------------------- | ------------------------------------------------------------------------ |
| `doctor [--sdk name]`                                | Report the host, toolchain catalog, and selected SDK requirements        |
| `check [file or directory]`                          | Validate source, packages, and supplied native inputs                    |
| `run [file or directory] [-- args...]`               | Execute with JIT                                                         |
| `build [file or directory] --output path`            | Produce AOT output                                                       |
| `verify-aot [file or directory] [-- args...]`        | Compare JIT/AOT behavior and captured output                             |
| `fmt paths... --check` / `--write`                   | Check or apply source formatting                                         |
| `test [package]`                                     | Execute the package's main                                               |
| `test <tests directory or .test.rinzu file>`         | Discover and run test files                                              |
| `test [package] --doc file.md`                       | Run explicit documentation examples                                      |
| `modules add <SDK name> <HTTPS URL or archive path>` | Add a source SDK and pin its dependency graph                            |
| `modules sync [--offline]`                           | Restore the SDKs pinned in the current project's modules lock            |
| `modules relock`                                     | Refresh declaration fingerprints with the same installed, pinned sources |
| `lock [package]` / `lock [package] --check`          | Create/update or check a package lock                                    |
| `api [sdk] --output file.json --markdown file.md`    | Export a checked public API                                              |
| `dev [package] [-- args...]`                         | Recheck and restart an application after edits                           |
| `bundle [package] --output app.app`                  | Build a macOS application bundle                                         |
| `bundle-verify app.app`                              | Verify a generated bundle                                                |
| `toolchain-bundle --output new-directory`            | Export the toolchain and bundled resources                               |
| `diagnose report.json`                               | Analyze a captured execution report                                      |
| `editor --stdio [package]`                           | Start the versioned editor protocol                                      |

The `release`, `verify-appkit`, `verify-async`, `run-empty`, and `native-quality-sources` commands are specialized implementation/development entry points. Use build or bundle for ordinary application distribution.

## Shared inputs

File inputs to check/run/build/verify-aot accept `--language 0.5` and the host `--target`. Explicit files do not inherit a nearby manifest.

Native input options are `-L path`, `-l name`, `--native-object path`, and optional `--native-header path`. They are available for test files as well. Repeat the same inputs for checking, locks, execution, and builds. See [Native interoperability](../language/native-interop.md).

## Program arguments

Put driver options and the source path before `--` with `run`, `verify-aot`, or `dev`. Each value after the separator reaches `rinzu.process.arguments()` in order, including empty values and additional `--` tokens. A directly launched AOT executable passes every argument after its executable name, including a leading `--`. `dev` reuses the arguments on every restart. Test entries receive an empty list.

```sh
rinzu run ./report -- '取引 11月.json'
rinzu verify-aot ./report -- '取引 11月.json'
rinzu build ./report --output ./report-aot
./report-aot '取引 11月.json'
```

Relative file arguments are resolved by the application against its working directory. See [process](../libraries/overview.md#process-settings) and [JSON](../libraries/json.md).

`run --trace` enables execution trace output. `run` and plain package `test` support `--resource-report file` and `--resource-interval-ms n`. The default interval is 100 ms; zero records start/end only. `run --diagnostic-report file` writes a diagnostic report. See [Diagnostics](diagnostics.md).

## JIT progress

`run` prints major preparation stages, the start of Main, and the final exit status to stderr. Every progress row starts with `[rinzu:jit]` and elapsed time since preparation began. Stage labels are in English. The Main row includes startup time; the final row includes execution time when Main started. A failed run names the last stage reached.

Frontend progress distinguishes `Checking frontend cache`, `Validating cached inputs`, `Verifying cached IR`, `Resolving dependencies`, `Checking dependency: <name>`, `Parsing application sources`, `Checking application types`, `Preparing and verifying IR`, and `Preparing frontend cache`. Rows appear when the corresponding work starts. A valid frontend cache skips dependency checking, source parsing, type checking, and IR generation. A stale cache falls back to the required source checks. Dependency names include local SDKs and imported toolchain packages; a dependency can appear again when its own checks resume after checking an import.

Native dependencies can be prepared while source checking continues. `Waiting for native dependencies` appears only if a successful frontend check must still wait for that preparation to finish. Skipped steps are not printed, and phase order follows the actual file/package and cache path.

`--progress=auto` is the default: progress is shown only when both stdout and stderr are terminals and no CI environment is detected. Redirecting either stream or piping program output disables automatic progress. CI detection recognizes `CI`, `CONTINUOUS_INTEGRATION`, and common provider variables (`GITHUB_ACTIONS`, `GITLAB_CI`, `TF_BUILD`, `JENKINS_URL`, `TEAMCITY_VERSION`, `BUILDKITE`, `CIRCLECI`, `TRAVIS`); empty values, `0`, and `false` do not enable CI detection.

Use `--progress=always` to show progress in a pipe or CI, or `--progress=never` to suppress it in a terminal. The space-separated form, such as `--progress always`, is also accepted. Put this option before `--` so it is handled by Rinzu rather than passed to the application.

```sh
rinzu run ./app --progress=always
rinzu run ./app --progress=never -- application-argument
```

Progress uses plain, complete lines without terminal control sequences. Program stdout, existing result summaries, explicit traces, and error diagnostics keep their usual behavior. Applications can also write to stderr; the `[rinzu:jit]` prefix distinguishes progress rows there.

## Test selection

From a package root, `rinzu test ./tests` discovers `.test.rinzu` files. Use `--list` to inspect the selected files and `--filter TEXT` to match a substring of an entry ID before execution. To run one named case, pass a single test file and `--case NAME`.

`--format json` writes structured results, and `--timeout-ms N` bounds each worker run. Use `--doc FILE` for explicit Markdown examples. Use `rinzu test .` to run the package's application `main`. See [Testing](testing.md) for examples and report fields.

## Output paths

An explicit `--output` makes application output placement predictable. Default package builds use a build directory under the package. SDK builds generate a directory of descriptors, facades, and related inputs. Keep generated output out of source control unless it is an intentional published artifact.

Diagnostics normally go to stderr, while program output stays on stdout. Command failures return a nonzero exit status. A program's explicit successful exit status is returned only after required cleanup.

## Local compiler cache

`RINZU_CACHE_DIR` selects the user cache directory. The compiled JIT records checked frontend results, relocatable machine code, function stencils, and eligible native/runtime images after native ABI/startup validation and executable publication succeed, before Main runs. Long-running applications therefore populate the cache even if they later stop by signal or exit with an application error. A JIT compilation, ABI validation, or publication failure discards its pending cache entries. Every restart still validates the current inputs and native images.

`check`, `build`, and `bundle` share the checked frontend cache with `run`. AOT builds also reuse the compiled runtime object when its Clang inputs match. Each build validates current native inputs and links the executable. Bundling also validates and signs the new app.

For large modules on multicore hosts, the compiler checks Error contracts and Future observation concurrently after Error inference converges. Each pass reads the same inferred summaries. Error contract diagnostics take priority when both passes find issues.

Each namespace managed by the source/JIT artifact cache (`source-v4` and `jit-*`) retains at most 64 completed generations and 256 MiB, including every regular file in a sealed generation. A save removes the oldest published generations until both limits hold; a generation larger than the byte limit is not saved. If abandoned files make the current directory alone exceed the budget, it is reclaimed last and that save is treated as a miss. Reads and saves use a process-shared namespace lock, so collection waits for readers to copy and validate their data. Running programs use private copies and do not keep an evicted persistent generation open. Incomplete generations and unrecognized top-level entries are left alone by collection.

SDK and application builds continue with their validated artifacts if cache saving or reclamation fails. Input/ABI validation and build output write failures still reject the build.

For manual cleanup, stop Rinzu builds/runs and remove the `jit-*` and `source-v4` directories inside the selected cache directory. Remove abandoned/incomplete entries there as well if needed. The adjacent `.jit-*.lock` and `.source-v4.lock` files are coordination files; keep them in place while any process may use the cache. Cache misses rebuild automatically. Other caches, such as downloaded SDK archives and native object compilation, have separate lifetimes.
