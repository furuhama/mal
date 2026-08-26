//! 値の表現とエラー型 (SPEC §3, §6.4)。
//!
//! Phase 1 ではコレクションは単純な `Vec` ベースで保持する。
//! Phase 2 で永続データ構造に置き換える (挙動は同一)。

use crate::env::Env;
use std::fmt;
use std::rc::Rc;

/// Lisp の値。
#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Keyword(String),
    Symbol(String),
    List(Rc<Vec<Value>>),
    Vector(Rc<Vec<Value>>),
    Map(Rc<Vec<(Value, Value)>>),
    Set(Rc<Vec<Value>>),
    MalFn(Rc<MalFn>),
}

/// 関数 (SPEC §3「関数」)。
#[derive(Debug)]
pub enum MalFn {
    /// 組み込み関数。
    Builtin {
        name: &'static str,
        func: fn(&[Value]) -> Result<Value, MalError>,
    },
    /// ユーザー定義関数 (クロージャ)。
    User(Rc<UserFn>),
    /// `partial` が生成する部分適用関数。
    Partial { f: Value, fixed: Vec<Value> },
    /// `comp` が生成する合成関数。
    Comp { fns: Vec<Value> },
    /// `constantly` が生成する定数関数。
    Constantly(Value),
}

/// ユーザー定義関数の中身。
#[derive(Debug)]
pub struct UserFn {
    pub params: Vec<String>,
    pub rest: Option<String>,
    pub body: Vec<Value>,
    pub env: Rc<Env>,
}

impl Value {
    /// Clojure の真偽規則 (SPEC §3.1): `false` と `nil` のみ偽。
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
}

/// 深い等価性。`=` 組み込み関数とマップキー検索に使う (SPEC §3.2, §6.3 補足)。
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Int(x), Value::Float(y)) => (*x as f64) == *y,
        (Value::Float(x), Value::Int(y)) => *x == (*y as f64),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::List(x), Value::List(y)) => vecs_equal(x, y),
        (Value::Vector(x), Value::Vector(y)) => vecs_equal(x, y),
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| values_equal(k, k2) && values_equal(v, v2)))
        }
        (Value::Set(x), Value::Set(y)) => {
            x.len() == y.len() && x.iter().all(|e| y.iter().any(|e2| values_equal(e, e2)))
        }
        (Value::MalFn(x), Value::MalFn(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn vecs_equal(x: &[Value], y: &[Value]) -> bool {
    x.len() == y.len() && x.iter().zip(y).all(|(a, b)| values_equal(a, b))
}

// ---------------------------------------------------------------------------
// エラー (SPEC §6.4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Reader,   // 読み取りエラー
    Unbound,  // 未束縛シンボル
    Arity,    // 引数エラー
    Type,     // 型エラー
    Range,    // 範囲外アクセス
    Syntax,   // 構文エラー (recur の位置など)
    #[allow(dead_code)] // Phase 3 で使用
    Stm,      // STM エラー
    Internal, // 内部エラー
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ErrorKind::Reader => "読み取りエラー",
            ErrorKind::Unbound => "未束縛シンボル",
            ErrorKind::Arity => "引数エラー",
            ErrorKind::Type => "型エラー",
            ErrorKind::Range => "範囲外アクセス",
            ErrorKind::Syntax => "構文エラー",
            ErrorKind::Stm => "STM エラー",
            ErrorKind::Internal => "内部エラー",
        };
        f.write_str(s)
    }
}

/// 言語エラー。`eof` は「入力が途中で終わった」ことを示す
/// (REPL が続きの行を読むためのヒント)。
#[derive(Debug)]
pub struct MalError {
    pub kind: ErrorKind,
    pub message: String,
    pub eof: bool,
}

impl MalError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        MalError { kind, message: message.into(), eof: false }
    }
    pub fn reader(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Reader, message)
    }
    pub fn reader_eof(message: impl Into<String>) -> Self {
        MalError { kind: ErrorKind::Reader, message: message.into(), eof: true }
    }
    pub fn unbound(name: &str) -> Self {
        Self::new(ErrorKind::Unbound, format!("{} は未定義です", name))
    }
    pub fn arity(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Arity, message)
    }
    pub fn type_err(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Type, message)
    }
    pub fn range(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Range, message)
    }
    pub fn syntax(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Syntax, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }
}

impl fmt::Display for MalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}
