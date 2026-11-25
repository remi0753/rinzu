# Native interoperability

A `native` block declares the Rinzu view of C-compatible symbols. It does not provide their implementation. Supply native sources through a package manifest, or object files and libraries through CLI inputs.

## Call a system symbol

Save this complete program as `absolute.rinzu`:

```rinzu
native {
    fn absolute(value: CInt) -> CInt symbol "abs";
}

fn main() {
    try {
        let result = absolute(CInt(-42));
        if Int(result) != 42 {
            panic("wrong native result");
        }
    }
    catch problems {
        panic("native operation failed");
    }
}
```

On macOS, explicitly select the system library for each command.

```sh
rinzu check absolute.rinzu -lSystem
rinzu run absolute.rinzu -lSystem
rinzu build absolute.rinzu -lSystem --output ./absolute
```

On x86_64 Linux, build against glibc and run the generated ELF.

```sh
rinzu build absolute.rinzu -lc --output ./absolute
./absolute
```

A caller's native scope is empty by default. Runtime or SDK dependencies do not grant direct access to their symbols. `-L` supplies search paths, `-l` selects libraries, and `--native-object` selects objects. An optional `--native-header` checks declarations; a header or search path alone grants no symbol access.

## Data at the boundary

`opaque Handle;` declares a native reference without fields. A native record declares nonempty ABI scalar fields:

```rinzu
native {
    opaque Handle;

    record NativeResult {
        status: CInt,
        value: Int
    }

    fn calculate(value: Int, output: NativeResult) -> Unit symbol "example_calculate";
}
```

A native record is a shared reference borrowed during the call and written back automatically. Reading its contents or borrowing it again while it is loaned can fail with Error. Opaque values and native records can be retained in the same Domain but cannot cross a Worker boundary.

Validate the complete record argument list before acquiring its loans. A busy
argument causes preparation to fail before native entry. Earlier validated
arguments preserve their prior ownership. The Call holding the active loan keeps
its buffer and staged writes through its own writeback boundary.

A language record passed through the native boundary is a checked call-context token, not a C struct expanded by value. Classes and enums have no direct native ABI mapping; use a supported wrapper. Native opaque values and native records cannot define methods.

## Callbacks, ownership, and cleanup

Supported call-lifetime Main callbacks can capture same-Domain class state. Function expressions can specify a checked native fallback, for example `fn(value: Int) -> Int fallback 0 { return value; }`. Linux callbacks run on the invoking native call's Owner thread. A callback from another thread without a compatible Origin fails with exit status 86.

Native resource ownership includes physical thread/Owner requirements. GC does not substitute for close, finalize, or native completion. Await resource users before closing handles. Borrowed callback values must not escape their allowed lifetime.

Cancellation does not end a native user's access to its environment. A completion carrier, managed captures, callback receipts, and retained wake targets have separate retirement conditions. A parent's finally may run before its children physically settle. See [Native lifetime during cancellation](../abi/native-lifetime.md) for the normative state machines and provider requirements.

Closed synchronous native consumers can support more precise callback return cleanup when the toolchain proves their usage. Unknown consumers retain the conservative lifetime contract. This does not provide general persistent capturing callback registration.

## Author a native SDK

Use [SDK authoring](../packages/authoring.md) for manifests and typed C/C++/ObjC++ scaffolds. See [C ABI](../abi/README.md) for headers and schema resources. Unresolved or duplicate providers, incompatible targets, and checked prototype mismatches are rejected. Incorrect ABI declarations, invalid pointers, and arbitrary native crashes are not generally converted into recoverable Errors.

Continue with [Standard libraries](../libraries/overview.md).
