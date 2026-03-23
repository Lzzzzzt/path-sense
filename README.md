# Path Sense

Zed extension for filename and path completion.

`path-sense-lsp` uses `tree-sitter` as the shared syntax layer for JS/TS/TSX, Python, Rust, Go, Nix, TOML, YAML, JSON, and Shell, then applies small language-specific node rules for path contexts. It supports quoted paths, Nix path expressions, Shell bare paths, `~` home completion, filesystem-root `/` completion by default on Linux/macOS, synthetic `..` folder entries, and conditional `path_mappings`.

Directory completions use a configurable `directory_suffix`, which defaults to `/`. When the suffix contains `/`, folder items also attach a best-effort `editor::ShowCompletions` command so Zed can reopen completion immediately after accepting a folder item. If the client ignores that command, the behavior safely degrades to inserting the configured suffix and waiting for the next manual or trigger-character completion.

## Development

1. Build the sidecar with `cargo build -p path-sense-lsp`.
1. Build the Zed extension with `cargo build --target wasm32-wasip2`.
1. In Zed, use `Extensions: Install Dev Extension` and point it at this worktree.
1. Zed will resolve the sidecar in this order: `lsp.path-sense.binary.path`, `<worktree>/target/debug/path-sense-lsp`, then `PATH`.

## Benchmark

Run the local performance suite with:

```bash
cargo bench -p path-sense-lsp
```

Criterion baselines can be compared with:

```bash
cargo bench -p path-sense-lsp -- --save-baseline before
cargo bench -p path-sense-lsp -- --baseline before
```

The benchmark suite covers compiled settings construction, syntax-context extraction, repeated completion on an open document, incremental `didChange` updates, and an end-to-end LSP round trip.

## Local override

You can override the language server binary in Zed settings:

```json
{
  "lsp": {
    "path-sense": {
      "binary": {
        "path": "/absolute/path/to/path-sense-lsp",
        "arguments": [],
        "env": {}
      }
    }
  }
}
```

You can also configure how directory completions are inserted:

```json
{
  "lsp": {
    "path-sense": {
      "settings": {
        "directory_suffix": ""
      }
    }
  }
}
```

Path semantics and filtering can also be configured:

```json
{
  "lsp": {
    "path-sense": {
      "settings": {
        "slash_root": "workspace",
        "min_auto_trigger_word_length": 2,
        "path_mappings": {
          "@assets": "${workspace}/assets",
          "/test": {
            "conditions": [
              {
                "when": "src/**",
                "value": ["${workspace}/src/test", "${home}/tmp/test"]
              }
            ]
          }
        },
        "trigger_outside_strings": true,
        "path_separators": " \t({[",
        "disable_up_one_folder": false,
        "ignored_files_patterns": ["vendor/**"],
        "ignored_prefixes": ["http://", "https://"]
      }
    }
  }
}
```

Notes:

- `slash_root` defaults to `"filesystem"`. Set it to `"workspace"` to resolve `/...` relative to the current workspace root.
- `min_auto_trigger_word_length` defaults to `2` and only applies to automatically triggered completions. Manual completion requests are not gated.
- Automatic path completion is triggered on path punctuation and common filename word characters (`a-z`, `A-Z`, `0-9`, `_`, `-`), so `min_auto_trigger_word_length` also applies to plain filename fragments.
- `path_mappings` supports a single string, an array of strings, or conditional mappings with `when`.
- Punctuation-prefixed `path_mappings` keys such as `@assets` or `$lib` also contribute their leading character to the LSP trigger set, so typing that prefix can reopen completion for alias roots.
- Supported mapping variables are `${home}`, `${workspace}`, `${folder}`, `${fileDirname}`, and `${relativeFileDirname}`.
- `trigger_outside_strings` is off by default. When enabled, Path Sense can also offer completions from conservative lexical fallback contexts in supported languages.
- `disable_up_one_folder = false` keeps the synthetic `..` entry enabled.
- Plain string tokens without `/` are resolved against the workspace root rather than the current file directory.

If you override `languages.<Language>.language_servers` in Zed, that list replaces the default extension-provided servers for that language. Add `path-sense` explicitly, or include `"..."` to keep the default servers.

For example, with a custom Nix setup:

```json
{
  "languages": {
    "Nix": {
      "language_servers": ["nil", "path-sense", "..."]
    }
  }
}
```
