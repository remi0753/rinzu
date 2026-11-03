# Introduction

Rinzu combines ordinary application logic, asynchronous I/O, CPU work, and native library calls in a small execution model. Types are checked before execution. A garbage collector manages language values. JIT execution supports development, and AOT compilation produces distributable executables.

## A program starts with main

Use `fn main()` for a synchronous entry or `async fn main()` when the entry needs `await`. The entry has no parameters and returns `Unit`.

```rinzu-test
import rinzu.stdio;

fn greeting(name: String) -> String {
    return f"Hello, {name}!";
}

fn main() {
    try {
        rinzu.stdio.writeLine(greeting("Rinzu"));
    }
    catch problems {
        panic("could not write greeting");
    }
}
```

`import` selects a module, `fn` declares a function, and `return` produces its result. An `f"...{expression}..."` literal formats a value into a string. Output can fail, so the example handles its Error at the application boundary. Functions can propagate ordinary Errors, but Main, Worker, and native exits must handle them.

## Values and state

Strings, lists, maps, and language records are immutable. Updating one produces a new value. Classes hold shared state: assigning a class to another binding shares its instance within the same Domain. A `const` field cannot be reassigned.

## Choose execution explicitly

An ordinary function runs in its caller's execution context. Calling an `async` function queues a task and returns `Future<T>`; `await` observes its completion. Async does not automatically move CPU work to another thread. Use `spawn` to send a synchronous function and transferable snapshots to a Worker.

Main and Workers have separate managed heaps. A heap and its execution context form a Domain. Transferring a value to a Worker does not share Main's mutable state or native handles.

## From source to an application

Output, timers, files, HTTP, TCP, and testing support are bundled with the toolchain. Additional SDKs, including SQLite and macOS GUI integration, use local package dependencies.

The usual workflow is to write source, run `check`, execute with `run`, add tests, and produce a binary with `build`. Start with one file and split it into modules or packages as the program grows.

Continue with [Installation](getting-started/installation.md). See [Supported platforms and boundaries](reference/support.md) for the exact execution scope.
