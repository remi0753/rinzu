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

Collection traversal uses `while` and collection APIs. There is no general `for` or range syntax. Read the length before a list traversal and fetch elements from the same snapshot.

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
