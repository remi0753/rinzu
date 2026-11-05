# Enums and Option

An enum declares a closed set of named variants. A variant can carry typed payload fields. Construct variants with the enum name and named payload arguments, then use an exhaustive switch to inspect them.

```rinzu-test
enum Reading {
    Missing,
    Value(number: Int)
}

fn numberOrZero(reading: Reading) -> Int {
    switch reading {
        case Reading.Missing: {
            return 0;
        }
        case Reading.Value(number): {
            return number;
        }
    }
}

fn main() {
    let reading = Reading.Value(number: 42);
    if numberOrZero(reading) != 42 {
        panic("invalid reading");
    }
}
```

Even a payload-free variant is constructed with parentheses, as in `Reading.Missing()`. The case pattern uses `case Reading.Missing:`. Payload names are local to the selected case.

## Represent absence with Option

`Option<T>` is available without an import:

```rinzu-test
fn defaultName(name: Option<String>) -> String {
    switch name {
        case Option.Some(value): {
            return value;
        }
        case Option.None: {
            return "guest";
        }
    }
}

fn main() {
    let present = Option<String>.Some(value: "Ada");
    let absent = Option<String>.None();
    if defaultName(present) != "Ada" || defaultName(absent) != "guest" {
        panic("invalid option");
    }
}
```

Lookup and slicing APIs return Option for ordinary absence or an invalid index. A failure such as malformed UTF-8 throws Error instead. There is no standard Result type; APIs return successful values directly and propagate ordinary failures through Error.

Enums can be generic, can contain methods, and derive capabilities from all variant payloads. Export and private-method rules match [classes](classes.md).

An enum can declare [interface conformance](interfaces.md) with `enum Entry<T> interface I + J`. The requirements apply to the whole enum; implement variant-specific behavior with an exhaustive switch in an ordinary enum method.

## Option helpers

Import `rinzu.option` to apply a transformation or compute a default. Use `switch` when the two cases need their own logic.

| Function | Result |
| --- | --- |
| `rinzu.option.map(Option<T>, fn (T) -> U)` | Wraps the transformed value in `Some`. Returns `None` on absent input |
| `rinzu.option.andThen(Option<T>, fn (T) -> Option<U>)` | Returns the result of the second lookup directly. Returns `None` on absent input |
| `rinzu.option.getOrElse(Option<T>, fn () -> T)` | Returns the value in `Some` or computes a default for `None` |

Arguments are evaluated normally. `map` and `andThen` invoke the callback once for `Some`; `getOrElse` invokes it once for `None`. Callbacks run synchronously and their Errors propagate to the caller.

```rinzu-test
import rinzu.map;
import rinzu.option;

fn defaultName() -> String {
    return "guest";
}

fn displayName(settings: Map<String, String>) -> String {
    return rinzu.option.getOrElse(rinzu.map.get(settings, "name"), defaultName);
}

fn main() {
    if displayName({"name": "Ada"}) != "Ada" || displayName({}) != "guest" {
        panic("unexpected display name");
    }
}
```

Next: [Generics and constraints](generics.md).
