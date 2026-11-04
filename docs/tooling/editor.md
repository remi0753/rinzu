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

Use `rinzu.entry` for an explicit standalone source file; it takes precedence over the package workspace. Relative workspace/entry paths resolve against the opened folder. `rinzu.executable` is one executable path, not a shell command with arguments. Reload the window after changing the service configuration.

The extension provides syntax highlighting for keywords, built-in types, numbers, comments, and String/Bytes literals, including raw Strings and formatted String expressions. Colors follow the selected VS Code theme. Highlighting does not require the Rinzu executable. After updating a linked local extension, run **Developer: Reload Window** to reload its grammar.

The extension also supports diagnostics for open buffers, definition/reference navigation, completion, formatting, and import organization. Use **Rinzu: Refresh Diagnostics** after external SDK changes. Import organization sorts/deduplicates eligible groups without guessing which imports are unused. It does not provide complete member completion or general recovery from arbitrary invalid syntax.

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
