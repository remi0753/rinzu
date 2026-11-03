# Classes and methods

A class has nominal identity and reference semantics within a Domain. Its constructor takes every data field once, using named arguments. Fields marked `const` cannot be reassigned.

```rinzu-test
class Counter {
    const label: String,
    value: Int

    fn increase(self: Counter, amount: Int) {
        self.value = self.value + amount;
    }

    fn create() -> Counter {
        return Counter(label: "items", value: 0);
    }

    private fn reset(self: Counter) {
        self.value = 0;
    }
}

fn main() {
    let counter = Counter.create();
    let alias = counter;
    alias.increase(2);
    if counter.value != 2 {
        panic("instance was not shared");
    }
}
```

Separate fields with commas. A comma before the first method is optional. An instance method starts with `self: Owner`; a static method has no self parameter. Functions inside a class follow ordinary result, async, and Error rules.

## Visibility

Exporting a class exposes its fields and non-private methods. Write `export Counter;` at module scope. Individual method exports such as `export Counter.increase;` are rejected.

A `private fn` or `private async fn` is callable only from methods of the same type. A free function in the same module does not gain access. Field privacy is unavailable: use an API design that does not require hidden mutable fields.

## Const fields and value capabilities

`const` protects the field binding. If it refers to another mutable class, that class's own mutable fields remain mutable.

When every field is const and the entire field graph qualifies, the compiler derives Equality, Key, and Transferable independently. Const classes can then compare by content, serve as map keys, or be copied to a Worker. Their methods do not participate in these checks. Mutable class graphs and function fields prevent the relevant derivations.

Worker transfer creates a snapshot in another Domain; it does not share the original class instance. Inheritance, custom constructors, user finalizers, and generic methods are unavailable.

Native opaque handles and native records cannot declare methods. A class can wrap such a handle and provide the operations around it.

Next: [Enums and Option](enums.md).
