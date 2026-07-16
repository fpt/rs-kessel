//! Shared Gemma 4 native tool-call parsing.
//!
//! Both local backends speak Gemma's native tool wire format — `llm_local`
//! (llama.cpp, via the GGUF's embedded chat template) and
//! `protocol::GemmaProtocol` (gallium, hand-written template). The two used to
//! carry independent parsers for it; this module owns the format knowledge so
//! they parse it identically.
//!
//! Wire format:
//! `<|tool_call>call:NAME{key:<|"|>strval<|"|>, key2:42}<tool_call|>`
//! where `<|"|>` delimits string values (so a value may contain commas/braces).
//!
//! Names are returned verbatim. The alias helpers ([`normalise_tool_name`],
//! [`normalise_path_args`]) are opt-in: gallium applies them (its small Gemma
//! models hallucinate names like `write_file`); the llama.cpp path keeps names
//! exact so mixed-case MCP tool names still match.

use serde_json::{Map, Value};

/// The Gemma string-value delimiter token.
const STR_DELIM: &str = "<|\"|>";

/// One parsed native tool call, with the name exactly as the model emitted it.
#[derive(Debug, Clone, PartialEq)]
pub struct GemmaCall {
    pub name: String,
    pub arguments: Value,
}

/// Parse every `call:NAME{...}` native tool call in `text`, in order. The
/// `<|tool_call>` marker is optional — matching the `call:` form is enough, and
/// both engines' real outputs contain it.
pub fn parse_native_tool_calls(text: &str) -> Vec<GemmaCall> {
    use std::sync::OnceLock;
    // `call:NAME{ body }`. Names allow the MCP charset (letters/digits/._-).
    // The `<|"|>` string tokens contain no braces, so the `[^{}]*` body capture
    // is safe for the flat Gemma arg format.
    static CALL_RE: OnceLock<regex::Regex> = OnceLock::new();
    let call_re = CALL_RE
        .get_or_init(|| regex::Regex::new(r"call:\s*([A-Za-z0-9_.\-]+)\s*\{([^{}]*)\}").unwrap());
    call_re
        .captures_iter(text)
        .map(|cap| GemmaCall {
            name: cap[1].to_string(),
            arguments: parse_kv_args(&cap[2]),
        })
        .collect()
}

/// Parse a `key:<|"|>strval<|"|>, key2:scalar, ...` body into a JSON object.
/// String values keep everything between the `<|"|>` delimiters (commas
/// included); bare values are coerced by [`parse_scalar`].
pub fn parse_kv_args(inner: &str) -> Value {
    let mut map = Map::new();
    let mut s = inner;

    loop {
        s = s.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        if s.is_empty() {
            break;
        }

        let colon = match s.find(':') {
            Some(p) => p,
            None => break,
        };
        let key = s[..colon].trim().to_string();
        s = &s[colon + 1..];
        if key.is_empty() {
            break;
        }

        if let Some(rest) = s.strip_prefix(STR_DELIM) {
            // String value enclosed in <|"|>...<|"|>.
            match rest.find(STR_DELIM) {
                Some(end) => {
                    map.insert(key, Value::String(rest[..end].to_string()));
                    s = &rest[end + STR_DELIM.len()..];
                }
                None => {
                    // Malformed: consume the remainder as the value.
                    map.insert(key, Value::String(rest.to_string()));
                    break;
                }
            }
        } else {
            // Bare value: read until the next comma or the end.
            let end = s.find(',').unwrap_or(s.len());
            map.insert(key, parse_scalar(s[..end].trim()));
            s = &s[end..];
        }
    }

    Value::Object(map)
}

/// Coerce a bare (non-string) Gemma value: bool / null / integer / float, else
/// keep it as a string.
pub fn parse_scalar(s: &str) -> Value {
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => {
            if let Ok(n) = s.parse::<i64>() {
                Value::from(n)
            } else if let Ok(f) = s.parse::<f64>() {
                Value::from(f)
            } else {
                Value::String(s.to_string())
            }
        }
    }
}

/// Fold common tool-name aliases a Gemma model may hallucinate onto the
/// registered names (e.g. `write_file` → `write`). Opt-in per caller.
///
/// Note `ls` is NOT an alias: kessel registers a real `ls` tool, so an `ls`
/// call must route to it verbatim (folding it onto `glob` used to send a bogus
/// `file_path` arg to a tool that wants `pattern`, wedging the ReAct loop).
pub fn normalise_tool_name(name: &str) -> String {
    match name {
        "write_file" | "create_file" | "file_write" | "write_to_file" | "writefile"
        | "write_tool" | "writetool" | "write_content" | "create" => "write".to_string(),
        "read_file" | "file_read" | "readfile" | "open_file" | "read_tool" => "read".to_string(),
        "list_files" | "list_file" | "find_files" | "glob_tool" => "glob".to_string(),
        "edit_file" | "file_edit" | "update_file" | "patch_file" | "edit_tool" => "edit".to_string(),
        _ => name.to_string(),
    }
}

/// Fold the short `file` / `path` argument aliases onto `file_path` — but only
/// for the file tools whose canonical parameter IS `file_path`. Other tools
/// (`ls`, `glob`, MCP tools, …) legitimately take `path`-named params that must
/// pass through untouched.
pub fn normalise_path_args(tool: &str, args: &mut Value) {
    if !matches!(tool, "read" | "write" | "edit" | "multi_edit") {
        return;
    }
    if let Some(map) = args.as_object_mut() {
        if let Some(v) = map.remove("file") {
            map.entry("file_path".to_string()).or_insert(v);
        }
        if let Some(v) = map.remove("path") {
            map.entry("file_path".to_string()).or_insert(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_native_call() {
        let calls = parse_native_tool_calls(
            "<|tool_call>call:search-godoc{query:<|\"|>mcp-go<|\"|>}<tool_call|>",
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search-godoc"); // verbatim, not normalised
        assert_eq!(calls[0].arguments["query"], "mcp-go");
    }

    #[test]
    fn parses_mixed_string_and_scalar_args() {
        let calls = parse_native_tool_calls(
            "<|tool_call>call:grep{pattern:<|\"|>foo<|\"|>, limit:50}<tool_call|>",
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["pattern"], "foo");
        assert_eq!(calls[0].arguments["limit"], 50);
    }

    #[test]
    fn string_value_may_contain_commas() {
        let v = parse_kv_args("msg:<|\"|>a, b, c<|\"|>, n:3");
        assert_eq!(v["msg"], "a, b, c");
        assert_eq!(v["n"], 3);
    }

    #[test]
    fn parses_multiple_calls() {
        let calls = parse_native_tool_calls(
            "call:read{file_path:<|\"|>a.rs<|\"|>} call:glob{pattern:<|\"|>*.rs<|\"|>}",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[1].name, "glob");
    }

    #[test]
    fn plain_prose_is_not_a_call() {
        assert!(parse_native_tool_calls("Sure, I'll call the search tool for you.").is_empty());
    }

    #[test]
    fn name_and_path_aliases_fold() {
        assert_eq!(normalise_tool_name("write_file"), "write");
        assert_eq!(normalise_tool_name("search-godoc"), "search-godoc");
        let mut args = serde_json::json!({"file": "x.rs"});
        normalise_path_args("read", &mut args);
        assert_eq!(args["file_path"], "x.rs");
        assert!(args.get("file").is_none());
    }

    #[test]
    fn ls_is_a_real_tool_and_keeps_its_path_arg() {
        // kessel registers a real `ls` tool taking `path` — neither the name
        // nor the arg may be folded (this wedged the 26B file_read loop).
        assert_eq!(normalise_tool_name("ls"), "ls");
        let mut args = serde_json::json!({"path": "."});
        normalise_path_args("ls", &mut args);
        assert_eq!(args["path"], ".");
        assert!(args.get("file_path").is_none());
    }
}
