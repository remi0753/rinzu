# Formatting, editors, and development restart

## Format source

```sh
rinzu fmt src/main.rinzu
rinzu fmt src --check
rinzu fmt src --write
rinzu fmt - < src/main.rinzu
```

Without a mode, one file or stdin is formatted to stdout. `--check` is read-only and fails when formatting would change. `--write` applies changes. Directories require one of those modes. `--write -` is invalid.

Formatting requires valid syntax but no manifest or main. Comments and literal spellings are retained, including raw-string content. Whitespace outside literals uses four-space indentation and LF. Separate declarations and functions, including class methods, with one blank line. Consecutive imports and consecutive exports stay together; leave one blank line between either group and the next declaration. The formatter aims at 120 Unicode code points per line, without splitting literal/comment tokens. Run check after formatting to validate types.

### Line wrapping

1. Keep a delimited group on one line if it fits at its current indentation and column. Count punctuation that stays on that line. Keep earlier groups inline when a later group or continuation point can take the break.
2. If a group needs to wrap, break after its opening delimiter, at each top-level comma, and before its closing delimiter. Indent the items by four spaces and align the closing delimiter with the line that opened the group.
3. Apply the same fit check to each item at its new indentation. If an item still needs to wrap, use its own delimiters or an expression or type continuation point, such as after `=` or `:`.

Treat a function's parameter list and its `throws(...)` list as separate groups. When both wrap, the parameter list ends on the line that opens `throws(`, and the Error list ends on the line with `-> Result` when the result fits. A break before `->` indents the continuation by four spaces.

Structural record types, literals, and updates also wrap when they have three or more items or their inline form exceeds 40 Unicode code points. The width includes both braces and the spaces inside them. Count the items directly inside each record, including the base expression of a record update as one item. Each item in a wrapped record starts on its own line. Comments and multiline literals inside a record force the wrapped form.

## Use the local VS Code extension

The repository contains a development extension at `tools/vscode-rinzu`. Build a debug driver, then launch VS Code with it:

```sh
cargo build -p rinzu-driver --bin rinzu
code --extensionDevelopmentPath="$PWD/tools/vscode-rinzu" --new-window .
```

Configure the extension in workspace settings:

```json
{
  "rinzu.executable": "/absolute/path/to/rinzu/target/debug/rinzu",
  "rinzu.workspace": "/absolute/path/to/my-app",
  "[rinzu]": {
    "editor.defaultFormatter": "rinzu-local.rinzu-source"
  }
}
```

With `rinzu.workspace` unset, the editor service selects the closest `rinzu.toml` for each open source file. Files outside a package receive single-buffer library checks.

Use `rinzu.entry` for an explicit standalone source file; it takes precedence over the package workspace. Relative workspace/entry paths resolve against the opened folder. `rinzu.executable` is one executable path, not a shell command with arguments. Reload the window after changing the service configuration.

After updating the checkout, rebuild the driver selected by `rinzu.executable` with `cargo build -p rinzu-driver --bin rinzu` and run **Developer: Reload Window**. Older drivers may report imported SDK types as unavailable because they check the source buffer without discovering its package.

The extension provides syntax highlighting for keywords, built-in types, numbers, comments, and String/Bytes literals, including raw Strings and formatted String expressions. Colors follow the selected VS Code theme. Highlighting does not require the Rinzu executable. After updating a linked local extension, run **Developer: Reload Window** to reload its grammar.

Hover shows checked function signatures, type aliases, and the types of bindings. Call hovers distinguish a declared Error bound from an inferred bound. An async call shows when the bound applies to its Future. Hovering over an await keyword shows the operand type, the result type, and the ordinary Errors that can be observed there. During an incomplete type check, hover can still show a declaration or annotation from the current buffer.

The extension also supports diagnostics for open buffers, definition/reference navigation, completion, formatting, and import organization. Diagnostics show related source locations and explanation-only suggestions under **Help**. Results for separate packages stay visible as you switch between files. **Rinzu: Refresh Diagnostics** rechecks the open buffers and their dependencies. Import organization sorts and deduplicates eligible groups without guessing which imports are unused.

Run the integration tests with an installed VS Code from a desktop session. The runner creates a temporary workspace and profile. On macOS the process must be able to register a desktop application.

```sh
cargo build -p rinzu-driver --bin rinzu
python3 tests/integration/test_vscode_editor.py
```

The optional hover method accepts the same document version and UTF-16 position as definition. Its result includes a token range, checked or syntax resolution, and any available definition and contract locations. Each checked effect gives the phase, the basis, and a none, finite, open, or unknown bound. Consecutive requests in a verified snapshot reuse a bounded index. The extension sends hover requests to a separate process so type checking and Error inference can answer while diagnostics verify Future observation. Protocol clients can also prepare the hover index during a full diagnostics request with `prepareHover: true`.

This is a local development extension, not an automatically installed Marketplace package. The editor service is a versioned JSON-lines protocol; it is not a claim of full LSP compatibility. It supports one folder/service and file documents. Native provider/header/receipt validation still requires CLI check/build with actual inputs.

## Restart during development

```sh
rinzu dev ./my-app
```

The default watcher restarts the process after edits and may stop the old process forcibly. For applications that save or close asynchronously, register cooperative signals through `rinzu.shutdown` or the macOS application's signal-aware entry, then use:

```sh
rinzu dev ./my-app --restart cooperative --stdin null --shutdown-timeout-ms 5000
```

Cooperative restart waits for shutdown acknowledgment and cleanup before launching a fresh process. The default timeout preserves the old run in a blocked state. `--on-timeout force` explicitly permits killing it; that does not mean its save completed. Invalid source edits keep the previous run available when possible.

Watchers follow declared source/native inputs and package settings. They do not guarantee observation of undeclared C includes or every environment change. Restart creates a new process; live object/closure migration is unavailable.
