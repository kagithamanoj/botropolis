//! Math tool: an expression evaluator with polynomial algebra.
//!
//! The calculator does arithmetic. This one does algebra and calculus
//! on polynomials: solve equations in one variable, simplify, factor,
//! expand, differentiate, integrate, limits, series, and numeric
//! evaluation of expressions with constants.
//!
//! Plainly stated limit: Rust has no sympy equal, so this is an
//! evaluator, not symbolic algebra. It handles arithmetic, functions,
//! and variables numerically, and exact polynomial algebra in one
//! variable. General symbolic work (transcendental equations,
//! non-polynomial derivatives and integrals, symbolic limits) is out
//! of scope and fails with a clear error instead of a made-up answer.
//!
//! Parsing is defensive, in two layers, like the Python tool. The raw
//! string is first checked against a character whitelist and an
//! identifier whitelist, so nothing but numbers, operators, and known
//! names can reach the parser at all. It is then parsed with the
//! shared expression parser against only the allowed names.

use serde_json::{json, Value};
use std::collections::HashMap;

use super::builtin::{arg_i64, arg_str, arg_str_default, arg_str_opt};
use super::expr::{eval, parse, BinOp, Expr, FuncTable};

/// Cap input size so one expression cannot turn into an unbounded
/// computation.
pub const MAX_EXPR_CHARS: usize = 200;

/// Variables an expression may use. `variable` arguments must name one.
pub const VARIABLES: [&str; 5] = ["x", "y", "z", "t", "n"];

/// Functions an expression may call.
pub const FUNCTIONS: [&str; 13] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "exp", "log", "sqrt",
    "Abs",
];

/// Named constants. `oo` is infinity, for limits and improper
/// integrals. `e` is the spelling everyone actually types for
/// Euler's number, so it counts too.
pub const CONSTANTS: [&str; 4] = ["pi", "E", "e", "oo"];

pub const OPERATIONS: [&str; 9] = [
    "solve",
    "simplify",
    "factor",
    "expand",
    "diff",
    "integrate",
    "limit",
    "series",
    "eval",
];

fn allowed_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    names.extend(VARIABLES);
    names.extend(FUNCTIONS);
    names.extend(CONSTANTS);
    names
}

fn validate(text: &str, what: &str) -> Result<String, String> {
    let cleaned = text.trim().to_string();
    if cleaned.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if cleaned.chars().count() > MAX_EXPR_CHARS {
        return Err(format!(
            "{what} is too long ({} chars, max {MAX_EXPR_CHARS})",
            cleaned.chars().count()
        ));
    }
    for ch in cleaned.chars() {
        if !(ch.is_ascii_alphanumeric() || ch.is_whitespace() || "+-*/^(),.=".contains(ch)) {
            let bad: Vec<char> = cleaned
                .chars()
                .filter(|c| {
                    !(c.is_ascii_alphanumeric() || c.is_whitespace() || "+-*/^(),.=".contains(*c))
                })
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            return Err(format!(
                "{what} contains characters that are not allowed: {bad:?}. Use digits, + - * / ^ ( ) , . = and the allowed names."
            ));
        }
    }
    let mut ident = String::new();
    for ch in cleaned.chars() {
        if ch.is_ascii_alphabetic() {
            ident.push(ch);
        } else {
            if !ident.is_empty() {
                if !allowed_names().contains(&ident.as_str()) {
                    return Err(format!(
                        "{what} uses the name '{ident}', which is not allowed. Variables: {}. Functions: {}. Constants: {}.",
                        VARIABLES.join(", "),
                        FUNCTIONS.join(", "),
                        CONSTANTS.join(", ")
                    ));
                }
                ident.clear();
            }
        }
    }
    if !ident.is_empty() && !allowed_names().contains(&ident.as_str()) {
        return Err(format!(
            "{what} uses the name '{ident}', which is not allowed. Variables: {}. Functions: {}. Constants: {}.",
            VARIABLES.join(", "),
            FUNCTIONS.join(", "),
            CONSTANTS.join(", ")
        ));
    }
    Ok(cleaned)
}

/// Insert `*` for implicit multiplication: `2x`, `2(x+1)`, `(x+1)(x+2)`.
fn insert_implicit_mult(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let prev_is_value = prev.is_ascii_digit() || prev == ')' || prev == '.';
            let prev_is_name_end = prev.is_ascii_alphabetic();
            let cur_starts_value = c == '(' || c.is_ascii_alphabetic();
            if (prev_is_value && cur_starts_value)
                || (prev_is_name_end && c == '(' && !is_function_start(&chars, i))
            {
                // Do not split a function name from its `(`: `sin(` stays.
                // Do not split a number's exponent: `1e3` is one token.
                let is_exponent = prev_is_value
                    && c == 'e'
                    && i + 1 < chars.len()
                    && chars[i + 1].is_ascii_digit();
                if !is_exponent {
                    out.push('*');
                }
            }
        }
        out.push(c);
    }
    out
}

/// True when the identifier ending just before position `i` is a known function.
fn is_function_start(chars: &[char], i: usize) -> bool {
    let mut start = i;
    while start > 0 && chars[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    let name: String = chars[start..i].iter().collect();
    FUNCTIONS.contains(&name.as_str())
}

fn parse_expr(text: &str, what: &str) -> Result<Expr, String> {
    let cleaned = validate(text, what)?;
    let with_mult = insert_implicit_mult(&cleaned);
    parse(&with_mult).map_err(|e| format!("could not parse {what} {text:?}: {e}"))
}

/// The function table the math tool allows.
pub fn math_funcs() -> FuncTable {
    let mut funcs: FuncTable = HashMap::new();
    let mut add = |name: &str, f: fn(&[f64]) -> Result<f64, String>| {
        funcs.insert(name.to_string(), f);
    };
    add("sin", |a| one(a, "sin", f64::sin));
    add("cos", |a| one(a, "cos", f64::cos));
    add("tan", |a| one(a, "tan", f64::tan));
    add("asin", |a| one(a, "asin", f64::asin));
    add("acos", |a| one(a, "acos", f64::acos));
    add("atan", |a| one(a, "atan", f64::atan));
    add("sinh", |a| one(a, "sinh", f64::sinh));
    add("cosh", |a| one(a, "cosh", f64::cosh));
    add("tanh", |a| one(a, "tanh", f64::tanh));
    add("exp", |a| one(a, "exp", f64::exp));
    add("log", |a| one(a, "log", f64::ln));
    add("sqrt", |a| {
        let v = one(a, "sqrt", |x| x)?;
        if v < 0.0 {
            return Err("sqrt of a negative number".to_string());
        }
        Ok(v.sqrt())
    });
    add("Abs", |a| one(a, "Abs", f64::abs));
    funcs
}

fn one(args: &[f64], name: &str, f: fn(f64) -> f64) -> Result<f64, String> {
    if args.len() != 1 {
        return Err(format!("{name} takes exactly one argument"));
    }
    Ok(f(args[0]))
}

fn constants_env() -> HashMap<String, f64> {
    let mut env = HashMap::new();
    env.insert("pi".to_string(), std::f64::consts::PI);
    env.insert("E".to_string(), std::f64::consts::E);
    env.insert("e".to_string(), std::f64::consts::E);
    env
}

/// A polynomial in one variable, coefficients low to high.
type Poly = Vec<f64>;

fn trim_poly(mut p: Poly) -> Poly {
    while p.len() > 1 && p.last().map(|c| *c == 0.0).unwrap_or(false) {
        p.pop();
    }
    if p.is_empty() {
        p.push(0.0);
    }
    p
}

fn poly_add(a: &Poly, b: &Poly) -> Poly {
    let mut out = vec![0.0; a.len().max(b.len())];
    for (i, c) in a.iter().enumerate() {
        out[i] += c;
    }
    for (i, c) in b.iter().enumerate() {
        out[i] += c;
    }
    trim_poly(out)
}

fn poly_mul(a: &Poly, b: &Poly) -> Poly {
    let mut out = vec![0.0; a.len() + b.len()];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            out[i + j] += x * y;
        }
    }
    trim_poly(out)
}

fn poly_pow(base: &Poly, n: i64) -> Option<Poly> {
    if !(0..=64).contains(&n) {
        return None;
    }
    let mut out = vec![1.0];
    for _ in 0..n {
        out = poly_mul(&out, base);
    }
    Some(out)
}

/// Convert an expression to a polynomial in `var`, when it is one.
/// Returns None for anything with functions of the variable, other
/// variables, or non-integer powers.
fn to_poly(expr: &Expr, var: &str) -> Option<Poly> {
    match expr {
        Expr::Num(v) => Some(vec![*v]),
        Expr::Var(name) => {
            if name == var {
                Some(vec![0.0, 1.0])
            } else if name == "pi" {
                Some(vec![std::f64::consts::PI])
            } else if name == "E" || name == "e" {
                Some(vec![std::f64::consts::E])
            } else {
                None
            }
        }
        Expr::Neg(inner) => to_poly(inner, var).map(|p| p.iter().map(|c| -c).collect()),
        Expr::Bin(op, left, right) => {
            let l = to_poly(left, var)?;
            let r = to_poly(right, var)?;
            match op {
                BinOp::Add => Some(poly_add(&l, &r)),
                BinOp::Sub => Some(poly_add(&l, &r.iter().map(|c| -c).collect())),
                BinOp::Mul => Some(poly_mul(&l, &r)),
                BinOp::Div => {
                    // Only exact division by a constant.
                    if r.len() == 1 && r[0] != 0.0 {
                        Some(l.iter().map(|c| c / r[0]).collect())
                    } else {
                        None
                    }
                }
                BinOp::Mod => None,
                BinOp::Pow => {
                    if r.len() == 1 && r[0].fract() == 0.0 {
                        poly_pow(&l, r[0] as i64)
                    } else {
                        None
                    }
                }
            }
        }
        Expr::Call(_, _) => None,
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.10}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s == "-" {
            "0".to_string()
        } else {
            s
        }
    }
}

/// Render a polynomial in words, e.g. `x^2 - 5*x + 6`.
fn render_poly(p: &Poly, var: &str) -> String {
    let p = trim_poly(p.to_vec());
    if p.len() == 1 {
        return fmt_num(p[0]);
    }
    let mut out = String::new();
    let mut first = true;
    for (deg, coeff) in p.iter().enumerate().rev() {
        if *coeff == 0.0 {
            continue;
        }
        let abs = coeff.abs();
        let term = if deg == 0 {
            fmt_num(abs)
        } else if abs == 1.0 {
            if deg == 1 {
                var.to_string()
            } else {
                format!("{var}^{deg}")
            }
        } else if deg == 1 {
            format!("{}*{var}", fmt_num(abs))
        } else {
            format!("{}*{var}^{deg}", fmt_num(abs))
        };
        if first {
            if *coeff < 0.0 {
                out.push('-');
            }
            out.push_str(&term);
            first = false;
        } else if *coeff < 0.0 {
            out.push_str(&format!(" - {term}"));
        } else {
            out.push_str(&format!(" + {term}"));
        }
    }
    if out.is_empty() {
        "0".to_string()
    } else {
        out
    }
}

fn poly_degree(p: &Poly) -> usize {
    trim_poly(p.to_vec()).len() - 1
}

fn poly_eval(p: &Poly, x: f64) -> f64 {
    p.iter().rev().fold(0.0, |acc, c| acc * x + c)
}

fn poly_derivative(p: &Poly) -> Poly {
    if p.len() <= 1 {
        return vec![0.0];
    }
    trim_poly(
        p.iter()
            .enumerate()
            .skip(1)
            .map(|(i, c)| c * i as f64)
            .collect(),
    )
}

fn poly_antiderivative(p: &Poly) -> Poly {
    let mut out = vec![0.0];
    for (i, c) in p.iter().enumerate() {
        out.push(c / (i as f64 + 1.0));
    }
    trim_poly(out)
}

fn numeric_result(op: &str, result: &str, value: Option<f64>) -> Value {
    let mut out = json!({"operation": op, "result": result});
    if let Some(v) = value {
        out["value"] = json!(v);
    }
    out
}

fn parse_number(text: &str, what: &str) -> Result<f64, String> {
    let expr = parse_expr(text, what)?;
    let env = constants_env();
    let is_neg_infinity = matches!(&expr, Expr::Neg(inner) if matches!(inner.as_ref(), Expr::Var(name) if name == "oo"));
    if is_neg_infinity {
        return Ok(f64::NEG_INFINITY);
    }
    if matches!(&expr, Expr::Var(name) if name == "oo") {
        return Ok(f64::INFINITY);
    }
    eval(&expr, &env, &math_funcs()).map_err(|e| format!("could not parse {what} {text:?}: {e}"))
}

fn body_expr(expression: &str) -> Result<Expr, String> {
    if expression.contains('=') {
        return Err(
            "this operation takes a plain expression, not an equation. Use operation 'solve' for equations."
                .to_string(),
        );
    }
    parse_expr(expression, "expression")
}

/// The math tool: expression evaluation with polynomial algebra.
pub fn math(args: &Value) -> Result<Value, String> {
    let operation = arg_str(args, "operation")?.trim().to_lowercase();
    if !OPERATIONS.contains(&operation.as_str()) {
        return Err(format!(
            "unsupported operation {operation:?}. Supported operations: {}",
            OPERATIONS.join(", ")
        ));
    }
    let expression = arg_str(args, "expression")?;
    let variable = arg_str_default(args, "variable", "x");
    let variable = variable.trim();
    if !VARIABLES.contains(&variable) {
        return Err(format!(
            "unknown variable {variable:?}. Use one of: {}",
            VARIABLES.join(", ")
        ));
    }
    let point = arg_str_opt(args, "point");
    let lower = arg_str_opt(args, "lower");
    let upper = arg_str_opt(args, "upper");
    let terms = arg_i64(args, "terms", 6);
    run_op(
        &operation,
        &expression,
        variable,
        point,
        lower,
        upper,
        terms,
    )
    .map_err(|e| format!("math {operation} failed for {expression:?}: {e}"))
}

fn run_op(
    op: &str,
    expression: &str,
    var: &str,
    point: Option<String>,
    lower: Option<String>,
    upper: Option<String>,
    terms: i64,
) -> Result<Value, String> {
    match op {
        "solve" => op_solve(expression, var),
        "simplify" => {
            let expr = body_expr(expression)?;
            let poly = to_poly(&expr, var).ok_or_else(|| {
                format!("simplify needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
            })?;
            Ok(numeric_result("simplify", &render_poly(&poly, var), None))
        }
        "factor" => {
            let expr = body_expr(expression)?;
            let poly = to_poly(&expr, var).ok_or_else(|| {
                format!("factor needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
            })?;
            Ok(numeric_result("factor", &factor_poly(&poly, var), None))
        }
        "expand" => {
            let expr = body_expr(expression)?;
            let poly = to_poly(&expr, var).ok_or_else(|| {
                format!("expand needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
            })?;
            Ok(numeric_result("expand", &render_poly(&poly, var), None))
        }
        "diff" => {
            let expr = body_expr(expression)?;
            let poly = to_poly(&expr, var).ok_or_else(|| {
                format!("diff needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
            })?;
            let derived = poly_derivative(&poly);
            Ok(numeric_result("diff", &render_poly(&derived, var), None))
        }
        "integrate" => op_integrate(expression, var, lower, upper),
        "limit" => op_limit(expression, var, point),
        "series" => op_series(expression, var, point, terms),
        "eval" => op_eval(expression, var),
        _ => Err(format!("unsupported operation {op:?}")),
    }
}

fn op_solve(expression: &str, var: &str) -> Result<Value, String> {
    let text = validate(expression, "expression")?;
    if text.matches('=').count() > 1 {
        return Err("solve takes one equation with a single '='".to_string());
    }
    let (lhs, rhs) = match text.split_once('=') {
        Some((l, r)) => (parse_expr(l, "left side")?, parse_expr(r, "right side")?),
        None => (parse_expr(&text, "expression")?, Expr::Num(0.0)),
    };
    let lp = to_poly(&lhs, var).ok_or_else(|| {
        format!("solve needs a polynomial equation in {var}; this evaluator does not do general symbolic algebra")
    })?;
    let rp = to_poly(&rhs, var).ok_or_else(|| {
        format!("solve needs a polynomial equation in {var}; this evaluator does not do general symbolic algebra")
    })?;
    let poly = poly_add(&lp, &rp.iter().map(|c| -c).collect());
    let degree = poly_degree(&poly);
    if degree == 0 {
        return Ok(json!({
            "operation": "solve",
            "variable": var,
            "solutions": [],
            "result": "no solutions",
        }));
    }
    if degree > 2 {
        return Err(
            "solve handles polynomials up to degree 2; this evaluator does not do general symbolic algebra"
                .to_string(),
        );
    }
    let rendered: Vec<String>;
    let mut values: Vec<f64> = Vec::new();
    if degree == 1 {
        let root = -poly[0] / poly[1];
        rendered = vec![fmt_num(root)];
        values.push(root);
    } else {
        let (a, b, c) = (poly[2], poly[1], poly[0]);
        let disc = b * b - 4.0 * a * c;
        if disc > 0.0 {
            let sq = disc.sqrt();
            let r1 = (-b - sq) / (2.0 * a);
            let r2 = (-b + sq) / (2.0 * a);
            rendered = vec![fmt_num(r1), fmt_num(r2)];
            values = vec![r1, r2];
        } else if disc == 0.0 {
            let r = -b / (2.0 * a);
            rendered = vec![fmt_num(r)];
            values.push(r);
        } else {
            let real = -b / (2.0 * a);
            let imag = (-disc).sqrt() / (2.0 * a);
            rendered = vec![
                format!("{} + {}i", fmt_num(real), fmt_num(imag.abs())),
                format!("{} - {}i", fmt_num(real), fmt_num(imag.abs())),
            ];
        }
    }
    let mut out = json!({
        "operation": "solve",
        "variable": var,
        "solutions": rendered,
        "result": rendered.iter().map(|s| format!("{var} = {s}")).collect::<Vec<_>>().join(", "),
    });
    if !values.is_empty() && values.len() == rendered.len() {
        out["values"] = json!(values);
    }
    Ok(out)
}

fn factor_poly(p: &Poly, var: &str) -> String {
    let p = trim_poly(p.to_vec());
    if poly_degree(&p) == 2 {
        // Rational root search: try integer and simple fractional roots.
        let (a, b, c) = (p[2], p[1], p[0]);
        let mut roots: Vec<f64> = Vec::new();
        for candidate in rational_candidates(a, c) {
            if poly_eval(&p, candidate).abs() < 1e-9 {
                roots.push(candidate);
            }
        }
        if roots.len() >= 2 {
            let (r1, r2) = (roots[0], roots[1]);
            let f1 = factor_term(r1, var);
            let f2 = factor_term(r2, var);
            let lead = if a == 1.0 {
                String::new()
            } else {
                format!("{}*", fmt_num(a))
            };
            let _ = b;
            return format!("{lead}({f1})*({f2})");
        }
    }
    render_poly(&p, var)
}

fn factor_term(root: f64, var: &str) -> String {
    if root == 0.0 {
        var.to_string()
    } else if root < 0.0 {
        format!("{var} + {}", fmt_num(-root))
    } else {
        format!("{var} - {}", fmt_num(root))
    }
}

fn rational_candidates(a: f64, c: f64) -> Vec<f64> {
    let mut out = Vec::new();
    if a == 0.0 || c == 0.0 {
        return out;
    }
    let divisors = |n: f64| -> Vec<f64> {
        let n = n.abs();
        if n.fract() != 0.0 || n > 10000.0 {
            return vec![n];
        }
        let n = n as i64;
        (1..=n).filter(|d| n % d == 0).map(|d| d as f64).collect()
    };
    for p in divisors(c) {
        for q in divisors(a) {
            let v = p / q;
            out.push(v);
            out.push(-v);
        }
    }
    out
}

fn op_integrate(
    expression: &str,
    var: &str,
    lower: Option<String>,
    upper: Option<String>,
) -> Result<Value, String> {
    let expr = body_expr(expression)?;
    let poly = to_poly(&expr, var).ok_or_else(|| {
        format!("integrate needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
    })?;
    if lower.is_none() != upper.is_none() {
        return Err("a definite integral needs both lower and upper bounds".to_string());
    }
    if let (Some(lo), Some(hi)) = (lower, upper) {
        let lo_val = parse_number(&lo, "lower bound")?;
        let hi_val = parse_number(&hi, "upper bound")?;
        let anti = poly_antiderivative(&poly);
        let value = poly_eval(&anti, hi_val) - poly_eval(&anti, lo_val);
        let result = if value.fract() == 0.0 && value.abs() < 9.0e15 {
            fmt_num(value)
        } else {
            format!("{value}")
        };
        return Ok(numeric_result("integrate", &result, Some(value)));
    }
    let anti = poly_antiderivative(&poly);
    Ok(numeric_result("integrate", &render_poly(&anti, var), None))
}

fn op_limit(expression: &str, var: &str, point: Option<String>) -> Result<Value, String> {
    let point =
        point.ok_or_else(|| "limit needs a point, e.g. point='0' or point='oo'".to_string())?;
    let expr = body_expr(expression)?;
    let at = parse_number(&point, "point")?;
    if let Some(poly) = to_poly(&expr, var) {
        if at.is_infinite() {
            let deg = poly_degree(&poly);
            if deg == 0 {
                return Ok(numeric_result("limit", &fmt_num(poly[0]), Some(poly[0])));
            }
            let lead = *poly.last().unwrap();
            let sign = if at.is_sign_positive() {
                lead.signum()
            } else {
                (-lead).signum() * if deg % 2 == 1 { -1.0 } else { 1.0 }
            };
            let result = if sign > 0.0 { "oo" } else { "-oo" };
            return Ok(json!({"operation": "limit", "result": result}));
        }
        let value = poly_eval(&poly, at);
        return Ok(numeric_result("limit", &fmt_num(value), Some(value)));
    }
    if at.is_infinite() {
        return Err(
            "limit at infinity needs a polynomial expression; this evaluator does not do general symbolic algebra"
                .to_string(),
        );
    }
    let mut env = constants_env();
    env.insert(var.to_string(), at);
    let value = eval(&expr, &env, &math_funcs())
        .map_err(|e| format!("could not evaluate the limit: {e}"))?;
    Ok(numeric_result("limit", &fmt_num(value), Some(value)))
}

fn op_series(
    expression: &str,
    var: &str,
    point: Option<String>,
    terms: i64,
) -> Result<Value, String> {
    let expr = body_expr(expression)?;
    let poly = to_poly(&expr, var).ok_or_else(|| {
        format!("series needs a polynomial expression in {var}; this evaluator does not do general symbolic algebra")
    })?;
    let at = match point {
        Some(p) => parse_number(&p, "point")?,
        None => 0.0,
    };
    if !at.is_finite() {
        return Err("series needs a finite point".to_string());
    }
    let count = terms.clamp(1, 20) as usize;
    // Taylor coefficients: f^(k)(at) / k!
    let mut deriv = poly.clone();
    let mut coeffs: Vec<f64> = Vec::new();
    let mut fact = 1.0;
    for k in 0..count {
        let value = poly_eval(&deriv, at);
        coeffs.push(value / fact);
        deriv = poly_derivative(&deriv);
        fact *= (k + 1) as f64;
    }
    Ok(numeric_result(
        "series",
        &render_series(&coeffs, var, at),
        None,
    ))
}

fn render_series(coeffs: &[f64], var: &str, at: f64) -> String {
    let base = if at == 0.0 {
        var.to_string()
    } else if at < 0.0 {
        format!("({var} + {})", fmt_num(-at))
    } else {
        format!("({var} - {})", fmt_num(at))
    };
    let mut out = String::new();
    let mut first = true;
    for (k, coeff) in coeffs.iter().enumerate() {
        if *coeff == 0.0 {
            continue;
        }
        let abs = coeff.abs();
        let term = if k == 0 {
            fmt_num(abs)
        } else if abs == 1.0 {
            if k == 1 {
                base.clone()
            } else {
                format!("{base}^{k}")
            }
        } else if k == 1 {
            format!("{}*{base}", fmt_num(abs))
        } else {
            format!("{}*{base}^{k}", fmt_num(abs))
        };
        if first {
            if *coeff < 0.0 {
                out.push('-');
            }
            out.push_str(&term);
            first = false;
        } else if *coeff < 0.0 {
            out.push_str(&format!(" - {term}"));
        } else {
            out.push_str(&format!(" + {term}"));
        }
    }
    if out.is_empty() {
        "0".to_string()
    } else {
        out
    }
}

fn op_eval(expression: &str, var: &str) -> Result<Value, String> {
    let expr = body_expr(expression)?;
    if let Some(poly) = to_poly(&expr, var) {
        if poly_degree(&poly) == 0 {
            let value = poly[0];
            return Ok(numeric_result("eval", &fmt_num(value), Some(value)));
        }
        return Ok(numeric_result("eval", &render_poly(&poly, var), None));
    }
    let env = constants_env();
    let value = eval(&expr, &env, &math_funcs()).map_err(|e| {
        format!(
            "could not evaluate {expression:?} numerically: {e}. Give a plain arithmetic expression, or a polynomial in {var}."
        )
    })?;
    Ok(numeric_result("eval", &fmt_num(value), Some(value)))
}
