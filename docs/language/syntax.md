# Source structure and syntax

Source files use UTF-8 and the `.rinzu` extension. Place imports first, followed by functions, types, constants, exports, and native declarations. File names and directories determine modules; no `module` declaration is needed.

```rinzu-test
import rinzu.stdio;

// Add two numbers.
fn add(left: Int, right: Int) -> Int {
    return left + right;
}

fn main() {
    try {
        let answer = add(20, 22);
        rinzu.stdio.writeLine(answer);
    }
    catch problems {
        panic("output failed");
    }
}
```

Statements end with `;`. Braces delimit blocks. Conditions in `if` and `while` do not need parentheses. Write parameter and field types as `name: Type`, and function results as `-> Type`. Executable statements belong inside functions.

## Names and comments

Identifiers use ASCII letters, digits, and `_`, and cannot begin with a digit. String contents can contain Unicode. Local module path segments use lowercase letters, digits, and `_`.

Use `//` for a line comment and `/* ... */` for a block comment. A `///` comment immediately before a public declaration can become [API documentation](../packages/authoring.md).

## Literals

| Example | Meaning |
| --- | --- |
| `true`, `false` | Bool |
| `42`, `0xff`, `0b1010`, `1_000` | Integer |
| `1.5` | Floating-point value |
| `"hello"`, `f"value={value}"` | String and formatted string |
| `r#"raw text"#` | Raw string |
| `b"\x00\xff"` | Fixed-size bytes |
| `[1, 2, 3]` | List |
| `{"answer": 42}` | Map |
| `{x: Int = 1, y: Int = 2}` | Record |

Maps use `key: value`; records use `field = value`, with an optional field type before `=`. Add a type annotation when the context cannot determine an empty collection or numeric width.

Next: [Variables and constants](variables.md).
