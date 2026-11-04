# Functions and closures

Declare parameter types explicitly. A result type is optional for Unit functions; other functions declare it with `-> Type` and use explicit `return` statements.

```rinzu-test
fn apply(operation: fn (Int) -> Int, value: Int) -> Int {
    return operation(value);
}

fn main() {
    var base = 40;
    let addBase = fn (value: Int) -> Int {
        return base + value;
    };
    base = 100;
    if apply(addBase, 2) != 42 {
        panic("wrong capture");
    }
}
```

All arguments are required. Ordinary calls use positional arguments. Class and enum constructors use named arguments. Overloads, default arguments and partial application are unavailable. Methods can have generic owner and method parameters.

A function or function expression can put `throws E1, E2` or `nothrow` after its result and optional `fallback`. Omission keeps inference. See [Error contracts](errors.md#declare-an-error-upper-bound).

## Capture values at creation

A `fn` expression creates a function value. It captures only the free variables it uses, when the expression is evaluated. Captured scalar and immutable values are snapshots; a captured class refers to the same instance in the same Domain. The example captures `40`, even though `base` changes later.

A closure cannot reassign an outer binding, but it can update mutable fields of a captured class. Locals declared inside the closure follow ordinary `let` and `var` rules.

An `async fn` expression creates an asynchronous function value. Calling it queues a task and returns a Future; merely creating the function value does not execute its body.

```rinzu
let load = async fn () -> Int {
    return 42;
};

let pending = load();

let answer = await pending;
// inside an async function
```

## Function values and boundaries

A named function can also be used where its function type is expected. `Type.method` includes an explicit `self` parameter for an instance method. To retain a particular receiver, wrap a call in a closure; `receiver.method` alone does not create a bound method.

Function values can be local values and class fields, subject to capture and effect checks. They do not have Equality, Key, or Transferable. Function values can be List elements, Map values and Future results within one Domain. Their code and capture environment remain in the typed GC graph. Storing or returning a captured Future does not observe it, and awaiting an outer Future does not observe Futures held by its returned function. Worker calls and native callbacks impose additional restrictions on effects and captured values.

A same-Domain managed closure can capture a Future, but its creation alone does not discharge responsibility for observing that Future. Future capture across the native ABI is prohibited.

Next: [Records and type aliases](records.md).
