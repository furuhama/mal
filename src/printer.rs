//! プリンター (SPEC §5)。読み取り可能表現 (`pr-str`) と表示用表現 (`str` / `print`)。

use crate::types::{MalFn, Value};

/// 読み取り可能表現。`read(print(x)) == x` をラウンドトリップ保証する。
pub fn pr_str(v: &Value) -> String {
    match v {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Int(i) => i.to_string(),
        Value::Float(f) => pr_float(*f),
        Value::Str(s) => escape(s),
        Value::Keyword(k) => format!(":{}", k),
        Value::Symbol(s) => s.clone(),
        Value::List(l) => format!("({})", join(l)),
        Value::Vector(v) => format!("[{}]", join(v)),
        Value::Map(m) => {
            let mut parts = Vec::with_capacity(m.len() * 2);
            for (k, v) in m.iter() {
                parts.push(pr_str(k));
                parts.push(pr_str(v));
            }
            format!("{{{}}}", parts.join(" "))
        }
        Value::Set(s) => format!("#{{{}}}", join(s)),
        Value::MalFn(f) => match &**f {
            MalFn::Builtin { name, .. } => format!("#<builtin {}>", name),
            _ => "#<fn>".to_string(),
        },
    }
}

fn join(v: &[Value]) -> String {
    v.iter().map(pr_str).collect::<Vec<_>>().join(" ")
}

/// 整数値を表す浮動小数は "1.0" のように出力してラウンドトリップを保つ。
fn pr_float(f: f64) -> String {
    let s = format!("{}", f);
    if s.contains('.') || s.contains('e') || s.contains('E') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{}.0", s)
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 表示用表現 (`print` / `println`)。文字列はそのまま出力する。
pub fn display_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        _ => pr_str(v),
    }
}
