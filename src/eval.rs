//! 評価器 (SPEC §6)。
//!
//! `loop` / `recur` は末尾位置チェック付きの制御フロー (`EvalErr::Recur`) で
//! TCO を実現する。`recur` は末尾位置 (loop/do/if/let などの末尾) でのみ許可され、
//! それ以外の位置では構文エラーになる。

use crate::env::Env;
use crate::persistent::{PHam, PSet, PVector};
use crate::printer::pr_str;
use crate::types::{list, strip_meta, MalError, MalFn, UserFn, Value, WithMetaValue};
use std::sync::Arc;
use std::time::Instant;

/// 評価エラー: 言語エラーと、loop/recur の制御フロー。
pub enum EvalErr {
    Mal(MalError),
    Recur(Vec<Value>),
}

impl From<MalError> for EvalErr {
    fn from(e: MalError) -> Self {
        EvalErr::Mal(e)
    }
}

/// トップレベル評価 (REPL / ファイル実行が使う)。
/// ここまで逃げてきた `recur` はエラーにする。
pub fn eval_top(env: &Arc<Env>, form: &Value) -> Result<Value, MalError> {
    match eval(env, form, false) {
        Ok(v) => Ok(v),
        Err(EvalErr::Mal(e)) => Err(e),
        Err(EvalErr::Recur(_)) => Err(MalError::syntax("recur に対応する loop がありません")),
    }
}

/// 評価。`tail` は「この式が末尾位置にあるか」を表す (recur の合法性判定)。
pub fn eval(env: &Arc<Env>, form: &Value, tail: bool) -> Result<Value, EvalErr> {
    match form {
        Value::Symbol(name) => env.get(name).ok_or_else(|| MalError::unbound(name).into()),
        Value::List(list) => eval_list(env, list, tail),
        // コレクションは要素を評価して新しい永続コレクションを作る (SPEC §6.1-2)
        Value::Vector(vec) => {
            let mut out = PVector::empty();
            for e in vec.to_vec() {
                out = out.conj(eval(env, &e, false)?);
            }
            Ok(Value::Vector(Arc::new(out)))
        }
        Value::Map(map) => {
            let mut out = PHam::empty();
            for (k, v) in map.to_vec() {
                out = out.assoc(eval(env, &k, false)?, eval(env, &v, false)?);
            }
            Ok(Value::Map(Arc::new(out)))
        }
        Value::Set(set) => {
            let mut out = PSet::empty();
            for e in set.to_vec() {
                out = out.conj(eval(env, &e, false)?);
            }
            Ok(Value::Set(Arc::new(out)))
        }
        // 残り (nil / 真偽値 / 数値 / 文字列 / キーワード / 関数) は自己評価
        _ => Ok(form.clone()),
    }
}

fn eval_list(env: &Arc<Env>, list: &Option<Arc<crate::types::ListCell>>, tail: bool) -> Result<Value, EvalErr> {
    let Some(head_cell) = list else {
        // 空リストは自分自身に評価される (Clojure 準拠)
        return Ok(Value::List(None));
    };
    // 未評価の引数 (マクロ展開用)
    let mut raw: Vec<Value> = Vec::new();
    let mut cur = head_cell.tail.as_ref();
    while let Some(cell) = cur {
        raw.push(cell.head.clone());
        cur = cell.tail.as_ref();
    }
    if let Value::Symbol(name) = &head_cell.head {
        if let Some(special) = special_form(name) {
            return eval_special(env, special, &list::to_vec(list), tail);
        }
        // マクロ: シンボルがマクロに束縛されていれば、未評価の引数で展開して評価する
        if let Some(v) = env.get(name) {
            if let Value::MalFn(mf) = strip_meta(&v) {
                if let MalFn::Macro(uf) = &**mf {
                    let expansion = apply_user(uf, &raw)?;
                    return eval(env, &expansion, tail);
                }
            }
        }
    }
    let f = eval(env, &head_cell.head, false)?;
    let mut args = Vec::with_capacity(raw.len());
    for a in &raw {
        args.push(eval(env, a, false)?);
    }
    apply(&f, &args).map_err(EvalErr::Mal)
}

fn special_form(name: &str) -> Option<&str> {
    match name {
        "def" | "fn" | "defn" | "defmacro" | "let" | "if" | "do" | "quote" | "quasiquote"
        | "and" | "or" | "cond" | "when" | "loop" | "recur" | "time" | "dosync" | "future"
        | "try" | "macroexpand" | "macroexpand-1" => Some(name),
        _ => None,
    }
}

fn eval_special(env: &Arc<Env>, name: &str, list: &[Value], tail: bool) -> Result<Value, EvalErr> {
    match name {
        "def" => {
            if list.len() != 3 {
                return Err(MalError::arity("def は (def sym expr) の形です").into());
            }
            let Value::Symbol(sym) = &list[1] else {
                return Err(MalError::syntax("def の第 1 引数はシンボルである必要があります").into());
            };
            let v = eval(env, &list[2], false)?;
            env.set(sym.clone(), v);
            Ok(Value::Symbol(sym.clone()))
        }
        "fn" => {
            if list.len() < 3 {
                return Err(MalError::arity("fn には引数ベクタと body が必要です").into());
            }
            let (params, rest) = parse_params(&list[1])?;
            let body = list[2..].to_vec();
            Ok(Value::MalFn(Arc::new(MalFn::User(Arc::new(UserFn {
                params,
                rest,
                body,
                env: Arc::clone(env),
            })))))
        }
        "defn" => {
            if list.len() < 4 {
                return Err(MalError::arity("defn は (defn name [params] body...) の形です").into());
            }
            let spec = parse_def_spec(list)?;
            let mut f = Value::MalFn(Arc::new(MalFn::User(Arc::new(UserFn {
                params: spec.params,
                rest: spec.rest,
                body: spec.body,
                env: Arc::clone(env),
            }))));
            if let Some(doc) = spec.doc {
                f = attach_doc(f, doc);
            }
            env.set(spec.name.clone(), f);
            Ok(Value::Symbol(spec.name))
        }
        "defmacro" => {
            if list.len() < 4 {
                return Err(MalError::arity("defmacro は (defmacro name [params] body...) の形です").into());
            }
            let spec = parse_def_spec(list)?;
            let mut f = Value::MalFn(Arc::new(MalFn::Macro(Arc::new(UserFn {
                params: spec.params,
                rest: spec.rest,
                body: spec.body,
                env: Arc::clone(env),
            }))));
            if let Some(doc) = spec.doc {
                f = attach_doc(f, doc);
            }
            env.set(spec.name.clone(), f);
            Ok(Value::Symbol(spec.name))
        }
        "let" => {
            if list.len() < 3 {
                return Err(MalError::arity("let にはバインディングと body が必要です").into());
            }
            let Value::Vector(bindings) = &list[1] else {
                return Err(MalError::syntax("let の第 1 引数はバインディングのベクタである必要があります").into());
            };
            let bvec = bindings.to_vec();
            if !bvec.len().is_multiple_of(2) {
                return Err(MalError::syntax("let のバインディングは偶数個である必要があります").into());
            }
            // 並行バインディング: 右辺はすべて親環境で評価してからまとめて束縛する
            let mut pairs: Vec<(String, Value)> = Vec::new();
            let mut i = 0;
            while i < bvec.len() {
                let Value::Symbol(sym) = &bvec[i] else {
                    return Err(MalError::syntax("let のバインディング名はシンボルである必要があります").into());
                };
                let v = eval(env, &bvec[i + 1], false)?;
                pairs.push((sym.clone(), v));
                i += 2;
            }
            let child = Env::child(env);
            for (s, v) in pairs {
                child.set(s, v);
            }
            eval_body(&child, &list[2..], tail)
        }
        "if" => {
            if list.len() < 3 || list.len() > 4 {
                return Err(MalError::arity("if は (if cond then else?) の形です").into());
            }
            let c = eval(env, &list[1], false)?;
            if c.truthy() {
                eval(env, &list[2], tail)
            } else if list.len() == 4 {
                eval(env, &list[3], tail)
            } else {
                Ok(Value::Nil)
            }
        }
        "do" => eval_body(env, &list[1..], tail),
        "quote" => {
            if list.len() != 2 {
                return Err(MalError::arity("quote は (quote form) の形です").into());
            }
            Ok(list[1].clone())
        }
        "and" => {
            let n = list.len() - 1;
            if n == 0 {
                return Ok(Value::Bool(true));
            }
            for item in &list[1..n] {
                let v = eval(env, item, false)?;
                if !v.truthy() {
                    return Ok(v);
                }
            }
            eval(env, &list[n], tail)
        }
        "or" => {
            let n = list.len() - 1;
            if n == 0 {
                return Ok(Value::Nil);
            }
            for item in &list[1..n] {
                let v = eval(env, item, false)?;
                if v.truthy() {
                    return Ok(v);
                }
            }
            eval(env, &list[n], tail)
        }
        "cond" => {
            // (cond c1 e1 c2 e2 ...) 奇数個なら最後がデフォルト
            let forms = &list[1..];
            let n = forms.len();
            let mut i = 0;
            while i + 1 < n {
                let c = eval(env, &forms[i], false)?;
                if c.truthy() {
                    return eval(env, &forms[i + 1], tail);
                }
                i += 2;
            }
            if n % 2 == 1 {
                eval(env, &forms[n - 1], tail)
            } else {
                Ok(Value::Nil)
            }
        }
        "when" => {
            if list.len() < 3 {
                return Err(MalError::arity("when は (when cond body...) の形です").into());
            }
            let c = eval(env, &list[1], false)?;
            if c.truthy() {
                eval_body(env, &list[2..], tail)
            } else {
                Ok(Value::Nil)
            }
        }
        "loop" => {
            if list.len() < 3 {
                return Err(MalError::arity("loop にはバインディングと body が必要です").into());
            }
            let Value::Vector(bindings) = &list[1] else {
                return Err(MalError::syntax("loop の第 1 引数はバインディングのベクタである必要があります").into());
            };
            let bvec = bindings.to_vec();
            if !bvec.len().is_multiple_of(2) {
                return Err(MalError::syntax("loop のバインディングは偶数個である必要があります").into());
            }
            let mut names: Vec<String> = Vec::new();
            let mut values: Vec<Value> = Vec::new();
            let mut i = 0;
            while i < bvec.len() {
                let Value::Symbol(sym) = &bvec[i] else {
                    return Err(MalError::syntax("loop のバインディング名はシンボルである必要があります").into());
                };
                let v = eval(env, &bvec[i + 1], false)?;
                names.push(sym.clone());
                values.push(v);
                i += 2;
            }
            let body = &list[2..];
            if body.is_empty() {
                return Err(MalError::arity("loop には body が必要です").into());
            }
            let mut current = Env::child(env);
            for (s, v) in names.iter().zip(values) {
                current.set(s.clone(), v);
            }
            loop {
                match eval_body(&current, body, true) {
                    Err(EvalErr::Recur(args)) => {
                        if args.len() != names.len() {
                            return Err(MalError::arity(format!(
                                "recur の引数 {} 個が loop のバインディング数 {} 個と一致しません",
                                args.len(),
                                names.len()
                            ))
                            .into());
                        }
                        let next = Env::child(env);
                        for (s, a) in names.iter().zip(args) {
                            next.set(s.clone(), a);
                        }
                        current = next;
                    }
                    other => return other,
                }
            }
        }
        "recur" => {
            if !tail {
                return Err(MalError::syntax("recur は末尾位置でのみ使用できます").into());
            }
            let mut args = Vec::with_capacity(list.len() - 1);
            for a in &list[1..] {
                args.push(eval(env, a, false)?);
            }
            Err(EvalErr::Recur(args))
        }
        "time" => {
            if list.len() != 2 {
                return Err(MalError::arity("time は (time expr) の形です").into());
            }
            let t0 = Instant::now();
            let v = eval(env, &list[1], false)?;
            println!("Elapsed time: {:.3} msecs", t0.elapsed().as_secs_f64() * 1000.0);
            Ok(v)
        }
        "dosync" => {
            if list.len() < 2 {
                return Err(MalError::arity("dosync には body が必要です").into());
            }
            crate::stm::run_dosync(env, &list[1..]).map_err(EvalErr::Mal)
        }
        "future" => {
            if list.len() < 2 {
                return Err(MalError::arity("future には body が必要です").into());
            }
            Ok(Value::Future(crate::stm::Future::spawn(
                Arc::clone(env),
                list[1..].to_vec(),
            )))
        }
        "quasiquote" => {
            if list.len() != 2 {
                return Err(MalError::arity("quasiquote は (quasiquote form) の形です").into());
            }
            // 生成したコードを評価する。unquote 部分はこの環境で解決される
            // (マクロの引数はここで展開される)。
            let code = quasiquote(&list[1]);
            eval(env, &code, false)
        }
        "try" => {
            if list.len() < 2 {
                return Err(MalError::arity("try には body が必要です").into());
            }
            eval_try(env, &list[1..], tail)
        }
        "macroexpand-1" => {
            if list.len() != 2 {
                return Err(MalError::arity("macroexpand-1 は 1 引数です").into());
            }
            match macroexpand_once(env, &list[1])? {
                Some(e) => Ok(e),
                None => Ok(list[1].clone()),
            }
        }
        "macroexpand" => {
            if list.len() != 2 {
                return Err(MalError::arity("macroexpand は 1 引数です").into());
            }
            let mut form = list[1].clone();
            while let Some(e) = macroexpand_once(env, &form)? {
                form = e;
            }
            Ok(form)
        }
        _ => Err(MalError::internal(format!("未知の特殊形式: {}", name)).into()),
    }
}

/// fn / defn の引数リストを解析する。`[a b & rest]` の可変長に対応。
fn parse_params(form: &Value) -> Result<(Vec<String>, Option<String>), MalError> {
    let Value::Vector(ps) = form else {
        return Err(MalError::syntax("fn の引数はベクタである必要があります"));
    };
    let pvec = ps.to_vec();
    let mut params = Vec::new();
    let mut rest = None;
    let mut i = 0;
    while i < pvec.len() {
        match &pvec[i] {
            Value::Symbol(s) if s == "&" => {
                if i + 1 >= pvec.len() {
                    return Err(MalError::syntax("& の後には rest 引数名が必要です"));
                }
                if i + 2 != pvec.len() {
                    return Err(MalError::syntax("& は最後の引数にのみ使用できます"));
                }
                let Value::Symbol(r) = &pvec[i + 1] else {
                    return Err(MalError::syntax("& の後はシンボルである必要があります"));
                };
                rest = Some(r.clone());
                break;
            }
            Value::Symbol(s) => params.push(s.clone()),
            _ => return Err(MalError::syntax("引数名はシンボルである必要があります")),
        }
        i += 1;
    }
    Ok((params, rest))
}

/// defn / defmacro の共通解析。`(defn name "doc"? [params] body...)` の形。
struct DefSpec {
    name: String,
    doc: Option<String>,
    params: Vec<String>,
    rest: Option<String>,
    body: Vec<Value>,
}

fn parse_def_spec(list: &[Value]) -> Result<DefSpec, MalError> {
    let Value::Symbol(name_sym) = &list[1] else {
        return Err(MalError::syntax("名前はシンボルである必要があります"));
    };
    let mut idx = 2;
    let mut doc = None;
    if let Value::Str(d) = &list[2] {
        doc = Some(d.clone());
        idx = 3;
    }
    if idx >= list.len() {
        return Err(MalError::arity("引数ベクタと body が必要です"));
    }
    let (params, rest) = parse_params(&list[idx])?;
    let body = list[idx + 1..].to_vec();
    if body.is_empty() {
        return Err(MalError::arity("body が必要です"));
    }
    Ok(DefSpec { name: name_sym.clone(), doc, params, rest, body })
}

/// ドキュメント文字列を :doc メタデータとして付与する (Clojure 準拠)。
fn attach_doc(v: Value, doc: String) -> Value {
    let meta = PHam::empty().assoc(Value::Keyword("doc".to_string()), Value::Str(doc));
    Value::WithMeta(Arc::new(WithMetaValue { value: v, meta: Arc::new(meta) }))
}

/// マクロが 1 回展開できれば Some(展開結果)、できなければ None。
/// 特殊形式の先頭は展開しない (Clojure 準拠)。
fn macroexpand_once(env: &Arc<Env>, form: &Value) -> Result<Option<Value>, MalError> {
    let Value::List(l) = form else {
        return Ok(None);
    };
    let Some(head_cell) = l else {
        return Ok(None);
    };
    let Value::Symbol(name) = &head_cell.head else {
        return Ok(None);
    };
    if special_form(name).is_some() {
        return Ok(None);
    }
    let Some(v) = env.get(name) else {
        return Ok(None);
    };
    let Value::MalFn(mf) = strip_meta(&v) else {
        return Ok(None);
    };
    let MalFn::Macro(uf) = &**mf else {
        return Ok(None);
    };
    let raw = list::to_vec(l).into_iter().skip(1).collect::<Vec<_>>();
    let expansion = apply_user(uf, &raw)?;
    Ok(Some(expansion))
}

// ---------------------------------------------------------------------------
// quasiquote (Phase 4): マクロでコードを組み立てるための糖衣
// ---------------------------------------------------------------------------

/// `` `(a ~b ~@c) `` を `(concat (list (quote a)) (list b) c)` に変換する。
fn quasiquote(form: &Value) -> Value {
    match form {
        Value::List(l) => {
            if list::is_empty(l) {
                return Value::List(None);
            }
            let items = list::to_vec(l);
            // (unquote x) → x
            if items.len() == 2 && matches!(&items[0], Value::Symbol(s) if s == "unquote") {
                return items[1].clone();
            }
            // (concat (list ...) ...) を構築
            let mut parts = vec![Value::Symbol("concat".to_string())];
            for item in &items {
                let Value::List(il) = item else {
                    parts.push(list_of(vec![
                        Value::Symbol("list".to_string()),
                        quasiquote(item),
                    ]));
                    continue;
                };
                let ii = list::to_vec(il);
                if ii.len() == 2 && matches!(&ii[0], Value::Symbol(s) if s == "unquote") {
                    parts.push(list_of(vec![Value::Symbol("list".to_string()), ii[1].clone()]));
                } else if ii.len() == 2 && matches!(&ii[0], Value::Symbol(s) if s == "unquote-splicing") {
                    parts.push(ii[1].clone());
                } else {
                    parts.push(list_of(vec![
                        Value::Symbol("list".to_string()),
                        quasiquote(item),
                    ]));
                }
            }
            list_of(parts)
        }
        Value::Vector(v) => {
            let inner = quasiquote(&Value::List(list::from_vec(v.to_vec())));
            list_of(vec![Value::Symbol("vec".to_string()), inner])
        }
        Value::Map(m) => {
            let mut flat = Vec::new();
            for (k, v) in m.to_vec() {
                flat.push(k);
                flat.push(v);
            }
            let inner = quasiquote(&Value::List(list::from_vec(flat)));
            list_of(vec![
                Value::Symbol("apply".to_string()),
                Value::Symbol("hash-map".to_string()),
                inner,
            ])
        }
        Value::Set(s) => {
            let inner = quasiquote(&Value::List(list::from_vec(s.to_vec())));
            list_of(vec![
                Value::Symbol("apply".to_string()),
                Value::Symbol("set".to_string()),
                inner,
            ])
        }
        // 原子は quote で包む
        _ => list_of(vec![Value::Symbol("quote".to_string()), form.clone()]),
    }
}

fn list_of(v: Vec<Value>) -> Value {
    Value::List(list::from_vec(v))
}

// ---------------------------------------------------------------------------
// try / catch / finally (Phase 4)
// ---------------------------------------------------------------------------

/// `(try body... (catch e body...)? (finally body...)?)` を評価する。
/// catch のエラー変数には `{:message ... :kind ...}` のマップを束縛する。
fn eval_try(env: &Arc<Env>, forms: &[Value], tail: bool) -> Result<Value, EvalErr> {
    let mut end = forms.len();
    let mut finally: Option<Vec<Value>> = None;
    let mut catch: Option<(String, Vec<Value>)> = None;
    // 末尾から (catch ...) / (finally ...) を取り出す (順序はどちらでもよい)
    while end > 0 {
        let Some((kind, items)) = extract_try_clause(&forms[end - 1]) else {
            break;
        };
        match kind {
            "catch" if catch.is_none() => {
                if items.len() < 2 {
                    return Err(MalError::syntax("catch にはエラー変数と body が必要です").into());
                }
                let Value::Symbol(sym) = &items[1] else {
                    return Err(MalError::syntax("catch の第 1 引数はシンボルである必要があります").into());
                };
                catch = Some((sym.clone(), items[2..].to_vec()));
                end -= 1;
            }
            "finally" if finally.is_none() => {
                finally = Some(items[1..].to_vec());
                end -= 1;
            }
            _ => break,
        }
        if catch.is_some() && finally.is_some() {
            break;
        }
    }
    let body = &forms[..end];
    match eval_body(env, body, tail) {
        Ok(v) => {
            if let Some(fb) = &finally {
                let _ = eval_body(env, fb, false);
            }
            Ok(v)
        }
        Err(EvalErr::Mal(e)) => {
            if let Some((sym, cb)) = &catch {
                let err_value = error_to_value(&e);
                let child = Env::child(env);
                child.set(sym.clone(), err_value);
                let r = eval_body(&child, cb, tail);
                if let Some(fb) = &finally {
                    let _ = eval_body(env, fb, false);
                }
                r
            } else {
                if let Some(fb) = &finally {
                    let _ = eval_body(env, fb, false);
                }
                Err(EvalErr::Mal(e))
            }
        }
        Err(e) => {
            // recur 制御フローは catch できないが finally は実行する
            if let Some(fb) = &finally {
                let _ = eval_body(env, fb, false);
            }
            Err(e)
        }
    }
}

/// (catch ...) / (finally ...) の形なら (種別, 要素) を返す。
fn extract_try_clause(form: &Value) -> Option<(&'static str, Vec<Value>)> {
    let Value::List(l) = form else {
        return None;
    };
    let items = list::to_vec(l);
    let Value::Symbol(s) = items.first()? else {
        return None;
    };
    match s.as_str() {
        "catch" => Some(("catch", items)),
        "finally" => Some(("finally", items)),
        _ => None,
    }
}

/// エラーを catch に束縛するマップ値に変換する。
fn error_to_value(e: &MalError) -> Value {
    let m = PHam::empty()
        .assoc(Value::Keyword("message".to_string()), Value::Str(e.message.clone()))
        .assoc(Value::Keyword("kind".to_string()), Value::Keyword(e.kind.name().to_string()));
    Value::Map(Arc::new(m))
}

/// body を暗黙の `do` として評価する。最後の式だけ `tail` を引き継ぐ。
fn eval_body(env: &Arc<Env>, body: &[Value], tail: bool) -> Result<Value, EvalErr> {
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    for f in &body[..body.len() - 1] {
        eval(env, f, false)?;
    }
    eval(env, &body[body.len() - 1], tail)
}

/// stm モジュール (dosync / future) から使うための公開ラッパ。
pub fn eval_body_pub(env: &Arc<Env>, body: &[Value], tail: bool) -> Result<Value, EvalErr> {
    eval_body(env, body, tail)
}

/// 関数を引数に適用する。
pub fn apply(f: &Value, args: &[Value]) -> Result<Value, MalError> {
    match f {
        // メタデータは透過 (meta / with-meta 以外は組み込み関数にメタを見せない)
        Value::WithMeta(w) => apply(&w.value, args),
        // キーワードは関数としてマップの検索に使える (Clojure 準拠): (:k m)
        Value::Keyword(k) => {
            if args.is_empty() {
                return Err(MalError::arity("キーワード関数には引数が必要です"));
            }
            let key = Value::Keyword(k.clone());
            match &args[0] {
                Value::Nil => Ok(Value::Nil),
                Value::Map(m) => Ok(m.get(&key).unwrap_or(Value::Nil)),
                _ => Err(MalError::type_err("キーワード関数の第 1 引数はマップです")),
            }
        }
        Value::MalFn(mf) => match &**mf {
            MalFn::Builtin { name, func } => {
                if *name == "meta" {
                    // meta だけは生の値 (メタ付き) を見る必要がある
                    func(args)
                } else {
                    func(&strip_args(args))
                }
            }
            MalFn::User(uf) => apply_user(uf, args),
            MalFn::Macro(_) => {
                Err(MalError::syntax("マクロを関数として適用できません (defmacro は展開専用です)"))
            }
            MalFn::Partial { f, fixed } => {
                let mut all = fixed.clone();
                all.extend_from_slice(args);
                apply(f, &all)
            }
            MalFn::Comp { fns } => {
                if fns.is_empty() {
                    return args
                        .first()
                        .cloned()
                        .ok_or_else(|| MalError::arity("(comp) には引数が 1 つ必要です"));
                }
                let mut result = apply(&fns[fns.len() - 1], args)?;
                for i in (0..fns.len() - 1).rev() {
                    result = apply(&fns[i], &[result])?;
                }
                Ok(result)
            }
            MalFn::Constantly(v) => Ok(v.clone()),
        },
        _ => Err(MalError::type_err(format!("関数ではありません: {}", pr_str(f)))),
    }
}

/// 組み込み関数に渡す引数のメタデータを剥がす。
fn strip_args(args: &[Value]) -> Vec<Value> {
    args.iter().map(|a| strip_meta(a).clone()).collect()
}

/// ユーザー定義関数の適用。fn 本体は末尾位置扱いで評価し、
/// 末尾の `recur` は fn のパラメータを再束縛する (Clojure 準拠、SPEC §6.2)。
fn apply_user(uf: &UserFn, args: &[Value]) -> Result<Value, MalError> {
    let mut call_env = bind_env(uf, args)?;
    loop {
        match eval_body(&call_env, &uf.body, true) {
            Ok(v) => return Ok(v),
            Err(EvalErr::Mal(m)) => return Err(m),
            Err(EvalErr::Recur(recur_args)) => {
                call_env = bind_env(uf, &recur_args)?;
            }
        }
    }
}

/// arity を検証し、パラメータを束縛した呼び出し環境を返す。
fn bind_env(uf: &UserFn, args: &[Value]) -> Result<Arc<Env>, MalError> {
    match &uf.rest {
        None if args.len() != uf.params.len() => {
            return Err(MalError::arity(format!(
                "引数の数が一致しません: 期待 {} 個, 実際 {} 個",
                uf.params.len(),
                args.len()
            )));
        }
        Some(_) if args.len() < uf.params.len() => {
            return Err(MalError::arity(format!(
                "引数の数が足りません: 期待 {} 個以上, 実際 {} 個",
                uf.params.len(),
                args.len()
            )));
        }
        _ => {}
    }
    let call_env = Env::child(&uf.env);
    for (p, a) in uf.params.iter().zip(args.iter()) {
        call_env.set(p.clone(), a.clone());
    }
    if let Some(r) = &uf.rest {
        call_env.set(r.clone(), Value::List(list::from_vec(args[uf.params.len()..].to_vec())));
    }
    Ok(call_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::default_env;
    use crate::printer::pr_str;

    fn eval_str(src: &str) -> Result<Value, MalError> {
        let env = default_env();
        let mut result = Value::Nil;
        for form in crate::reader::read_forms(src)? {
            result = eval_top(&env, &form)?;
        }
        Ok(result)
    }

    #[test]
    fn fib_loop_recur() {
        // TCO: 深いループでもスタックを消費しない
        let v = eval_str(
            "(defn fib [n] (loop [i 0 a 0 b 1] (if (= i n) a (recur (inc i) b (+ a b))))) (fib 30)",
        )
        .unwrap();
        assert_eq!(pr_str(&v), "832040");
    }

    #[test]
    fn recur_outside_tail_is_error() {
        let e = eval_str("(loop [x 0] (+ 1 (recur (inc x))))").unwrap_err();
        assert_eq!(e.kind, crate::types::ErrorKind::Syntax);
    }

    #[test]
    fn parallel_let() {
        // 並行バインディング: 右辺は束縛前の環境で評価される
        let v = eval_str("(def a 1) (let [a 2 b a] b)").unwrap();
        assert_eq!(pr_str(&v), "1");
    }

    #[test]
    fn variadic_fn() {
        let v = eval_str("((fn [a & rest] rest) 1 2 3)").unwrap();
        assert_eq!(pr_str(&v), "(2 3)");
    }
}
