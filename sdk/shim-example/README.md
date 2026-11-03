# Typed native shim example

An additional source SDK demonstrating a C-compatible shim with a checked header, typed native result record, and a managed public wrapper. Native records are borrowed during the call and automatically written back. The wrapper returns the successful scalar result and throws on a failing native status.

Generate another scaffold from the compiler checkout:

```sh
python3 tools/new_sdk.py ../my-sdk --name example.native_api --language cpp
```

Choose c, cpp, or objcxx. The destination must not exist. Specify allocator/release, physical Owner/thread, callback lifetime, and cancellation/physical-stop requirements for the real library. Foreign exceptions must remain inside the shim. A scaffold does not infer persistent callback registration or native ownership.

[Authoring guide](../../docs/packages/authoring.md) · [API reference](../../docs/reference/sdk-shim-example.md) · [Source](src/api.rinzu)
