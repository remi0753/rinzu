# Standard library overview

The toolchain bundles the following modules. Import them directly in either a standalone file or a package. Do not add them to `[dependencies]`: standard modules cannot be replaced with local packages.

| Module | Purpose | Details |
| --- | --- | --- |
| `rinzu.core` | Built-in types, text, bytes, numeric and task operations | [Core APIs](core.md) |
| `rinzu.list` | List queries, immutable updates and bulk construction | [Collection APIs](collections.md) |
| `rinzu.map` | Map queries, immutable updates and bulk construction | [Collection APIs](collections.md) |
| `rinzu.option` | Optional transformations, chained lookups and lazy defaults | [Option helpers](../language/enums.md#option-helpers) |
| `rinzu.stdio` | Synchronous display of language values | Below |
| `rinzu.timer` | Asynchronous sleep | Below |
| `rinzu.io` | Files, standard input/output, clocks, resource cleanup | [I/O](io.md) |
| `rinzu.process` | Program arguments, environment, process ID, exit status, error output | Below |
| `rinzu.json` | Validate, read, build and write JSON | [JSON](json.md) |
| `rinzu.stream` | Pull-based file streams | [I/O](io.md) |
| `rinzu.testing` | Assertions | [Testing](../tooling/testing.md) |
| `rinzu.shutdown` | Signal registration and cooperative shutdown | [Async](../language/async.md) |
| `rinzu.http` | Bounded HTTP/HTTPS client | [Networking](networking.md) |
| `rinzu.http.server` | HTTP route registration and dispatch | [Networking](networking.md#http-application-dispatch) |
| `rinzu.socket` | Nonblocking TCP listener and stream | [Networking](networking.md) |
| `rinzu.httpcodec` | Request-line scanning for HTTP-oriented code | [Source API](../../sdk/httpcodec/src/httpcodec.rinzu) |

Core operations are available by unqualified names without an import. Other modules use their qualified module names.

## Output and timers

```rinzu-test
import rinzu.stdio;
import rinzu.timer;

async fn main() {
    try {
        rinzu.stdio.writeLine("starting");
        await rinzu.timer.sleep(1);
        rinzu.stdio.writeLine("finished");
    }
    catch problems {
        panic("standard library operation failed");
    }
}
```

`rinzu.stdio.write(value)` and `writeLine(value)` display constructible values; writeLine appends a newline. Displaying a Future does not await it or observe its failure. Program output goes to stdout, while compiler and execution diagnostics go to stderr.

`rinzu.timer.sleep(milliseconds)` requires a nonnegative Int. Zero still yields. Negative delays throw Error. A sleeping task does not occupy Main's thread while pending.

## Process settings

`rinzu.process.arguments()` returns the immutable list of program arguments in Main. The first invalid UTF-8 argument throws `InvalidArgumentEncoding` with a zero-based argument index and byte offset. A Worker can receive the list or a selected value as a parameter. See [CLI commands](../tooling/cli.md#program-arguments).

`rinzu.process.environment(name)` returns `Option<String>` with a UTF-8 snapshot. `setting(name,defaultValue)` selects a default when absent. `integer(name,defaultValue,minimum,maximum)` additionally parses and checks the range. Malformed or oversized settings throw Error.

`identifier()` returns the process ID. `setExitStatus(status)` accepts 0 through 255 and returns Bool; use it from async Main. `writeErrorLine(text)` writes a bounded UTF-8 line to stderr, returning a native status result. The line including its newline is limited to 4096 bytes and can block if its peer does not read.

## Additional packages

SQLite, codecs, numeric logging, SDL2, macOS GUI, and the callback-consumption demonstration SDK are additional packages. See [Additional SDKs](sdk.md) and [Manifests](../packages/manifest.md). Sources under `sdk/` are also the detailed public declarations used to generate API references.
