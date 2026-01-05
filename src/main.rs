use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use mcp_rust_sdk::{
    error::{Error, ErrorCode},
    server::{Server, ServerHandler},
    transport::stdio::StdioTransport,
    types::{ClientCapabilities, Implementation, ServerCapabilities, Tool},
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tantivy::{
    collector::TopDocs,
    query::QueryParser,
    schema::{Field, OwnedValue, Schema, STORED, STRING, TEXT},
    Index, IndexReader, TantivyDocument,
};
use tokio::sync::RwLock;
use tree_sitter::Parser;
use walkdir::WalkDir;

const DEFAULT_LINE_NUMBER: usize = 1;
const CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Serialize)]
struct SearchResult {
    path: String,
    line: usize,
    snippet: String,
}

#[derive(Debug, Clone, Serialize)]
struct SymbolInfo {
    name: String,
    path: String,
    line: usize,
    kind: String,
}

struct SnippetCache {
    entries: HashMap<String, Arc<String>>,
    order: VecDeque<String>,
    capacity: usize,
}

impl SnippetCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn get_or_load(&mut self, path: &str) -> Result<Arc<String>, Error> {
        if let Some(contents) = self.entries.get(path) {
            return Ok(contents.clone());
        }

        let contents = fs::read_to_string(path).map_err(|e| Error::Other(e.to_string()))?;
        let contents = Arc::new(contents);
        self.insert(path.to_string(), contents.clone());
        Ok(contents)
    }

    fn insert(&mut self, path: String, contents: Arc<String>) {
        if self.entries.contains_key(&path) {
            return;
        }
        self.entries.insert(path.clone(), contents);
        self.order.push_back(path);
        if self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.entries.remove(&old);
            }
        }
    }
}

struct IndexState {
    index: Index,
    reader: IndexReader,
    path_field: Field,
    body_field: Field,
    symbols: Arc<RwLock<HashMap<String, Vec<SymbolInfo>>>>,
    snippet_cache: Mutex<SnippetCache>,
}

impl IndexState {
    fn build(root: PathBuf) -> Result<Self, Error> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STRING | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        let thread_count = std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(2)
            .max(2);
        let mut writer = index
            .writer_with_num_threads(thread_count, 30_000_000)
            .map_err(|e| Error::Other(e.to_string()))?;

        let files: Vec<PathBuf> = WalkDir::new(&root)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .collect();

        let documents: Vec<(TantivyDocument, Vec<SymbolInfo>)> = files
            .par_iter()
            .with_max_len(32)
            .filter_map(|path| {
                let language = language_for(path)?;
                let contents = fs::read_to_string(path).ok()?;

                let mut doc = TantivyDocument::default();
                doc.add_text(path_field, path.display().to_string());
                doc.add_text(body_field, &contents);

                let symbols = extract_symbols(path, language, &contents);
                Some((doc, symbols))
            })
            .collect();

        let mut symbol_map: HashMap<String, Vec<SymbolInfo>> = HashMap::new();
        for (document, symbols) in documents {
            writer
                .add_document(document)
                .map_err(|e| Error::Other(e.to_string()))?;
            for symbol in symbols {
                symbol_map
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(symbol);
            }
        }

        writer.commit().map_err(|e| Error::Other(e.to_string()))?;

        let reader = index.reader().map_err(|e| Error::Other(e.to_string()))?;

        Ok(Self {
            index,
            reader,
            path_field,
            body_field,
            symbols: Arc::new(RwLock::new(symbol_map)),
            snippet_cache: Mutex::new(SnippetCache::new(CACHE_CAPACITY)),
        })
    }

    fn search(&self, query_text: &str, limit: usize) -> Result<Vec<SearchResult>, Error> {
        if query_text.trim().is_empty() {
            return Err(Error::protocol(
                ErrorCode::InvalidParams,
                "query cannot be empty",
            ));
        }

        let searcher = self.reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.body_field]);
        let parsed = parser
            .parse_query(query_text)
            .map_err(|e| Error::Other(e.to_string()))?;

        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit))
            .map_err(|e| Error::Other(e.to_string()))?;

        let mut results = Vec::new();
        for (_score, doc_address) in top_docs {
            let retrieved: TantivyDocument = searcher
                .doc::<TantivyDocument>(doc_address)
                .map_err(|e| Error::Other(e.to_string()))?;
            if let Some(path_value) = retrieved.get_first(self.path_field) {
                let owned: OwnedValue = path_value.into();
                if let OwnedValue::Str(path) = owned {
                    let contents = {
                        let mut cache = self
                            .snippet_cache
                            .lock()
                            .map_err(|_| Error::Other("snippet cache poisoned".to_string()))?;
                        cache.get_or_load(&path)
                    };
                    let contents = match contents {
                        Ok(content) => content,
                        Err(_) => continue,
                    };
                    let (line, snippet) = snippet_for_query(&contents, query_text);
                    results.push(SearchResult {
                        path,
                        line,
                        snippet,
                    });
                }
            }
        }

        Ok(results)
    }

    async fn symbols(&self, name: &str) -> Vec<SymbolInfo> {
        let symbols = self.symbols.read().await;
        symbols.get(name).cloned().unwrap_or_default()
    }
}

fn language_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
    {
        Some(ext) if ext == "rs" => Some("rust"),
        Some(ext) if ext == "py" => Some("python"),
        Some(ext) if ext == "js" => Some("javascript"),
        _ => None,
    }
}

fn extract_symbols(path: &Path, language: &str, source: &str) -> Vec<SymbolInfo> {
    let mut parser = Parser::new();
    let language_fn = match language {
        "rust" => tree_sitter_rust::LANGUAGE,
        "python" => tree_sitter_python::LANGUAGE,
        "javascript" => tree_sitter_javascript::LANGUAGE,
        _ => return Vec::new(),
    };

    if parser.set_language(&language_fn.into()).is_err() {
        return Vec::new();
    }

    let tree = match parser.parse(source, None) {
        Some(tree) => tree,
        None => return Vec::new(),
    };

    let mut symbols = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if let Some(kind) = symbol_kind(language, node.kind()) {
            if let Some(name_node) = node.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
                    symbols.push(SymbolInfo {
                        name: name.to_string(),
                        path: path.display().to_string(),
                        line: node.start_position().row + 1,
                        kind: kind.to_string(),
                    });
                }
            }
        }

        for idx in 0..node.child_count() {
            if let Some(child) = node.child(idx) {
                stack.push(child);
            }
        }
    }

    symbols
}

fn symbol_kind(language: &str, node_kind: &str) -> Option<&'static str> {
    match (language, node_kind) {
        ("rust", "function_item") => Some("function"),
        ("rust", "struct_item") => Some("struct"),
        ("rust", "enum_item") => Some("enum"),
        ("rust", "trait_item") => Some("trait"),
        ("python", "function_definition") => Some("function"),
        ("python", "class_definition") => Some("class"),
        ("javascript", "function_declaration") => Some("function"),
        ("javascript", "class_declaration") => Some("class"),
        _ => None,
    }
}

fn snippet_for_query(contents: &str, query: &str) -> (usize, String) {
    let needle = query.to_lowercase();
    for (idx, line) in contents.lines().enumerate() {
        if line.to_lowercase().contains(&needle) {
            return (idx + 1, line.trim().to_string());
        }
    }

    let fallback = contents.lines().next().unwrap_or("").trim().to_string();
    if fallback.is_empty() {
        (DEFAULT_LINE_NUMBER, "No matching line found".to_string())
    } else {
        (DEFAULT_LINE_NUMBER, fallback)
    }
}

#[derive(Clone)]
struct CodeRefHandler {
    state: Arc<IndexState>,
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

#[derive(Deserialize)]
struct SymbolArgs {
    name: String,
}

#[derive(Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[async_trait]
impl ServerHandler for CodeRefHandler {
    async fn initialize(
        &self,
        _implementation: Implementation,
        _capabilities: ClientCapabilities,
    ) -> Result<ServerCapabilities, Error> {
        let tools = self.tools();
        let mut custom = HashMap::new();
        custom.insert("tools".to_string(), json!(tools));
        Ok(ServerCapabilities {
            custom: Some(custom),
        })
    }

    async fn shutdown(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn handle_method(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Error> {
        match method {
            "tools/list" => Ok(json!({ "tools": self.tools() })),
            "tools/call" => {
                let params: ToolCallParams =
                    serde_json::from_value(params.unwrap_or_else(|| json!({})))
                        .map_err(|e| Error::protocol(ErrorCode::InvalidParams, e.to_string()))?;
                self.call_tool(&params.name, params.arguments).await
            }
            "search_code" => {
                let args: SearchArgs = serde_json::from_value(params.unwrap_or_else(|| json!({})))
                    .map_err(|e| Error::protocol(ErrorCode::InvalidParams, e.to_string()))?;
                let results = self.state.search(&args.query, args.limit)?;
                Ok(json!({ "results": results }))
            }
            "get_symbol" => {
                let args: SymbolArgs = serde_json::from_value(params.unwrap_or_else(|| json!({})))
                    .map_err(|e| Error::protocol(ErrorCode::InvalidParams, e.to_string()))?;
                let matches = self.state.symbols(&args.name).await;
                Ok(json!({ "results": matches }))
            }
            _ => Err(Error::protocol(
                ErrorCode::MethodNotFound,
                format!("Unknown method: {method}"),
            )),
        }
    }
}

impl CodeRefHandler {
    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: "search_code".to_string(),
                description: "Full-text search across indexed Rust, Python, and JavaScript files."
                    .to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "get_symbol".to_string(),
                description: "Locate symbol definitions using tree-sitter indexes.".to_string(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Symbol name to lookup" }
                    },
                    "required": ["name"]
                }),
            },
        ]
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        match name {
            "search_code" => {
                let args: SearchArgs = serde_json::from_value(arguments)
                    .map_err(|e| Error::protocol(ErrorCode::InvalidParams, e.to_string()))?;
                let results = self.state.search(&args.query, args.limit)?;
                let summary = results
                    .iter()
                    .map(|r| format!("{}:{} {}", r.path, r.line, r.snippet))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": summary
                    }],
                    "results": results
                }))
            }
            "get_symbol" => {
                let args: SymbolArgs = serde_json::from_value(arguments)
                    .map_err(|e| Error::protocol(ErrorCode::InvalidParams, e.to_string()))?;
                let matches = self.state.symbols(&args.name).await;
                let summary = matches
                    .iter()
                    .map(|m| format!("{}:{} [{}]", m.path, m.line, m.kind))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": summary
                    }],
                    "results": matches
                }))
            }
            _ => Err(Error::protocol(
                ErrorCode::MethodNotFound,
                format!("Unknown tool: {name}"),
            )),
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var_os("CODE_REF_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);

    let state = IndexState::build(root)?;
    let handler = Arc::new(CodeRefHandler {
        state: Arc::new(state),
    });

    let (transport, _receiver) = StdioTransport::new();
    let server = Server::new(Arc::new(transport), handler);
    server.start().await?;
    Ok(())
}
