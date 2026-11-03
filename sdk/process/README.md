# rinzu.process

Bundled process settings and status APIs. Import directly without a path dependency.

environment(name) returns Option<String> with a UTF-8 snapshot. setting(name,defaultValue) supplies a default only for absence; integer(name,defaultValue,minimum,maximum) parses and bounds an integer. Invalid names, oversized data, malformed UTF-8, parse failures, and invalid ranges throw Error.

identifier returns the process ID. setExitStatus accepts 0..255 from async Main and returns Bool. It does not terminate execution; required cleanup finishes before the process returns the status. Runtime failure remains a failure.

writeErrorLine writes a UTF-8 line synchronously to stderr, including embedded NUL. The newline-inclusive limit is 4096 bytes. It can block if the peer does not read. Binary stdin/stdout use rinzu.io.

[Guide](../../docs/libraries/overview.md) · [API reference](../../docs/reference/sdk-process.md) · [Source](src/process.rinzu)
