//! 評価器 (SPEC §6)。
//!
//! `loop` / `recur` は末尾位置チェック付きの制御フロー (`EvalErr::Recur`) で
//! TCO を実現する。`recur` は末尾位置 (loop/do/if/let などの末尾) でのみ許可され、
//! それ以外の位置では構文エラーになる。

use crate::env::Env;
use crate::printer::pr_str;
use crate::types::{MalError, MalFn, UserFn, Value};
use std::rc::Rc;
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
pub fn eval_top(env: &Rc<Env>, form: &Value) -> Result<Value, MalError> {
    match eval(env, form, false) {
        Ok(v) => Ok(v),
        Err(EvalErr::Mal(e)) => Err(e),
        Err(EvalErr::Recur(_)) => Err(MalError::syntax("recur に対応する loop がありません")),
    }
}

/// 評価。`tail` は「この式が末尾位置にあるか」を表す (recur の合法性判定)。
pub fn eval(env: &Rc<Env>, form: &Value, tail: bool) -> Result<Value, EvalErr> {
    match form {
        Value::Symbol(name) => env.get(name).ok_or_else(|| MalError::unbound(name).into()),
        Value::List(list) => eval_list(env, list, tail),
        // コレクションは要素を評価する (SPEC §6.1-2)
        Value::Vector(vec) => {
            let mut out = Vec::with_capacity(vec.len());
            for e in vec.iter() {
                out.push(eval(env, e, false)?);
            }
            Ok(Value::Vector(Rc::new(out)))
        }
        Value::Map(map) => {
            let mut out = Vec::with_capacity(map.len());
            for (k, v) in map.iter() {
                out.push((eval(env, k, false)?, eval(env, v, false)?));
            }
            Ok(Value::Map(Rc::new(out)))
        }
        Value::Set(set) => {
            let mut out = Vec::with_capacity(set.len());
            for e in set.iter() {
                out.push(eval(env, e, false)?);
            }
            Ok(Value::Set(Rc::new(out)))
        }
        // 残り (nil / 真偽値 / 数値 / 文字列 / キーワード / 関数) は自己評価
        _ => Ok(form.clone()),
    }
}

fn eval_list(env: &Rc<Env>, list: &[Value], tail: bool) -> Result<Value, EvalErr> {
    if list.is_empty() {
        // 空リストは自分自身に評価される (Clojure 準拠)
        return Ok(Value::List(Rc::new(vec![])));
    }
    if let Value::Symbol(name) = &list[0] {
        if let Some(special) = special_form(name) {
            return eval_special(env, special, list, tail);
        }
    }
    let f = eval(env, &list[0], false)?;
    let mut args = Vec::with_capacity(list.len() - 1);
    for a in &list[1..] {
        args.push(eval(env, a, false)?);
    }
    apply(&f, &args).map_err(EvalErr::Mal)
}

fn special_form(name: &str) -> Option<&str> {
    match name {
        "def" | "fn" | "defn" | "let" | "if" | "do" | "quote" | "and" | "or" | "cond" | "when"
        | "loop" | "recur" | "time" => Some(name),
        _ => None,
    }
}

fn eval_special(env: &Rc<Env>, name: &str, list: &[Value], tail: bool) -> Result<Value, EvalErr> {
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
            Ok(Value::MalFn(Rc::new(MalFn::User(Rc::new(UserFn {
                params,
                rest,
                body,
                env: Rc::clone(env),
            })))))
        }
        "defn" => {
            if list.len() < 4 {
                return Err(MalError::arity("defn は (defn name [params] body...) の形です").into());
            }
            let Value::Symbol(name_sym) = &list[1] else {
                return Err(MalError::syntax("defn の第 1 引数はシンボルである必要があります").into());
            };
            let (params, rest) = parse_params(&list[2])?;
            let body = list[3..].to_vec();
            let f = Value::MalFn(Rc::new(MalFn::User(Rc::new(UserFn {
                params,
                rest,
                body,
                env: Rc::clone(env),
            }))));
            env.set(name_sym.clone(), f);
            Ok(Value::Symbol(name_sym.clone()))
        }
        "let" => {
            if list.len() < 3 {
                return Err(MalError::arity("let にはバインディングと body が必要です").into());
            }
            let Value::Vector(bindings) = &list[1] else {
                return Err(MalError::syntax("let の第 1 引数はバインディングのベクタである必要があります").into());
            };
            if bindings.len() % 2 != 0 {
                return Err(MalError::syntax("let のバインディングは偶数個である必要があります").into());
            }
            // 並行バインディング: 右辺はすべて親環境で評価してからまとめて束縛する
            let mut pairs: Vec<(String, Value)> = Vec::new();
            let mut i = 0;
            while i < bindings.len() {
                let Value::Symbol(sym) = &bindings[i] else {
                    return Err(MalError::syntax("let のバインディング名はシンボルである必要があります").into());
                };
                let v = eval(env, &bindings[i + 1], false)?;
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
            if bindings.len() % 2 != 0 {
                return Err(MalError::syntax("loop のバインディングは偶数個である必要があります").into());
            }
            let mut names: Vec<String> = Vec::new();
            let mut values: Vec<Value> = Vec::new();
            let mut i = 0;
            while i < bindings.len() {
                let Value::Symbol(sym) = &bindings[i] else {
                    return Err(MalError::syntax("loop のバインディング名はシンボルである必要があります").into());
                };
                let v = eval(env, &bindings[i + 1], false)?;
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
        _ => Err(MalError::internal(format!("未知の特殊形式: {}", name)).into()),
    }
}

/// fn / defn の引数リストを解析する。`[a b & rest]` の可変長に対応。
fn parse_params(form: &Value) -> Result<(Vec<String>, Option<String>), MalError> {
    let Value::Vector(ps) = form else {
        return Err(MalError::syntax("fn の引数はベクタである必要があります"));
    };
    let mut params = Vec::new();
    let mut rest = None;
    let mut i = 0;
    while i < ps.len() {
        match &ps[i] {
            Value::Symbol(s) if s == "&" => {
                if i + 1 >= ps.len() {
                    return Err(MalError::syntax("& の後には rest 引数名が必要です"));
                }
                if i + 2 != ps.len() {
                    return Err(MalError::syntax("& は最後の引数にのみ使用できます"));
                }
                let Value::Symbol(r) = &ps[i + 1] else {
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

/// body を暗黙の `do` として評価する。最後の式だけ `tail` を引き継ぐ。
fn eval_body(env: &Rc<Env>, body: &[Value], tail: bool) -> Result<Value, EvalErr> {
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    for f in &body[..body.len() - 1] {
        eval(env, f, false)?;
    }
    eval(env, &body[body.len() - 1], tail)
}

/// 関数を引数に適用する。
pub fn apply(f: &Value, args: &[Value]) -> Result<Value, MalError> {
    match f {
        Value::MalFn(mf) => match &**mf {
            MalFn::Builtin { func, .. } => func(args),
            MalFn::User(uf) => apply_user(uf, args),
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
fn bind_env(uf: &UserFn, args: &[Value]) -> Result<Rc<Env>, MalError> {
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
        call_env.set(r.clone(), Value::List(Rc::new(args[uf.params.len()..].to_vec())));
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
