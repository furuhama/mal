//! リーダー (SPEC §4)。文字列 → 式 (`Value`)。
//!
//! `'x` は `(quote x)`、`@x` は `(deref x)` の糖衣としてパーサ側で展開する。

use crate::types::{MalError, Value};
use std::rc::Rc;

/// トークン (テキストとバイト位置)。
#[derive(Debug, Clone)]
struct Token {
    text: String,
    offset: usize,
}

/// 文字列をトークン列に分割する。
fn tokenize(src: &str) -> Result<Vec<Token>, MalError> {
    let mut tokens = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() || c == ',' {
            chars.next();
            continue;
        }
        if c == ';' {
            // コメント: 行末まで読み飛ばす
            while let Some(&(_, c2)) = chars.peek() {
                if c2 == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        if "()[]{}'@".contains(c) {
            tokens.push(Token { text: c.to_string(), offset: start });
            chars.next();
            continue;
        }
        if c == '#' {
            // #{ のみセットリテラルとして対応する (SPEC §3)
            chars.next(); // '#' を消費
            match chars.peek() {
                Some(&(_, '{')) => {
                    chars.next();
                    tokens.push(Token { text: "#{".to_string(), offset: start });
                }
                _ => {
                    let (line, col) = line_col(src, start);
                    return Err(MalError::reader(format!("予期しない文字: # ({}行{}列)", line, col)));
                }
            }
            continue;
        }
        if c == '"' {
            let tok_start = start;
            chars.next(); // 開きの " を消費
            let mut text = String::from("\"");
            let mut closed = false;
            while let Some(&(_, c2)) = chars.peek() {
                let ch = c2;
                chars.next();
                text.push(ch);
                if ch == '\\' {
                    // エスケープ: 次の文字もトークンに含める
                    if let Some(&(_, c3)) = chars.peek() {
                        text.push(c3);
                        chars.next();
                    }
                } else if ch == '"' {
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(MalError::reader_eof("文字列が閉じられていません"));
            }
            tokens.push(Token { text, offset: tok_start });
            continue;
        }
        // 通常トークン: 区切り文字まで
        let tok_start = start;
        let mut text = String::new();
        while let Some(&(_, c2)) = chars.peek() {
            if c2.is_whitespace() || c2 == ',' || "()[]{}'@;\"".contains(c2) {
                break;
            }
            text.push(c2);
            chars.next();
        }
        tokens.push(Token { text, offset: tok_start });
    }
    Ok(tokens)
}

/// 文字列中の複数の式を順に読み取る。
pub fn read_forms(src: &str) -> Result<Vec<Value>, MalError> {
    let tokens = tokenize(src)?;
    let mut p = Parser { tokens, idx: 0, src };
    let mut forms = Vec::new();
    while !p.at_end() {
        forms.push(p.parse_form()?);
    }
    Ok(forms)
}

/// ちょうど 1 つの式を読み取る (read-string 組み込み関数が使う)。
pub fn read_str(src: &str) -> Result<Value, MalError> {
    let mut forms = read_forms(src)?;
    match forms.len() {
        0 => Err(MalError::reader("式がありません")),
        1 => Ok(forms.remove(0)),
        _ => Err(MalError::reader("1 つの式だけを指定してください (余分なトークンがあります)")),
    }
}

struct Parser<'a> {
    tokens: Vec<Token>,
    idx: usize,
    src: &'a str,
}

impl Parser<'_> {
    fn at_end(&self) -> bool {
        self.idx >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.idx)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.idx).cloned();
        if t.is_some() {
            self.idx += 1;
        }
        t
    }

    fn pos_err(&self, tok: &Token, msg: &str) -> MalError {
        let (line, col) = line_col(self.src, tok.offset);
        MalError::reader(format!("{} ({}行{}列)", msg, line, col))
    }

    fn parse_form(&mut self) -> Result<Value, MalError> {
        let tok = self.next().ok_or_else(|| MalError::reader_eof("式が途中で終わっています"))?;
        match tok.text.as_str() {
            "(" => self.parse_seq(')', |v| Value::List(crate::types::list::from_vec(v))),
            "[" => self.parse_seq(']', |v| Value::Vector(Rc::new(crate::persistent::PVector::from_vec(v)))),
            "{" => self.parse_map(),
            "#{" => self.parse_seq('}', |v| Value::Set(Rc::new(crate::persistent::PSet::from_vec(v)))),
            ")" | "]" | "}" => Err(self.pos_err(&tok, "対応する開き括弧がありません")),
            "'" => self.parse_sugar(&tok, "quote"),
            "@" => self.parse_sugar(&tok, "deref"),
            _ => self.parse_atom(&tok),
        }
    }

    /// `'x` → `(quote x)`、`@x` → `(deref x)`。
    fn parse_sugar(&mut self, tok: &Token, name: &str) -> Result<Value, MalError> {
        if self.at_end() {
            return Err(self.pos_err(tok, &format!("{} の対象となる式がありません", name)));
        }
        let form = self.parse_form()?;
        Ok(Value::List(crate::types::list::from_vec(vec![
            Value::Symbol(name.to_string()),
            form,
        ])))
    }

    fn parse_seq(&mut self, close: char, make: impl Fn(Vec<Value>) -> Value) -> Result<Value, MalError> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => return Err(MalError::reader_eof("対応する閉じ括弧がありません")),
                Some(t) if t.text == close.to_string() => {
                    self.next();
                    return Ok(make(items));
                }
                Some(t) if t.text == ")" || t.text == "]" || t.text == "}" => {
                    return Err(self.pos_err(t, "括弧の種類が一致しません"));
                }
                Some(_) => items.push(self.parse_form()?),
            }
        }
    }

    fn parse_map(&mut self) -> Result<Value, MalError> {
        let mut items: Vec<(Value, Value)> = Vec::new();
        loop {
            match self.peek() {
                None => return Err(MalError::reader_eof("対応する閉じ括弧がありません")),
                Some(t) if t.text == "}" => {
                    self.next();
                    return Ok(Value::Map(Rc::new(crate::persistent::PHam::from_vec(items))));
                }
                Some(t) if t.text == ")" || t.text == "]" => {
                    return Err(self.pos_err(t, "括弧の種類が一致しません"));
                }
                Some(_) => {
                    let k = self.parse_form()?;
                    let v = match self.peek() {
                        None => return Err(MalError::reader_eof("マップの値がありません")),
                        Some(t) if t.text == "}" => {
                            return Err(self.pos_err(t, "マップのキーに対応する値がありません"));
                        }
                        _ => self.parse_form()?,
                    };
                    items.push((k, v));
                }
            }
        }
    }

    fn parse_atom(&mut self, tok: &Token) -> Result<Value, MalError> {
        let s = tok.text.as_str();
        if s.starts_with('"') {
            // 文字列トークン: エスケープを解除する
            if s.len() < 2 || !s.ends_with('"') {
                return Err(self.pos_err(tok, "文字列が正しく閉じられていません"));
            }
            let inner = &s[1..s.len() - 1];
            return match unescape(inner) {
                Ok(v) => Ok(Value::Str(v)),
                Err(msg) => Err(self.pos_err(tok, &msg)),
            };
        }
        if s.starts_with("::") {
            return Err(self.pos_err(tok, ":: キーワードは非対応です"));
        }
        if s.starts_with(':') {
            if s.len() == 1 {
                return Err(self.pos_err(tok, "空のキーワードです"));
            }
            return Ok(Value::Keyword(s.strip_prefix(':').unwrap().to_string()));
        }
        if is_number_shape(s) {
            return match parse_number(s) {
                Some(v) => Ok(v),
                None => Err(self.pos_err(tok, "整数が i64 の範囲を超えています")),
            };
        }
        match s {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "nil" => Ok(Value::Nil),
            _ => Ok(Value::Symbol(s.to_string())),
        }
    }
}

/// `-?[0-9]+` または `-?[0-9]+\.[0-9]+` の形かどうか。
fn is_number_shape(s: &str) -> bool {
    let rest = s.strip_prefix('-').unwrap_or(s);
    let digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    match rest.find('.') {
        Some(dot) => digits(&rest[..dot]) && digits(&rest[dot + 1..]),
        None => digits(rest),
    }
}

fn parse_number(s: &str) -> Option<Value> {
    if s.contains('.') {
        s.parse::<f64>().ok().map(Value::Float)
    } else {
        s.parse::<i64>().ok().map(Value::Int)
    }
}

/// 文字列リテラルのエスケープ (`\"` `\\` `\n` `\t`) を解除する。
fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => return Err(format!("不明なエスケープ: \\{}", other)),
                None => return Err("末尾にエスケープがあります".to_string()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn line_col(src: &str, byte_offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, c) in src.char_indices() {
        if i >= byte_offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::pr_str;

    #[test]
    fn roundtrip() {
        for s in [
            "42",
            "-7",
            "3.14",
            "-0.5",
            "\"hello\"",
            "\"a\\nb\"",
            "\"say \\\"hi\\\"\"",
            "\"tab\\there\"",
            ":kw",
            "sym",
            "[1 2 3]",
            "{:a 1 :b 2}",
            "(1 2 3)",
            "#{1 2}",
            "(quote a)",
            "(fn [x] (+ x 1))",
        ] {
            let v = read_str(s).expect(s);
            assert_eq!(pr_str(&v), s, "ラウンドトリップ失敗: {}", s);
        }
    }

    #[test]
    fn sugar() {
        let v = read_str("'a").unwrap();
        assert_eq!(pr_str(&v), "(quote a)");
        let v = read_str("@x").unwrap();
        assert_eq!(pr_str(&v), "(deref x)");
    }

    #[test]
    fn comments() {
        let v = read_str("1 ; コメント\n2").unwrap_err();
        // 2 つ目の式が余分 → エラーになること自体を確認 (read_str は 1 式のみ)
        assert!(v.kind == crate::types::ErrorKind::Reader);
    }

    #[test]
    fn eof_flag() {
        let e = read_forms("(1 2").unwrap_err();
        assert!(e.eof, "閉じ括弧なしは eof フラグが立つべき");
        let e = read_forms("\"abc").unwrap_err();
        assert!(e.eof);
    }
}
