use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Schema, Value, STORED, TEXT};
use tantivy::{doc, Index, ReloadPolicy, TantivyDocument};
use tracing::{debug, info};
use walkdir::WalkDir;

use crate::search::SearchResult;
use crate::symbols::{extract_symbols, Symbol};

pub struct CodeIndexer {
    index: Index,
    schema: Schema,
    #[allow(dead_code)]
    index_path: PathBuf,
}

pub struct IndexStats {
    pub files_indexed: usize,
    pub symbols_found: usize,
}

impl CodeIndexer {
    pub fn new<P: AsRef<Path>>(index_path: P) -> Result<Self> {
        let index_path = index_path.as_ref().to_path_buf();
        
        // Create schema
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("path", TEXT | STORED);
        schema_builder.add_text_field("content", TEXT | STORED);
        schema_builder.add_text_field("language", TEXT | STORED);
        let schema = schema_builder.build();

        // Create or open index
        let index = if index_path.exists() {
            Index::open_in_dir(&index_path)?
        } else {
            fs::create_dir_all(&index_path)?;
            Index::create_in_dir(&index_path, schema.clone())?
        };

        Ok(Self {
            index,
            schema,
            index_path,
        })
    }

    pub async fn index_directory<P: AsRef<Path>>(&mut self, dir_path: P) -> Result<IndexStats> {
        let dir_path = dir_path.as_ref();
        info!("Indexing directory: {:?}", dir_path);

        let mut index_writer = self.index.writer(50_000_000)?; // 50MB buffer
        let mut files_indexed = 0;
        let mut symbols_found = 0;

        let path_field = self.schema.get_field("path").unwrap();
        let content_field = self.schema.get_field("content").unwrap();
        let language_field = self.schema.get_field("language").unwrap();

        for entry in WalkDir::new(dir_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
        {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            if let Some(lang) = detect_language(path) {
                match fs::read_to_string(path) {
                    Ok(content) => {
                        // Index file content
                        let doc = doc!(
                            path_field => path.to_string_lossy().to_string(),
                            content_field => content.clone(),
                            language_field => lang.to_string(),
                        );
                        index_writer.add_document(doc)?;
                        files_indexed += 1;

                        // Extract symbols
                        let symbols = extract_symbols(&content, lang, path);
                        symbols_found += symbols.len();

                        debug!("Indexed: {:?} ({} symbols)", path, symbols.len());
                    }
                    Err(e) => {
                        debug!("Failed to read {:?}: {}", path, e);
                    }
                }
            }
        }

        index_writer.commit()?;
        info!(
            "Indexing complete: {} files, {} symbols",
            files_indexed, symbols_found
        );

        Ok(IndexStats {
            files_indexed,
            symbols_found,
        })
    }

    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let searcher = reader.searcher();

        let content_field = self.schema.get_field("content").unwrap();
        let path_field = self.schema.get_field("path").unwrap();

        let query_parser = QueryParser::for_index(&self.index, vec![content_field]);
        let query = query_parser.parse_query(query_str)?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            
            let path = doc
                .get_first(path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let content = doc
                .get_first(content_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Extract snippet around the match
            let snippet = extract_snippet(content, query_str, 3);

            results.push(SearchResult {
                path,
                snippet,
                score,
            });
        }

        Ok(results)
    }

    pub fn find_symbol(&self, symbol_name: &str) -> Result<Vec<Symbol>> {
        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let searcher = reader.searcher();
        
        let content_field = self.schema.get_field("content").unwrap();
        let path_field = self.schema.get_field("path").unwrap();
        let language_field = self.schema.get_field("language").unwrap();

        let query_parser = QueryParser::for_index(&self.index, vec![content_field]);
        let query = query_parser.parse_query(symbol_name)?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(50))?;

        let mut symbols = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            
            let path = doc
                .get_first(path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let content = doc
                .get_first(content_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let lang = doc
                .get_first(language_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Extract symbols from content
            let file_symbols = extract_symbols(content, lang, Path::new(path));
            
            // Filter symbols by name
            for sym in file_symbols {
                if sym.name.contains(symbol_name) {
                    symbols.push(sym);
                }
            }
        }

        Ok(symbols)
    }
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.') || s == "node_modules" || s == "target" || s == "__pycache__")
        .unwrap_or(false)
}

fn detect_language(path: &Path) -> Option<&'static str> {
    path.extension()?.to_str().and_then(|ext| match ext {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        _ => None,
    })
}

fn extract_snippet(content: &str, query: &str, context_lines: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let query_lower = query.to_lowercase();

    for (i, line) in lines.iter().enumerate() {
        if line.to_lowercase().contains(&query_lower) {
            let start = i.saturating_sub(context_lines);
            let end = (i + context_lines + 1).min(lines.len());

            let snippet_lines = &lines[start..end];
            return snippet_lines.join("\n");
        }
    }

    // If no match found, return first few lines
    lines.iter().take(5).copied().collect::<Vec<_>>().join("\n")
}
