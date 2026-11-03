# Modules, imports, and exports

Each source file is a module. In a package, local dotted names are relative to `src/`. For standalone file input, they are relative to the explicit entry's parent directory.

```text
my-app/
  rinzu.toml
  src/
    main.rinzu
    math/
      totals.rinzu
```

`src/math/totals.rinzu`:

```rinzu
fn add(left: Int, right: Int) -> Int {
    return left + right;
}

export add;
```

`src/main.rinzu`:

```rinzu
import math.totals;

fn main() {
    if math.totals.add(20, 22) != 42 {
        panic("wrong total");
    }
}
```

Imports do not inject members into the current scope. Use the qualified module name. A local symbol is private to its module unless explicitly exported.

## Import by file path

An explicit file path requires an alias:

```rinzu
import "./math/totals.rinzu" as totals;

fn main() {
    if totals.add(20, 22) != 42 {
        panic("wrong total");
    }
}
```

Resolve paths relative to the importing file. A file path must name a `.rinzu` source and cannot contain control characters. Dotted imports do not accept aliases. The alias `rinzu` is reserved.

Package imports must remain within the package's canonical `src/` directory. Standalone file graphs may use relative paths outside the entry directory, including `..`, or absolute file paths. Import cycles and conflicting aliases are rejected.

The same canonical file keeps the same identity through multiple aliases or dotted/path imports. Distinct files with identical text still declare distinct nominal types. Only the chosen entry's main is the program entry.

## Public APIs and re-exports

Use `export name;` for functions, types, aliases, and constants. Types appearing in public signatures must be exported too. Exporting a class or enum also exposes its non-private methods; individual method exports are rejected.

Re-export a public imported symbol with `export totals.add;`. Re-exports preserve the original function/type identity. An SDK's `[exports] modules` determines which modules are visible outside the package. An exported symbol from a private SDK module remains usable only within the package.

Next: [Manifests and dependencies](manifest.md).
