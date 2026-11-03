# Toolchain support tools

| Tool | Purpose |
| --- | --- |
| native_consumption.cpp | Driver input for supported closed native-consumer analysis |
| native_quality.py | Native formatting/linting and generated compilation contexts |
| clangd | Resolve and start the LLVM 21 language server |
| new_sdk.py | Generate a typed C/C++/Objective-C++ SDK scaffold |
| vscode-rinzu/ | Local Rinzu editor extension |

Run integration checks from tests/integration. Build output and execution reports belong in ignored artifact or temporary directories.

[Compiler development](../docs/contributing.md) · [Editor setup](../docs/tooling/editor.md) · [SDK authoring](../docs/packages/authoring.md)
