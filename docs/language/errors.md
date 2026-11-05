# Errors and recovery

Ordinary failure uses Error. An API returns its successful value directly and throws on failure. Normal absence uses Option. Error effects are inferred by default. Functions can declare a public upper bound with `throws` or `nothrow`.

## Declare and throw an Error

```rinzu-test
error InvalidCount {
    payload: Int
}

fn requireCount(count: Int) -> Int {
    if count < 0 {
        throw InvalidCount(message: "count must be nonnegative", payload: count);
    }
    return count;
}

fn main() {
    try {
        let count = requireCount(-1);
        panic("expected InvalidCount");
    }
    catch [problem: InvalidCount] {
        if problem.payload != -1 {
            panic("wrong payload");
        }
    }
}
```

An `error` declaration creates a nominal Error type. A constructor requires `message`. The declaration name supplies `name`. `payload: T` is required; `payload?: T` is optional and reads as `Option<T>`; an empty body has no payload. Generic Error constructors and catch patterns specify concrete type arguments.

Error retains its concrete type when stored as Error. `name`, `message`, and payload bindings are read-only. A class payload still refers to its original instance. A payload cannot contain a Future, directly or indirectly.

## Declare an Error upper bound

Place the contract after the result and any callback `fallback`, before the body:

```rinzu-test
error ReadFailed {}

fn read() -> String throws ReadFailed {
    return "ready";
}

fn recover() -> String nothrow {
    try {
        return read();
    }
    catch problems {
        return "unavailable";
    }
}

fn main() nothrow {
    let text = recover();
}
```

`throws ReadFailed` permits escaping ReadFailed values; it does not require a failure path. Callers use the declared bound even when the current body always succeeds. They must recover the possible Error, propagate it through their own `throws` contract, or let inference propagate it. Local try/catch is optional; leaking an Error from main, a Worker, or a native callback remains forbidden.

Use `throws First, Second` for several concrete Error types, `throws Error` for any ordinary Error, and `nothrow` for an empty bound. The compiler checks the body, catch handlers and finally against the bound. Duplicate types, redundant `Error` plus concrete types, non-Error types and bare type parameters are rejected. Generic nominal Errors such as `Failure<T>` are allowed. A declared set does not promise an exact failure count or ordering: typed catches generally need a rest pattern or an exhaustive final binding.

Named functions, methods and `fn` expressions support these contracts. Omitting the annotation retains inference. Interface requirements have no body and must explicitly declare `throws` or `nothrow`. Function types have no effect annotation; function values retain their checked target and capture provenance.

For an async function, the bound describes the ordinary Errors observed from that task's body and cleanup. Argument evaluation occurs before the call, and returned or captured Futures retain their separate observation responsibilities. A nothrow factory can return a failing Future. `nothrow` does not cover panic, cancellation, traps, OOM or fatal native faults.

## Catch an entire failure list

`catch problems` binds a nonempty `List<Error>`. Typed list patterns match the entire ordered list:

```rinzu
catch [first: SaveFailed] {
    /* exactly one SaveFailed */
}
catch [first: SaveFailed, ...rest] {
    /* a SaveFailed followed by any remainder */
}
catch problems {
    /* every remaining nonempty list */
}
```

Only the first matching catch runs. Catch does not dispatch each element separately. Patterns must cover possible failures; add an exhaustive binding when the full list shape is not known. An incomplete catch is rejected even if an outer catch exists. A catch or finally cannot catch its own newly raised failures.

`throw` accepts one Error, not a list. There is no dedicated rethrow statement; throw a captured element to propagate it. Main, Worker, and native exits must not leak ordinary Errors. Tests use their own failure boundary.

## Continue with try all

A normal `try` leaves its body at the first Error. `try all` continues at the next statement and collects failures in observation order.

```rinzu-test
import rinzu.list;

error Rejected {}

fn reject() {
    throw Rejected(message: "rejected");
}

fn main() {
    try all {
        reject();
        reject();
    }
    catch problems {
        if rinzu.list.length(problems) != 2 {
            panic("wrong failure count");
        }
    }
    finally {
        let cleanupCompleted = true;
    }
}
```

Failed conditions skip an if or stop a while. Assignment commits only after the right-hand side succeeds; completed side effects are not rolled back. Reading a binding whose initializer failed raises an unbound-value Error.

A return, break, or continue stops the body. Collected Errors, catch, and finally are processed before the pending control exit. Unhandled Errors take priority over a pending return. A repeated failed await of the same Future is deduplicated within one group. Independent operations remain separate failures.

## Cleanup and diagnostics

`finally` runs for normal completion, Error, and control exits. Async functions may await cleanup there. Finally cannot return, break, or continue. Additional cleanup failures follow pending failures in order.

The first throw attaches source information; rethrow preserves it. `source` is an Option of a SourceLocation with one-based file/line/column information. `cause` is an explicit Option of Error. Set `RINZU_ERROR_TRACE=1` to collect diagnostic traces; there is no public Error.trace field.

Use `panic("reason")` for an unrecoverable invariant violation. Panic bypasses ordinary catches, still follows required cleanup, and fails normal process execution. Cancellation, shutdown, arithmetic traps, fatal native failures, and OOM are not ordinary catchable Error values.

Next: [Asynchronous tasks and Workers](async.md).
