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
rinzu test ./hello
rinzu test ./hello --unit
rinzu build ./hello --output ./hello-app
```

Plain `test` runs the package's `main`. `--unit` discovers `test_*` functions. See [Testing](../tooling/testing.md) to add cases.

## File input and package input

| Input | Configuration |
| --- | --- |
| `rinzu run hello.rinzu` | Uses the explicit file as entry; does not read nearby manifests or locks |
| `rinzu run ./hello` | Uses the package manifest, entry, dependencies, and existing lock |

A standalone file can import bundled libraries and local source files. Additional SDK dependencies require package input. Programs do not accept additional command-line arguments; use environment settings through [rinzu.process](../libraries/overview.md).

Next: [Source structure and syntax](../language/syntax.md).
