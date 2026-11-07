# Interfaces

An interface declares public instance-method requirements. A nominal class or enum declares conformance alongside its definition; matching method names alone do not establish conformance.

```rinzu-test
error ReadFailed {}

interface Reader {
    fn read(self: Self) throws(ReadFailed) -> String;
}

class MemoryReader interface Reader {
    const text: String

    fn read(self: MemoryReader) nothrow -> String {
        return self.text;
    }
}

enum Input interface Reader {
    Empty,
    Text(text: String),

    fn read(self: Input) nothrow -> String {
        switch self {
            case Input.Empty: { return ""; }
            case Input.Text(text): { return text; }
        }
    }
}

fn read<R interface Reader>(reader: R) throws(ReadFailed) -> String {
    return reader.read();
}

fn main() nothrow {
    let memory = MemoryReader(text: "ready");
    let direct = memory.read();
    try {
        let fromMemory = read(memory);
        let fromEnum = read(Input.Text(text: "ready"));
        if fromMemory != fromEnum || direct != fromMemory {
            panic("reader mismatch");
        }
    }
    catch problems {
        panic("unexpected failure");
    }
}
```

The compiler checks every required method's visibility, parameter and result types, and sync/async kind. The implementation must permit no more ordinary Errors than the interface. A narrower bound, including `nothrow`, is allowed. An implementation without an annotation uses inference. Interface requirements put an explicit `throws(E1, E2)`, `throws(Error)`, or `nothrow` after the parameter list and end with a semicolon.

The direct `memory.read()` uses MemoryReader.read's nothrow contract. The constrained call in generic `read` uses Reader.read's declared ReadFailed bound even after specialization for MemoryReader. The generic implementation can recover that Error or propagate it through an explicit or inferred contract. A throws declaration never requires an actual failure path.

## Self and type parameters

`Self` denotes the complete implementing type. It can occur in the receiver, other parameters, results and nested types. For `Box<T>`, Self becomes `Box<T>`; for a Copyable Counter, `fn copy(self: Self) nothrow -> Self;` requires `fn copy(self: Counter) nothrow -> Counter`. Self is available in interface signatures, not as a new shorthand inside class definitions.

```rinzu
interface Getter<T> {
    fn get(self: Self) nothrow -> T;
}

class Box<T> interface Getter<T> {
    const value: T

    fn get(self: Box<T>) nothrow -> T {
        return self.value;
    }
}

fn unbox<T, B interface Getter<T>>(box: B, otherwise: T) nothrow -> T {
    return box.get();
}
```

Use `+` for multiple conformance declarations or constraints. `<T: Transferable interface Reader + Named>` combines compiler-owned markers with interface requirements. Interface arguments may have their own marker and interface constraints, checked when concrete arguments are selected. Generic templates retain the existing specialization checks and limits.

One method must satisfy all same-name requirements: parameter/result types and sync/async kind must agree, and its Error bound must fit every requirement. A generic call constrained by several such requirements uses their intersection. Duplicate conformance declarations, including different applications of the same interface on one owner, are rejected.

## Scope and boundaries

Only source classes and enums can declare conformance. An enum implements each requirement for the whole enum; payload conformance does not create delegation. Native opaque references and native records have no methods or interface conformance. Wrap native references in a class when behavior is needed. Builtins, structural records and transparent aliases do not introduce conformance.

Export interfaces and every nominal type or Error exposed by their signatures. Public conformance declarations and generic constraints must also name public interfaces. Imports, re-exports and transparent type aliases retain the original nominal identities. Required methods follow the ordinary owner-private access rules for their implementation bodies.

Interfaces are constraints, not value types: `value: Reader` and `List<Reader>` are unavailable. Dispatch specializes to concrete code. Default bodies, interface inheritance, interface methods with their own type parameters, associated types, dynamic dispatch and boxing are unavailable. Interface conformance grants no Equality, Key or Transferable capability and does not discharge Future, native or Worker obligations.

Next: [Lists and maps](collections.md).
