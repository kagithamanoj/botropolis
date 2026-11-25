//! A small expression parser and evaluator, shared by the calculator
//! and the math tool.
//!
//! Grammar: numbers, names, unary minus, binary `+ - * / % ^`
//! (power is right-associative), parentheses, commas, and function
//! calls. No attribute access, no statements, nothing but arithmetic.
//! Evaluation takes an environment of variable values and resolves
//! function names through the caller-supplied table, so the calculator
//! (arithmetic only) and the math tool (trig, logs, constants) share
//! one parser with different name tables.

use std::collections::HashMap;

/// One parsed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Var(String),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let value: f64 = text
                .parse()
                .map_err(|_| format!("Invalid expression: bad number {text:?}"))?;
            tokens.push(Token {
                kind: TokenKind::Num(value),
            });
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            tokens.push(Token {
                kind: TokenKind::Ident(name),
            });
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' | '%' | '^' => {
                // `**` is the same power operator as `^`.
                if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
                    tokens.push(Token {
                        kind: TokenKind::Op('^'),
                    });
                    i += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Op(c),
                    });
                    i += 1;
                }
            }
            '(' => {
                tokens.push(Token {
                    kind: TokenKind::LParen,
                });
                i += 1;
            }
            ')' => {
                tokens.push(Token {
                    kind: TokenKind::RParen,
                });
                i += 1;
            }
            ',' => {
                tokens.push(Token {
                    kind: TokenKind::Comma,
                });
                i += 1;
            }
            _ => return Err(format!("Invalid expression: unexpected character {c:?}")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

fn precedence(op: char) -> u8 {
    match op {
        '+' | '-' => 1,
        '*' | '/' | '%' => 2,
        '^' => 3,
        _ => 0,
    }
}

impl Parser {
    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn next(&mut self) -> Option<TokenKind> {
        let tok = self.tokens.get(self.pos).map(|t| t.kind.clone());
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn parse_expr(&mut self, min_prec: u8) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        while let Some(TokenKind::Op(op)) = self.peek().cloned() {
            let prec = precedence(op);
            if prec == 0 || prec < min_prec {
                break;
            }
            self.next();
            // Power is right-associative; the rest are left-associative.
            let next_min = if op == '^' { prec } else { prec + 1 };
            let right = self.parse_expr(next_min)?;
            let binop = match op {
                '+' => BinOp::Add,
                '-' => BinOp::Sub,
                '*' => BinOp::Mul,
                '/' => BinOp::Div,
                '%' => BinOp::Mod,
                '^' => BinOp::Pow,
                _ => unreachable!(),
            };
            left = Expr::Bin(binop, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(TokenKind::Op('-')) => {
                self.next();
                Ok(Expr::Neg(Box::new(self.parse_unary()?)))
            }
            Some(TokenKind::Op('+')) => {
                self.next();
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(TokenKind::Num(v)) => Ok(Expr::Num(v)),
            Some(TokenKind::Ident(name)) => {
                if self.peek() == Some(&TokenKind::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if self.peek() != Some(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            match self.next() {
                                Some(TokenKind::Comma) => continue,
                                Some(TokenKind::RParen) => break,
                                _ => {
                                    return Err("Invalid expression: expected ',' or ')' in call"
                                        .to_string())
                                }
                            }
                        }
                    } else {
                        self.next();
                    }
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(TokenKind::LParen) => {
                let inner = self.parse_expr(0)?;
                match self.next() {
                    Some(TokenKind::RParen) => Ok(inner),
                    _ => Err("Invalid expression: missing closing ')'".to_string()),
                }
            }
            Some(other) => Err(format!("Invalid expression: unexpected {other:?}")),
            None => Err("Invalid expression: unexpected end of input".to_string()),
        }
    }
}

/// Parse an expression string into an AST.
pub fn parse(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_expr(0)?;
    if parser.pos != parser.tokens.len() {
        return Err("Invalid expression: trailing input".to_string());
    }
    Ok(expr)
}

/// A table of functions an expression may call.
pub type FuncTable = HashMap<String, fn(&[f64]) -> Result<f64, String>>;

/// Evaluate an AST with a variable environment and a function table.
pub fn eval(expr: &Expr, vars: &HashMap<String, f64>, funcs: &FuncTable) -> Result<f64, String> {
    match expr {
        Expr::Num(v) => Ok(*v),
        Expr::Var(name) => vars
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown name: {name}")),
        Expr::Neg(inner) => Ok(-eval(inner, vars, funcs)?),
        Expr::Bin(op, left, right) => {
            let l = eval(left, vars, funcs)?;
            let r = eval(right, vars, funcs)?;
            match op {
                BinOp::Add => Ok(l + r),
                BinOp::Sub => Ok(l - r),
                BinOp::Mul => Ok(l * r),
                BinOp::Div => {
                    if r == 0.0 {
                        Err("division by zero".to_string())
                    } else {
                        Ok(l / r)
                    }
                }
                BinOp::Mod => {
                    if r == 0.0 {
                        Err("division by zero".to_string())
                    } else {
                        Ok(l % r)
                    }
                }
                BinOp::Pow => Ok(l.powf(r)),
            }
        }
        Expr::Call(name, args) => {
            let func = funcs
                .get(name)
                .ok_or_else(|| format!("Function not allowed: {name}"))?;
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(eval(arg, vars, funcs)?);
            }
            func(&values)
        }
    }
}

fn one_arg(name: &str, args: &[f64]) -> Result<f64, String> {
    if args.len() != 1 {
        return Err(format!("{name} takes exactly one argument"));
    }
    Ok(args[0])
}

/// The function table the calculator allows: abs, round, min, max.
pub fn calculator_funcs() -> FuncTable {
    let mut funcs: FuncTable = HashMap::new();
    funcs.insert(
        "abs".to_string(),
        (|args: &[f64]| Ok(one_arg("abs", args)?.abs())) as fn(&[f64]) -> Result<f64, String>,
    );
    funcs.insert(
        "round".to_string(),
        (|args: &[f64]| {
            if args.len() == 1 {
                Ok(args[0].round())
            } else if args.len() == 2 {
                let places = args[1] as i32;
                let factor = 10f64.powi(places);
                Ok((args[0] * factor).round() / factor)
            } else {
                Err("round takes one or two arguments".to_string())
            }
        }) as fn(&[f64]) -> Result<f64, String>,
    );
    funcs.insert(
        "min".to_string(),
        (|args: &[f64]| {
            if args.is_empty() {
                return Err("min needs at least one argument".to_string());
            }
            Ok(args.iter().copied().fold(f64::INFINITY, f64::min))
        }) as fn(&[f64]) -> Result<f64, String>,
    );
    funcs.insert(
        "max".to_string(),
        (|args: &[f64]| {
            if args.is_empty() {
                return Err("max needs at least one argument".to_string());
            }
            Ok(args.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        }) as fn(&[f64]) -> Result<f64, String>,
    );
    funcs
}

/// Evaluate an arithmetic expression the way the calculator does:
/// numbers, operators, and abs/round/min/max only.
pub fn eval_calculator(expression: &str) -> Result<f64, String> {
    let expr = parse(expression)?;
    eval(&expr, &HashMap::new(), &calculator_funcs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval_calculator("2 + 3 * 4").unwrap(), 14.0);
        assert_eq!(eval_calculator("(2 + 3) * 4").unwrap(), 20.0);
        assert_eq!(eval_calculator("2 ^ 3 ^ 2").unwrap(), 512.0);
        assert_eq!(eval_calculator("-5 + 3").unwrap(), -2.0);
        assert_eq!(eval_calculator("7 % 3").unwrap(), 1.0);
        assert_eq!(eval_calculator("42 * 2").unwrap(), 84.0);
    }

    #[test]
    fn functions_and_errors() {
        assert_eq!(eval_calculator("abs(-4)").unwrap(), 4.0);
        assert_eq!(eval_calculator("max(1, 9, 3)").unwrap(), 9.0);
        assert!(eval_calculator("1 / 0").is_err());
        assert!(eval_calculator("x + 1").is_err());
        assert!(eval_calculator("sin(1)").is_err());
        assert!(eval_calculator("2 +").is_err());
    }
}
