//! Builtin tools shipped with Botropolis.
//!
//! The calculator lands here with the tool loop, because the loop
//! tests need one real tool to run. The rest of the builtins land in
//! the next PR.

use serde_json::{json, Value};

use super::expr::eval_calculator;

/// Read a required string argument, or an error naming it.
pub fn arg_str(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing required argument: {name}"))
}

/// Read an optional integer argument with a default.
pub fn arg_i64(args: &Value, name: &str, default: i64) -> i64 {
    args.get(name)
        .and_then(|v| v.as_i64())
        .or_else(|| args.get(name).and_then(|v| v.as_f64()).map(|f| f as i64))
        .unwrap_or(default)
}

/// Read an optional string argument with a default.
pub fn arg_str_default(args: &Value, name: &str, default: &str) -> String {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default.to_string())
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
