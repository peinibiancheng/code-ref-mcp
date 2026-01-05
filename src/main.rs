use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

mod indexer;
mod search;
mod symbols;

use indexer::CodeIndexer;

#[derive(Clone)]
struct ServerState {
    indexer: Arc<RwLock<CodeIndexer>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let indexer = CodeIndexer::new("./index")?;
    let state = ServerState {
        indexer: Arc::new(RwLock::new(indexer)),
    };

    info!("code-ref-mcp server started");

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = std::io::stdout();

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match serde_json::from_str::<Value>(trimmed) {
                    Ok(request) => {
                        let response = handle_request(&state, request).await;
                        let response_str = serde_json::to_string(&response)?;
                        writeln!(stdout, "{}", response_str)?;
                        stdout.flush()?;
                    }
                    Err(e) => {
                        error!("Failed to parse request: {}", e);
                        let error_response = json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32700,
                                "message": format!("Parse error: {}", e)
                            },
                            "id": null
                        });
                        let response_str = serde_json::to_string(&error_response)?;
                        writeln!(stdout, "{}", response_str)?;
                        stdout.flush()?;
                    }
                }
            }
            Err(e) => {
                error!("Error reading input: {}", e);
                break;
            }
        }
    }

    Ok(())
}

async fn handle_request(state: &ServerState, request: Value) -> Value {
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(|m| m.as_str());

    match method {
        Some("initialize") => {
            json!({
                "jsonrpc": "2.0",
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "code-ref-mcp",
                        "version": "0.1.0"
                    }
                },
                "id": id
            })
        }
        Some("tools/list") => {
            json!({
                "jsonrpc": "2.0",
                "result": {
                    "tools": [
                        {
                            "name": "search_code",
                            "description": "Search code in indexed directories. Returns snippets and file paths matching the query.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": {
                                        "type": "string",
                                        "description": "Search query for code content"
                                    }
                                },
                                "required": ["query"]
                            }
                        },
                        {
                            "name": "get_symbol",
                            "description": "Find symbol definitions (functions, classes, etc.) using Tree-sitter parsing.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Name of the symbol to find"
                                    }
                                },
                                "required": ["name"]
                            }
                        },
                        {
                            "name": "index_directory",
                            "description": "Index a local directory for code search.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Path to the directory to index"
                                    }
                                },
                                "required": ["path"]
                            }
                        }
                    ]
                },
                "id": id
            })
        }
        Some("tools/call") => {
            let params = request.get("params");
            if let Some(params) = params {
                handle_tool_call(state, params).await.unwrap_or_else(|e| {
                    json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32603,
                            "message": format!("Tool execution error: {}", e)
                        },
                        "id": id
                    })
                })
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32602,
                        "message": "Invalid params"
                    },
                    "id": id
                })
            }
        }
        Some("notifications/initialized") => {
            // Acknowledge initialization
            return json!(null);
        }
        _ => {
            json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {:?}", method)
                },
                "id": id
            })
        }
    }
}

async fn handle_tool_call(state: &ServerState, params: &Value) -> Result<Value> {
    let tool_name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match tool_name {
        "search_code" => {
            let query = arguments
                .get("query")
                .and_then(|q| q.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing query parameter"))?;

            let indexer = state.indexer.read().await;
            let results = indexer.search(query, 10)?;

            let content = if results.is_empty() {
                "No results found.".to_string()
            } else {
                let mut output = format!("Found {} results:\n\n", results.len());
                for (i, result) in results.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} (score: {:.2})\n{}\n\n",
                        i + 1,
                        result.path,
                        result.score,
                        result.snippet
                    ));
                }
                output
            };

            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": content
                    }
                ]
            }))
        }
        "get_symbol" => {
            let name = arguments
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing name parameter"))?;

            let indexer = state.indexer.read().await;
            let symbols = indexer.find_symbol(name)?;

            let content = if symbols.is_empty() {
                format!("Symbol '{}' not found.", name)
            } else {
                let mut output = format!("Found {} definition(s) for '{}':\n\n", symbols.len(), name);
                for (i, sym) in symbols.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} in {} (line {})\n   Type: {}\n\n",
                        i + 1, sym.name, sym.path, sym.line, sym.kind
                    ));
                }
                output
            };

            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": content
                    }
                ]
            }))
        }
        "index_directory" => {
            let path = arguments
                .get("path")
                .and_then(|p| p.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

            let mut indexer = state.indexer.write().await;
            let stats = indexer.index_directory(path).await?;

            let content = format!(
                "Successfully indexed directory: {}\nFiles indexed: {}\nSymbols found: {}",
                path, stats.files_indexed, stats.symbols_found
            );

            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": content
                    }
                ]
            }))
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", tool_name)),
    }
}
