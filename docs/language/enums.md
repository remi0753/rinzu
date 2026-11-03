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

Next: [Generics and markers](generics.md).
