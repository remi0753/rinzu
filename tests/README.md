# Compiler and runtime test inputs

fixtures/programs contains only programs used by compiler, codegen, or driver regression tests. Other fixtures cover the native ABI, callbacks, functions, state, and HTTP parsing. These are test inputs rather than an application sample catalog.

integration contains Python runners invoked by Rust tests or native quality checks. runtime-model is a separate Cargo package exercised with cargo xtask model. conformance contains deterministic vectors and trace-schema fixtures.

Generated binaries, test captures, and resource reports are local artifacts and are not committed.

[Compiler development](../docs/contributing.md)
