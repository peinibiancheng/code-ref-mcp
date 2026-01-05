# code-ref-mcp

High-performance Rust MCP server for indexing local code. It uses Tantivy for instant full-text search and pattern-based symbol parsing.

## Features

- **Fast Code Search**: Uses Tantivy for efficient full-text search across multiple file types
- **Symbol Extraction**: Finds function, class, struct, enum, and trait definitions
- **Multi-language Support**: Supports Rust (.rs), Python (.py), and JavaScript (.js) files
- **MCP Protocol**: Implements the Model Context Protocol for tool integration
- **Async Performance**: Built with Tokio for high-performance async operations
- **Low Memory Footprint**: Optimized for <50MB RAM usage

## Supported Languages

- **Rust**: Functions, structs, enums, traits
- **Python**: Functions, classes
- **JavaScript**: Functions, classes, const arrow functions

## Installation

### Prerequisites

- Rust 1.70 or later
- Cargo

### Build

```bash
cargo build --release
```

The binary will be located at `./target/release/code-ref-mcp`

## Usage

The server implements the MCP (Model Context Protocol) and communicates via JSON-RPC over stdin/stdout.

### Available Tools

#### 1. `index_directory`

Index a local directory for code search.

**Parameters:**
- `path` (string): Path to the directory to index

**Example:**
```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "index_directory",
    "arguments": {
      "path": "./src"
    }
  },
  "id": 1
}
```

#### 2. `search_code`

Search code in indexed directories. Returns snippets and file paths matching the query.

**Parameters:**
- `query` (string): Search query for code content

**Example:**
```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "search_code",
    "arguments": {
      "query": "async function"
    }
  },
  "id": 2
}
```

#### 3. `get_symbol`

Find symbol definitions (functions, classes, etc.) across indexed files.

**Parameters:**
- `name` (string): Name of the symbol to find

**Example:**
```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "get_symbol",
    "arguments": {
      "name": "MyClass"
    }
  },
  "id": 3
}
```

## Running the Server

Start the server:

```bash
./target/release/code-ref-mcp
```

The server will listen for JSON-RPC requests on stdin and write responses to stdout.

## Architecture

- **main.rs**: MCP server implementation with JSON-RPC handling
- **indexer.rs**: Tantivy-based code indexing and search
- **symbols.rs**: Symbol extraction using pattern matching
- **search.rs**: Search result data structures

## Performance

- Async indexing using Tokio
- Optimized for <50MB RAM usage
- Fast search with Tantivy inverted index
- Efficient symbol extraction with pattern matching

## Development

Run in development mode:

```bash
cargo run
```

Run tests:

```bash
cargo test
```

Build optimized release:

```bash
cargo build --release
```

## License

MIT
