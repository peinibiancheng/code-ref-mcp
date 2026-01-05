use std::path::Path;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
}

pub fn extract_symbols(content: &str, language: &str, file_path: &Path) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let path_str = file_path.to_string_lossy().to_string();

    match language {
        "rust" => symbols.extend(extract_rust_symbols(content, &path_str)),
        "python" => symbols.extend(extract_python_symbols(content, &path_str)),
        "javascript" => symbols.extend(extract_javascript_symbols(content, &path_str)),
        _ => {}
    }

    symbols
}

fn extract_rust_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        
        if let Some(fn_name) = parse_rust_function(trimmed) {
            symbols.push(Symbol {
                name: fn_name,
                kind: "function".to_string(),
                path: path.to_string(),
                line: line_num + 1,
            });
        } else if let Some(struct_name) = parse_rust_struct(trimmed) {
            symbols.push(Symbol {
                name: struct_name,
                kind: "struct".to_string(),
                path: path.to_string(),
                line: line_num + 1,
            });
        } else if let Some(enum_name) = parse_rust_enum(trimmed) {
            symbols.push(Symbol {
                name: enum_name,
                kind: "enum".to_string(),
                path: path.to_string(),
                line: line_num + 1,
            });
        } else if let Some(trait_name) = parse_rust_trait(trimmed) {
            symbols.push(Symbol {
                name: trait_name,
                kind: "trait".to_string(),
                path: path.to_string(),
                line: line_num + 1,
            });
        }
    }
    
    symbols
}

fn extract_python_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("def ") {
            if let Some(name) = extract_name_after(trimmed, "def ", '(') {
                symbols.push(Symbol {
                    name,
                    kind: "function".to_string(),
                    path: path.to_string(),
                    line: line_num + 1,
                });
            }
        } else if trimmed.starts_with("class ") {
            if let Some(name) = extract_name_after(trimmed, "class ", ':') {
                // Remove any parentheses for inheritance
                let name = name.split('(').next().unwrap_or(&name).trim().to_string();
                symbols.push(Symbol {
                    name,
                    kind: "class".to_string(),
                    path: path.to_string(),
                    line: line_num + 1,
                });
            }
        }
    }
    
    symbols
}

fn extract_javascript_symbols(content: &str, path: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    
    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("function ") {
            if let Some(name) = extract_name_after(trimmed, "function ", '(') {
                symbols.push(Symbol {
                    name,
                    kind: "function".to_string(),
                    path: path.to_string(),
                    line: line_num + 1,
                });
            }
        } else if trimmed.starts_with("class ") {
            if let Some(name) = extract_name_after(trimmed, "class ", '{') {
                let name = name.split(|c| c == '{' || c == ' ').next().unwrap_or(&name).trim().to_string();
                symbols.push(Symbol {
                    name,
                    kind: "class".to_string(),
                    path: path.to_string(),
                    line: line_num + 1,
                });
            }
        } else if let Some(const_name) = parse_js_const_function(trimmed) {
            symbols.push(Symbol {
                name: const_name,
                kind: "function".to_string(),
                path: path.to_string(),
                line: line_num + 1,
            });
        }
    }
    
    symbols
}

fn parse_rust_function(line: &str) -> Option<String> {
    if line.starts_with("fn ") || line.starts_with("pub fn ") || line.starts_with("async fn ") || line.starts_with("pub async fn ") {
        let start = line.find("fn ")? + 3;
        let rest = &line[start..];
        let end = rest.find('(')?;
        Some(rest[..end].trim().to_string())
    } else {
        None
    }
}

fn parse_rust_struct(line: &str) -> Option<String> {
    if line.starts_with("struct ") || line.starts_with("pub struct ") {
        let start = line.find("struct ")? + 7;
        let rest = &line[start..];
        let end = rest.find(|c: char| c == '<' || c == '{' || c == ';' || c == '(' || c.is_whitespace())?;
        Some(rest[..end].trim().to_string())
    } else {
        None
    }
}

fn parse_rust_enum(line: &str) -> Option<String> {
    if line.starts_with("enum ") || line.starts_with("pub enum ") {
        let start = line.find("enum ")? + 5;
        let rest = &line[start..];
        let end = rest.find(|c: char| c == '<' || c == '{' || c == ';' || c.is_whitespace())?;
        Some(rest[..end].trim().to_string())
    } else {
        None
    }
}

fn parse_rust_trait(line: &str) -> Option<String> {
    if line.starts_with("trait ") || line.starts_with("pub trait ") {
        let start = line.find("trait ")? + 6;
        let rest = &line[start..];
        let end = rest.find(|c: char| c == '<' || c == '{' || c == ':' || c.is_whitespace())?;
        Some(rest[..end].trim().to_string())
    } else {
        None
    }
}

fn parse_js_const_function(line: &str) -> Option<String> {
    if line.starts_with("const ") && line.contains(" = ") && (line.contains("=>") || line.contains("function")) {
        let start = 6; // "const ".len()
        let rest = &line[start..];
        let end = rest.find(" =")?;
        Some(rest[..end].trim().to_string())
    } else {
        None
    }
}

fn extract_name_after(line: &str, prefix: &str, delimiter: char) -> Option<String> {
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(delimiter).unwrap_or(rest.len());
    let name = rest[..end].trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}
