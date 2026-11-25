//! Job hunting tools backed by the RemoteOK public feed.
//!
//! job_search reads the RemoteOK API (no key needed) and returns
//! postings that match a set of keywords. The feed's first element is
//! metadata, not a job, so it is skipped. A search that finds nothing
//! is a normal empty result, but a search that could not run is an
//! error: network failures, bad HTTP statuses, and malformed payloads
//! all raise, so nobody mistakes a broken feed for "no jobs today".
//!
//! job_apply is deliberately narrower than its name. It does not
//! submit anything to an external site. It validates that the resume
//! lives inside the workspace, records the application in a local log
//! (`botropolis/data/applications.jsonl`), and returns the package,
//! apply url included, so a human or a browser flow does the actual
//! submission. Applying twice to the same role returns a warning
//! instead of a second record.
//!
//! Tests and evals can point `BOTROPOLIS_JOBS_FEED_FILE` at a saved
//! copy of the feed payload to run without the network, and
//! `BOTROPOLIS_APPLICATIONS_LOG` moves the applications log.

use serde_json::{json, Value};
use std::env;
use std::path::PathBuf;
use std::time::Duration;

use super::builtin::{arg_i64, arg_str};
use crate::sandbox::{resolve_workspace_path, workspace_relative};

pub const REMOTEOK_API_URL: &str = "https://remoteok.com/api";
const USER_AGENT: &str = "botropolis job_search tool";
pub const DEFAULT_LIMIT: i64 = 10;
pub const MAX_LIMIT: i64 = 25;

fn fetch_feed() -> Result<Value, String> {
    if let Ok(feed_file) = env::var("BOTROPOLIS_JOBS_FEED_FILE") {
        if !feed_file.is_empty() {
            let raw = std::fs::read_to_string(&feed_file).map_err(|e| {
                format!("job search failed: could not read feed file {feed_file}: {e}")
            })?;
            return serde_json::from_str(&raw).map_err(|e| {
                format!("job search failed: feed file {feed_file} is not valid JSON: {e}")
            });
        }
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new());
    let resp = client
        .get(REMOTEOK_API_URL)
        .send()
        .map_err(|e| format!("job search failed: could not reach the RemoteOK feed: {e}"))?;
    if resp.status().as_u16() != 200 {
        return Err(format!(
            "job search failed: RemoteOK returned HTTP {}",
            resp.status()
        ));
    }
    resp.json()
        .map_err(|e| format!("job search failed: the RemoteOK feed was not valid JSON: {e}"))
}

fn posting(item: &Value) -> Value {
    let tags = match item.get("tags") {
        Some(Value::Array(tags)) => tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect(),
        Some(other) => vec![other.to_string()],
        None => Vec::new(),
    };
    json!({
        "id": item.get("id").map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_default(),
        "title": item.get("position").and_then(|v| v.as_str()).unwrap_or(""),
        "company": item.get("company").and_then(|v| v.as_str()).unwrap_or(""),
        "tags": tags,
        "location": item.get("location").and_then(|v| v.as_str()).unwrap_or(""),
        "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
        "date": item.get("date").and_then(|v| v.as_str()).unwrap_or(""),
    })
}

fn matches(posting: &Value, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let title = posting.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let company = posting
        .get("company")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tags: Vec<String> = posting
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let haystack = format!("{} {} {}", title, company, tags.join(" ")).to_lowercase();
    terms.iter().all(|term| haystack.contains(term))
}

/// Search remote job postings on RemoteOK by keyword.
pub fn job_search(args: &Value) -> Result<Value, String> {
    let keywords = arg_str(args, "keywords")?;
    let limit = arg_i64(args, "limit", DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let payload = fetch_feed()?;
    let items = match payload.as_array() {
        Some(items) => items.clone(),
        None => {
            return Err(format!(
                "job search failed: the RemoteOK feed had an unexpected shape (expected a list, got {})",
                match &payload {
                    Value::Null => "null",
                    Value::Bool(_) => "bool",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "list",
                    Value::Object(_) => "object",
                }
            ));
        }
    };
    // The first element of the feed is metadata (timestamps, legal
    // text), not a posting. It is the one entry with no position.
    let items = if items
        .first()
        .map(|first| first.get("position").is_none())
        .unwrap_or(false)
    {
        items[1..].to_vec()
    } else {
        items
    };
    let terms: Vec<String> = keywords
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    let mut jobs = Vec::new();
    for item in &items {
        if item.get("position").is_none() {
            continue;
        }
        let post = posting(item);
        if matches(&post, &terms) {
            jobs.push(post);
        }
    }
    let total = jobs.len();
    jobs.truncate(limit);
    let count = jobs.len();
    let _ = total;
    Ok(json!({"keywords": keywords, "count": count, "jobs": jobs}))
}

fn applications_log_path() -> PathBuf {
    if let Ok(override_path) = env::var("BOTROPOLIS_APPLICATIONS_LOG") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    crate::paths::data_dir().join("applications.jsonl")
}

fn read_applications(path: &PathBuf) -> Vec<Value> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            serde_json::from_str(line).ok()
        })
        .collect()
}

/// Record a prepared job application. Does not submit anything.
pub fn job_apply(args: &Value) -> Result<Value, String> {
    let title = arg_str(args, "title")?;
    let company = arg_str(args, "company")?;
    let url = arg_str(args, "url")?;
    let resume = arg_str(args, "resume")?;
    let resume_path = resolve_workspace_path(&resume)?;
    if !resume_path.is_file() {
        return Err(format!("resume not found in the workspace: {resume}"));
    }
    let log_path = applications_log_path();
    for existing in read_applications(&log_path) {
        let same_company = existing
            .get("company")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case(company.trim());
        let same_title = existing
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case(title.trim());
        if same_company && same_title {
            let when = existing
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("an earlier run");
            return Ok(json!({
                "status": "duplicate",
                "warning": format!("already recorded an application for {title} at {company} on {when}; nothing new was written"),
                "application": existing,
                "apply_url": url,
            }));
        }
    }
    let record = json!({
        "timestamp": crate::tools::builtin::utc_now_iso(),
        "title": title,
        "company": company,
        "url": url,
        "resume": workspace_relative(&resume_path),
        "status": "prepared",
    });
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    file.write_all(format!("{}\n", serde_json::to_string(&record).unwrap()).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "status": "prepared",
        "application": record,
        "apply_url": url,
        "note": "Recorded locally, nothing was submitted. Use apply_url to submit the application.",
    }))
}
