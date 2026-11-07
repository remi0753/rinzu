# Diagnostics and troubleshooting

Compiler diagnostics include a diagnostic code, message, and source location. Fix the earliest syntax error before interpreting later type information. Package/import diagnostics include the affected graph or path when available.

## Common problems

| Symptom | Check |
| --- | --- |
| Unsupported execution target | Run `rinzu doctor`; runtime execution requires macOS arm64 |
| Import cannot be resolved | Check the entry mode, source root, exports, and direct manifest dependency |
| Standard package cannot be replaced | Remove its `[dependencies]` entry; import it directly |
| Error can escape Main or Worker | Add appropriate exhaustive recovery at that boundary |
| RZ0414: Error contract exceeded | Recover the escaping Error or widen the declared throws bound; nothrow permits none |
| RZ0415: Interface mismatch | Declare conformance on the class/enum and match public instance signatures and Error bounds |
| Future observation error | Await, return, or correctly hand off the possibly failing Future on every path |
| Native library reports a newer minimum macOS version | Set `MACOSX_DEPLOYMENT_TARGET` to a compatible minimum or install a library build that supports the chosen target |
| Native symbol unresolved | Supply the actual object/library; headers and search paths alone do not grant symbols |
| Lock mismatch | Review intended dependency/toolchain changes, then explicitly regenerate the lock |
| AOT startup rejects an image | Preserve sidecars and use a compatible native OS/library context |
| Editor shows no diagnostics | Check language mode, extension development window, executable, workspace/entry, and Output: Rinzu |

## Capture runtime diagnostics

```sh
rinzu run ./app --diagnostic-report /tmp/rinzu-run.json
rinzu diagnose /tmp/rinzu-run.json --format json
```

The analyzer can also accept explicitly supplied resource, native-log, trace, and exit-record files. Diagnostic files contain execution details and should be stored as local artifacts rather than source documentation.

Set `RINZU_ERROR_TRACE=1` before execution to collect Error traces. Set `RINZU_COMPILE_TIMINGS=1` to print compilation phase timings to stderr. Trace collection changes diagnostic information, not which Errors are catchable.

## Resource reports

```sh
rinzu run ./app --resource-report /tmp/rinzu-resources.jsonl --resource-interval-ms 100
```

Run/test reports use JSONL. Interval zero writes start/end snapshots only. AOT executables use `RINZU_RESOURCE_REPORT` and `RINZU_RESOURCE_INTERVAL_MS` environment variables. Reports do not replace normal stdout/stderr.

Read live, cumulative, peak, retained, and copied quantities separately. A retained control or reserved callback entry is different from a live user resource. Unknown measurements remain unknown; do not interpret a missing field or SDK counter as proof about all process memory. See the [resource schema](../abi/resource-report-v1.schema.json).

## Resource shutdown problems

Await resource users, stop intake, and close in finally. A timeout or cancellation request does not force a blocked native call to return. A shutdown-blocker diagnostic identifies native work whose physical completion is still required. Applications need SDK-specific stop/drain policies for that work.
