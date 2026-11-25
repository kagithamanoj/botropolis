//! Gmail and Google Calendar tools for Botropolis agents.
//!
//! These shell out to the hatch_gws_cli commands from the gmail and
//! google-calendar skills. Everything degrades gracefully: when a
//! service is not connected, or the CLI is missing, the tool returns a
//! plain message instead of raising, so the agent loop keeps working.
//!
//! Deliberate limits:
//! - gmail_draft creates drafts only. There is no gmail_send tool:
//!   agents prepare mail, humans send it.
//! - calendar_create_event creates private events only (no attendees),
//!   so nothing ever notifies another person without a human in the loop.

use serde_json::{json, Value};
use std::process::Command;
use std::time::{Duration, Instant};

use super::builtin::{arg_i64, arg_str, arg_str_default};

const CLI: &str = "hatch_gws_cli";
const TIMEOUT_SECS: u64 = 90;

/// Run one hatch_gws_cli command.
///
/// Returns {"data": parsed} on success or {"error": message} on any
/// failure. Never raises.
fn run_cli(args: &[&str]) -> Value {
    let mut child = match Command::new(CLI)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            // Distinguish "not installed" from other spawn failures.
            if e.kind() == std::io::ErrorKind::NotFound {
                return json!({"error": format!("{CLI} is not installed on this machine.")});
            }
            return json!({"error": format!("Could not run {CLI}: {e}")});
        }
    };
    let deadline = Instant::now() + Duration::from_secs(TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(o) => o,
                    Err(e) => return json!({"error": format!("Could not run {CLI}: {e}")}),
                };
                let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !status.success() {
                    let message = if err.is_empty() { out.clone() } else { err };
                    let message = if message.is_empty() {
                        "unknown error".to_string()
                    } else {
                        message
                    };
                    let lowered = message.to_lowercase();
                    if lowered.contains("not connected") || lowered.contains("missing access_token")
                    {
                        return json!({
                            "error": "not_connected",
                            "detail": "This Google service is not connected on this machine. Tell the user to connect it before asking an agent to use it.",
                        });
                    }
                    let short: String = message.chars().take(300).collect();
                    return json!({"error": format!("Command failed: {short}")});
                }
                if out.is_empty() {
                    return json!({"error": "The command returned nothing."});
                }
                return match serde_json::from_str::<Value>(&out) {
                    Ok(parsed) => json!({"data": parsed}),
                    Err(_) => json!({"data": out}),
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return json!({"error": "The command timed out."});
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return json!({"error": format!("Could not run {CLI}: {e}")}),
        }
    }
}

fn short(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() > limit {
        let head: String = text.chars().take(limit).collect();
        format!("{head}... [truncated]")
    } else {
        text.to_string()
    }
}

/// Search Gmail with a Gmail search query. Read-only.
pub fn gmail_search(args: &Value) -> Result<Value, String> {
    let query = arg_str(args, "query")?;
    let max_results = arg_i64(args, "max_results", 5).clamp(1, 20);
    let max = max_results.to_string();
    let res = run_cli(&[
        "gmail", "+triage", "--query", &query, "--max", &max, "--format", "json",
    ]);
    if res.get("error").is_some() {
        return Ok(res);
    }
    let data = res.get("data").cloned().unwrap_or(Value::Null);
    if let Some(text) = data.as_str() {
        return Ok(json!({"query": query, "results": [], "note": text}));
    }
    let messages = data
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "query": query,
        "results": messages.iter().map(|m| json!({
            "id": m.get("id"),
            "from": m.get("from"),
            "subject": m.get("subject"),
            "date": m.get("date"),
        })).collect::<Vec<_>>(),
    }))
}

/// Read a single Gmail message by its id. Read-only.
pub fn gmail_read(args: &Value) -> Result<Value, String> {
    let message_id = arg_str(args, "message_id")?;
    let res = run_cli(&[
        "gmail",
        "+read",
        "--id",
        &message_id,
        "--headers",
        "--format",
        "json",
    ]);
    if res.get("error").is_some() {
        return Ok(res);
    }
    let data = res.get("data").cloned().unwrap_or(Value::Null);
    if let Some(text) = data.as_str() {
        return Ok(json!({"message_id": message_id, "body": short(text, 3000)}));
    }
    if !data.is_object() {
        return Ok(json!({"message_id": message_id, "body": short(&data.to_string(), 3000)}));
    }
    let body = data
        .get("body")
        .or_else(|| data.get("snippet"))
        .or_else(|| data.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| data.to_string());
    Ok(json!({
        "message_id": message_id,
        "from": data.get("from").or_else(|| data.get("From")),
        "subject": data.get("subject").or_else(|| data.get("Subject")),
        "date": data.get("date").or_else(|| data.get("Date")),
        "body": short(&body, 3000),
    }))
}

/// Create a Gmail draft. Drafts only, nothing is sent.
pub fn gmail_draft(args: &Value) -> Result<Value, String> {
    let to = arg_str(args, "to")?;
    let subject = arg_str(args, "subject")?;
    let body = arg_str(args, "body")?;
    if to.is_empty() || !to.contains('@') {
        return Ok(json!({"error": format!("Not a valid recipient address: {to:?}")}));
    }
    let res = run_cli(&[
        "gmail",
        "+draft",
        "--to",
        &to,
        "--subject",
        &subject,
        "--body",
        &body,
    ]);
    if res.get("error").is_some() {
        return Ok(res);
    }
    Ok(json!({"drafted": true, "to": to, "subject": subject, "detail": res.get("data")}))
}

/// List upcoming Google Calendar events. Read-only.
pub fn calendar_agenda(args: &Value) -> Result<Value, String> {
    let days = arg_i64(args, "days", 7).clamp(1, 31);
    let days_str = days.to_string();
    let res = run_cli(&[
        "calendar", "+agenda", "--days", &days_str, "--format", "json",
    ]);
    if res.get("error").is_some() {
        return Ok(res);
    }
    let data = res.get("data").cloned().unwrap_or(Value::Null);
    if let Some(text) = data.as_str() {
        return Ok(json!({"days": days, "events": [], "note": text}));
    }
    let events = data
        .get("events")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "days": days,
        "events": events.iter().map(|e| json!({
            "summary": e.get("summary"),
            "starts_at": e.get("event_starts_at").or_else(|| e.get("start")),
            "ends_at": e.get("event_ends_at").or_else(|| e.get("end")),
            "location": e.get("location"),
        })).collect::<Vec<_>>(),
    }))
}

/// Create a private event on the primary Google Calendar.
/// No attendees are invited, so nobody else is notified.
pub fn calendar_create_event(args: &Value) -> Result<Value, String> {
    let summary = arg_str(args, "summary").or_else(|_| arg_str(args, "title"))?;
    let start = arg_str(args, "start")?;
    let end = arg_str(args, "end")?;
    let description = arg_str_default(args, "description", "");
    let location = arg_str_default(args, "location", "");
    let mut event = json!({
        "summary": summary,
        "start": {"dateTime": start},
        "end": {"dateTime": end},
    });
    if !description.is_empty() {
        event["description"] = json!(description);
    }
    if !location.is_empty() {
        event["location"] = json!(location);
    }
    let params = serde_json::to_string(&json!({"calendarId": "primary"})).unwrap();
    let body = serde_json::to_string(&event).unwrap();
    let res = run_cli(&[
        "calendar", "events", "insert", "--params", &params, "--json", &body,
    ]);
    if res.get("error").is_some() {
        return Ok(res);
    }
    let data = res.get("data").cloned().unwrap_or(Value::Null);
    let created = if data.is_object() { data } else { json!({}) };
    Ok(json!({
        "created": true,
        "summary": summary,
        "starts_at": created.get("event_starts_at"),
        "ends_at": created.get("event_ends_at"),
    }))
}
