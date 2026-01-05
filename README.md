# code-ref-mcp

Rust-based MCP server that indexes local code using Tantivy and Tree-sitter. It exposes two MCP tools:

- `search_code(query, limit?)` — multi-threaded full-text search for `.rs`, `.py`, and `.js` files, returning file paths, line numbers, and snippets.
- `get_symbol(name)` — locate symbol definitions parsed with Tree-sitter.

## Running

```bash
export CODE_REF_ROOT=/path/to/code  # defaults to current directory
cargo run --release
```

The server communicates over stdio via the MCP protocol. Use the `tools/list` and `tools/call` methods to drive the available tools.
