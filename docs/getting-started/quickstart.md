# Your first program and project

## Start with one file

Create `hello.rinzu` in an empty working directory:

```rinzu-test
import rinzu.stdio;

fn main() {
    try {
        let name = "Rinzu";
        rinzu.stdio.writeLine(f"Hello, {name}!");
    }
    catch problems {
        panic("output failed");
    }
}
```

Check, run, format, and build it:

```sh
rinzu check hello.rinzu
rinzu run hello.rinzu
rinzu fmt hello.rinzu --write
rinzu build hello.rinzu --output ./hello
./hello
```

The program prints `Hello, Rinzu!`. `check` validates source and supplied native inputs; `run` executes with JIT; `build` creates an AOT executable. Keep generated sidecars with that executable, as described in [Distribution](../tooling/distribution.md).

## Create a package

Use this structure when adding files or additional SDK dependencies:

```text
hello/
  rinzu.toml
  src/
    main.rinzu
```

Put the same program in `src/main.rinzu`, and create this manifest:

```toml
[package]
name = "hello"
language = "0.5"
```

Pass the package directory to the CLI:

```sh
rinzu check ./hello
rinzu run ./hello
rinzu build ./hello --output ./hello-app
```

## Run tests

Create test files under `hello/tests/` with names ending in `.test.rinzu`. Each file has its own `main` and can import modules from `hello/src/`. Once you have a test file, list and run the suite from the parent directory.

```sh
rinzu test ./hello/tests --list
rinzu test ./hello/tests
```

Use `rinzu test ./hello` to run the package's application `main` as a smoke check. See [Testing](../tooling/testing.md#write-and-select-tests) for a file that runs multiple named cases.

## File input and package input

| Input                   | Configuration                                                            |
| ----------------------- | ------------------------------------------------------------------------ |
| `rinzu run hello.rinzu` | Uses the explicit file as entry; does not read nearby manifests or locks |
| `rinzu run ./hello`     | Uses the package manifest, entry, dependencies, and existing lock        |

A standalone file can import standard modules, official SDKs from a matching toolchain bundle, and local source files. Dependencies declared in `rinzu.toml` require package input. Pass arguments with `rinzu run ./hello -- argument` and read them with `rinzu.process.arguments()`. See [CLI program arguments](../tooling/cli.md#program-arguments).

Next: [Source structure and syntax](../language/syntax.md).
