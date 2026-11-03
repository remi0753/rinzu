# Control flow

`if` requires a Bool condition. `while` repeats its block while the condition is true. Use `break` to leave a loop and `continue` to start its next condition check.

```rinzu-test
fn sumThrough(limit: Int) -> Int {
    var total = 0;
    var index = 1;
    while index <= limit {
        total = total + index;
        index = index + 1;
    }
    return total;
}

fn main() {
    if sumThrough(4) == 10 {
        return;
    }
    else {
        panic("wrong sum");
    }
}
```

## Iterate a List snapshot

`for value in values { ... }` evaluates the List once and visits its elements in increasing index order. Each iteration has a fresh, read-only binding. Reassigning the source variable does not change the snapshot, while class elements retain their ordinary shared reference semantics. `_` discards a binding; it does not observe a Future.

Continue advances to the next element; break exits the innermost loop. Return and loop exits run finally as usual. Async bodies can await. Complete traversal can observe all Future elements; an early break or a conditional skip must not lose responsibility. The first supported iterable is List; ranges, Map, strings, user iterators and for-await are unavailable.

## Switch over variants

`switch` covers the variants of an enum. A case can bind its payload, and cases do not fall through.

```rinzu-test
fn valueOrZero(value: Option<Int>) -> Int {
    switch value {
        case Option.Some(number): {
            return number;
        }
        case Option.None: {
            return 0;
        }
    }
}

fn main() {
    if valueOrZero(Option<Int>.Some(value: 42)) != 42 {
        panic("wrong option");
    }
}
```

A non-Unit function must explicitly return a value on every normal exit. Control exits still execute required `finally` blocks. A `finally` block cannot contain `return`, `break`, or `continue`. See [Errors and recovery](errors.md).

Next: [Functions and closures](functions.md).
