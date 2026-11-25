//! Builtin tools shipped with Botropolis.
//!
//! Each tool has a name, a description, a JSON-schema-style parameter
//! map, and an execute function. Agents declare which tools they may
//! actually call in the `toolkit` field of their spec YAML.
//!
//! File and shell tools are sandboxed: see `crate::sandbox`.
//! `web_search` uses DuckDuckGo with no API key and degrades to an
//! empty result set (never an exception) when the network is unavailable.

use regex::Regex;
use serde_json::{json, Value};
use std::io::Read;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::expr::eval_calculator;
use crate::sandbox::{
    check_shell_command, resolve_workspace_path, workspace_relative, workspace_root,
};

const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// Cap raw download size so one huge page cannot eat memory.
const MAX_FETCH_BYTES: usize = 2_000_000;

/// The shared company notebook lives at the top of the agent workspace.
/// It is plain markdown so humans can read and edit it too.
const NOTES_FILE: &str = "notes.md";
const NOTES_READ_CHARS: usize = 4000;

/// Read a required string argument, or an error naming it.
pub fn arg_str(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required argument: {name}"))
}

/// Read an optional integer argument with a default. Accepts JSON
/// numbers and numeric strings.
pub fn arg_i64(args: &Value, name: &str, default: i64) -> i64 {
    match args.get(name) {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().map(|f| f as i64).unwrap_or(default)),
        Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(default),
        _ => default,
    }
}

/// Read an optional string argument with a default.
pub fn arg_str_default(args: &Value, name: &str, default: &str) -> String {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default.to_string())
}

/// Read an optional string argument, None when absent.
pub fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(BROWSER_UA)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn strip_tags(fragment: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    static WS_RE: OnceLock<Regex> = OnceLock::new();
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    let ws_re = WS_RE.get_or_init(|| Regex::new(r"\s+").unwrap());
    let no_tags = tag_re.replace_all(fragment, "");
    let unescaped = html_unescape(&no_tags);
    ws_re.replace_all(&unescaped, " ").trim().to_string()
}

fn html_unescape(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn ddg_real_url(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let rest = &href[idx + 5..];
        let end = rest.find('&').unwrap_or(rest.len());
        let encoded = &rest[..end];
        let mut out = String::new();
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                let hex: String = chars.by_ref().take(2).collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    out.push(byte as char);
                }
            } else if c == '+' {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https:{stripped}");
    }
    href.to_string()
}

/// Web search via DuckDuckGo. No API key needed.
pub fn web_search(args: &Value) -> Result<Value, String> {
    let query = arg_str(args, "query")?;
    let num_results = arg_i64(args, "num_results", 5).max(1) as usize;
    let resp = match http_client()
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query.as_str())])
        .timeout(Duration::from_secs(15))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(
                json!({"query": query, "results": [], "error": format!("search failed: {e}")}),
            );
        }
    };
    if !resp.status().is_success() {
        return Ok(
            json!({"query": query, "results": [], "error": format!("search failed: HTTP {}", resp.status())}),
        );
    }
    let body = resp.text().unwrap_or_default();
    let link_re =
        Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snippet_re = Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();
    let links: Vec<(String, String)> = link_re
        .captures_iter(&body)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    let snippets: Vec<String> = snippet_re
        .captures_iter(&body)
        .map(|c| c[1].to_string())
        .collect();
    let mut results = Vec::new();
    for (index, (href, title_html)) in links.iter().enumerate() {
        let snippet = snippets.get(index).cloned().unwrap_or_default();
        results.push(json!({
            "title": strip_tags(title_html),
            "url": ddg_real_url(href),
            "snippet": strip_tags(&snippet),
        }));
        if results.len() >= num_results {
            break;
        }
    }
    Ok(json!({"query": query, "results": results}))
}

/// Fetch a web page and return readable text.
pub fn web_fetch(args: &Value) -> Result<Value, String> {
    let url = arg_str(args, "url")?;
    let max_chars = arg_i64(args, "max_chars", 20000).max(0) as usize;
    let resp = match http_client()
        .get(&url)
        .timeout(Duration::from_secs(20))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            return Ok(json!({"url": url, "text": "", "error": format!("fetch failed: {e}")}));
        }
    };
    if !resp.status().is_success() {
        return Ok(
            json!({"url": url, "text": "", "error": format!("fetch failed: HTTP {}", resp.status())}),
        );
    }
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.contains("html") && !content_type.contains("text") {
        return Ok(
            json!({"url": url, "text": "", "error": format!("not a text page: {content_type}")}),
        );
    }
    let mut buf = Vec::new();
    let mut body = resp;
    let mut chunk = [0u8; 65536];
    loop {
        match body.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_FETCH_BYTES {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let page = String::from_utf8_lossy(&buf).to_string();
    let script_re = Regex::new(r"(?is)<script.*?</script>").unwrap();
    let style_re = Regex::new(r"(?is)<style.*?</style>").unwrap();
    let page = script_re.replace_all(&page, " ");
    let page = style_re.replace_all(&page, " ");
    let text = strip_tags(&page);
    let truncated = text.chars().count() > max_chars;
    let text: String = text.chars().take(max_chars).collect();
    Ok(json!({"url": url, "text": text, "truncated": truncated}))
}

/// Run a shell command inside the agent workspace.
pub fn shell(args: &Value) -> Result<Value, String> {
    let command = arg_str(args, "command")?;
    let timeout = arg_i64(args, "timeout", 30).clamp(1, 60);
    check_shell_command(&command)?;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(workspace_root())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(timeout as u64);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|e| e.to_string())?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                return Ok(json!({
                    "returncode": status.code(),
                    "stdout": tail_chars(&stdout, 8000),
                    "stderr": tail_chars(&stderr, 4000),
                }));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(json!({
                        "returncode": Value::Null,
                        "stdout": "",
                        "stderr": "",
                        "error": format!("timed out after {timeout}s"),
                    }));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("could not run: {e}")),
        }
    }
}

fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        text.to_string()
    } else {
        text.chars().skip(count - limit).collect()
    }
}

/// Safe arithmetic evaluator. No imports, no attribute access.
pub fn calculator(args: &Value) -> Result<Value, String> {
    let expression = arg_str(args, "expression")?;
    let value = eval_calculator(&expression)?;
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        Ok(json!(value as i64))
    } else {
        Ok(json!(value))
    }
}

/// Read a UTF-8 text file inside the agent workspace.
pub fn read_file(args: &Value) -> Result<Value, String> {
    let path = arg_str(args, "path")?;
    let max_chars = arg_i64(args, "max_chars", 60000).max(0) as usize;
    let target = resolve_workspace_path(&path)?;
    if !target.is_file() {
        return Err(format!("No such file in workspace: {path}"));
    }
    let content = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
    let text: String = content.chars().take(max_chars).collect();
    Ok(Value::String(text))
}

/// Write text to a file inside the agent workspace, creating parents.
pub fn write_file(args: &Value) -> Result<Value, String> {
    let path = arg_str(args, "path")?;
    let content = arg_str(args, "content")?;
    let target = resolve_workspace_path(&path)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&target, &content).map_err(|e| e.to_string())?;
    Ok(json!({"path": workspace_relative(&target), "bytes": content.len()}))
}

/// List entries of a directory inside the agent workspace.
pub fn list_dir(args: &Value) -> Result<Value, String> {
    let path = arg_str_default(args, "path", ".");
    let target = resolve_workspace_path(&path)?;
    if !target.exists() {
        return Err(format!("No such path in workspace: {path}"));
    }
    if !target.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }
    let mut entries: Vec<String> = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&target)
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    names.sort_by_key(|e| e.file_name());
    for entry in names {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(if is_dir { format!("{name}/") } else { name });
    }
    Ok(json!({"path": workspace_relative(&target), "entries": entries}))
}

/// Current UTC time as an ISO 8601 string.
pub fn current_time(_args: &Value) -> Result<Value, String> {
    Ok(Value::String(utc_now_iso()))
}

/// The current UTC time, ISO 8601 with a +00:00 offset.
pub fn utc_now_iso() -> String {
    let now = SystemTime::now();
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+00:00")
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The absolute path of the shared company notebook file.
pub fn notebook_path() -> std::path::PathBuf {
    resolve_workspace_path(NOTES_FILE).unwrap_or_else(|_| workspace_root().join(NOTES_FILE))
}

/// Append a timestamped entry to the shared company notebook.
pub fn notes_append(args: &Value) -> Result<Value, String> {
    let text = arg_str(args, "text")?.trim().to_string();
    if text.is_empty() {
        return Ok(json!({"error": "Nothing to note: text was empty."}));
    }
    let text = if text.chars().count() > 2000 {
        let head: String = text.chars().take(2000).collect();
        format!("{head}... [truncated]")
    } else {
        text
    };
    let path = notebook_path();
    let stamp = utc_now_iso();
    let stamp = stamp
        .get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or(stamp.clone())
        + " UTC";
    let entry = format!("\n## {stamp}\n\n{text}\n");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Could not write the notebook: {e}"))?;
    file.write_all(entry.as_bytes())
        .map_err(|e| format!("Could not write the notebook: {e}"))?;
    Ok(json!({"noted": true, "file": workspace_relative(&path)}))
}

/// Read the shared company notebook, newest entries last.
pub fn notes_read(_args: &Value) -> Result<Value, String> {
    let path = notebook_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"notes": "", "note": "The notebook is empty."}));
        }
        Err(e) => {
            return Ok(json!({"error": format!("Could not read the notebook: {e}")}));
        }
    };
    let content = content.trim().to_string();
    if content.chars().count() > NOTES_READ_CHARS {
        let tail: String = content
            .chars()
            .skip(content.chars().count() - NOTES_READ_CHARS)
            .collect();
        return Ok(json!({"notes": format!("... [older entries truncated]\n{tail}")}));
    }
    Ok(json!({"notes": content}))
}
