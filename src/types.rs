//! 値の表現とエラー型 (SPEC §3, §6.4)。
//!
//! Phase 2 より、コレクションは永続データ構造 (`persistent` モジュール) で保持する。
//! - リスト: cons セル (単方向連結リスト、先頭追加 O(1))
//! - ベクタ: 32-way 分岐トライ + tail (`PVector`)
//! - マップ: Array (≤8) → HAMT (`PHam`)
//! - セット: マップの値なし版 (`PSet`)

use crate::env::Env;
use crate::persistent::{ham_equal, set_equal, vector_equal};
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
    List(Option<Rc<ListCell>>),
    Vector(Rc<crate::persistent::PVector>),
    Map(Rc<crate::persistent::PHam>),
    Set(Rc<crate::persistent::PSet>),
    MalFn(Rc<MalFn>),
}

/// リストの cons セル。`None` が空リスト。
#[derive(Debug)]
pub struct ListCell {
    pub head: Value,
    pub tail: Option<Rc<ListCell>>,
}

/// リスト操作のヘルパ (Phase 2 より cons セル)。
pub mod list {
    use super::*;

    /// Vec から連結リストを構築する。
    pub fn from_vec(v: Vec<Value>) -> Option<Rc<ListCell>> {
        let mut l = None;
        for x in v.into_iter().rev() {
            l = Some(Rc::new(ListCell { head: x, tail: l }));
        }
        l
    }

    /// 連結リストを Vec に変換する。
    pub fn to_vec(l: &Option<Rc<ListCell>>) -> Vec<Value> {
        let mut out = Vec::new();
        let mut cur = l.as_ref();
        while let Some(cell) = cur {
            out.push(cell.head.clone());
            cur = cell.tail.as_ref();
        }
        out
    }

    pub fn len(l: &Option<Rc<ListCell>>) -> usize {
        let mut n = 0;
        let mut cur = l.as_ref();
        while let Some(cell) = cur {
            n += 1;
            cur = cell.tail.as_ref();
        }
        n
    }

    pub fn is_empty(l: &Option<Rc<ListCell>>) -> bool {
        l.is_none()
    }

    /// 先頭に追加 (O(1))。
    pub fn cons(head: Value, tail: Option<Rc<ListCell>>) -> Option<Rc<ListCell>> {
        Some(Rc::new(ListCell { head, tail }))
    }
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
        (Value::List(x), Value::List(y)) => list_equal(x, y),
        (Value::Vector(x), Value::Vector(y)) => vector_equal(x, y),
        (Value::Map(x), Value::Map(y)) => ham_equal(x, y),
        (Value::Set(x), Value::Set(y)) => set_equal(x, y),
        (Value::MalFn(x), Value::MalFn(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn list_equal(a: &Option<Rc<ListCell>>, b: &Option<Rc<ListCell>>) -> bool {
    let mut ca = a.as_ref();
    let mut cb = b.as_ref();
    loop {
        match (ca, cb) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if !values_equal(&x.head, &y.head) {
                    return false;
                }
                ca = x.tail.as_ref();
                cb = y.tail.as_ref();
            }
            _ => return false,
        }
    }
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
