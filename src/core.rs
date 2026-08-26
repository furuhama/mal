//! 組み込み関数 (SPEC §6.3)。

use crate::env::Env;
use crate::eval::apply as apply_fn;
use crate::persistent::{PHam, PSet, PVector};
use crate::printer::{display_str, pr_str};
use crate::reader::read_str;
use crate::types::{list, values_equal, MalError, MalFn, Value};
use std::cmp::Ordering;
use std::io::Write;
use std::sync::Arc;

type BuiltinFn = fn(&[Value]) -> Result<Value, MalError>;

/// 組み込み関数をすべて束縛した環境を返す。
pub fn default_env() -> Arc<Env> {
    let env = Env::new();
    const BUILTINS: &[(&str, BuiltinFn)] = &[
        // 算術
        ("+", add),
        ("-", sub),
        ("*", mul),
        ("/", div),
        ("quot", quot),
        ("rem", rem),
        ("inc", inc),
        ("dec", dec),
        ("max", max),
        ("min", min),
        ("abs", abs),
        // 比較
        ("=", eq),
        ("not=", neq),
        ("<", lt),
        (">", gt),
        ("<=", le),
        (">=", ge),
        // 述語
        ("nil?", p_nil),
        ("true?", p_true),
        ("false?", p_false),
        ("number?", p_number),
        ("int?", p_int),
        ("float?", p_float),
        ("string?", p_string),
        ("keyword?", p_keyword),
        ("symbol?", p_symbol),
        ("list?", p_list),
        ("vector?", p_vector),
        ("map?", p_map),
        ("set?", p_set),
        ("fn?", p_fn),
        ("seq?", p_seq),
        ("empty?", p_empty),
        ("pos?", p_pos),
        ("neg?", p_neg),
        ("zero?", p_zero),
        ("even?", p_even),
        ("odd?", p_odd),
        // リスト / シーケンス
        ("list", list),
        ("cons", cons),
        ("first", first),
        ("rest", rest),
        ("next", next),
        ("conj", conj),
        ("seq", seq),
        ("count", count),
        ("nth", nth),
        ("last", last),
        // ベクタ
        ("vector", vector),
        ("vec", vec),
        // マップ
        ("hash-map", hash_map),
        ("get", get),
        ("assoc", assoc),
        ("dissoc", dissoc),
        ("contains?", contains),
        ("keys", keys),
        ("vals", vals),
        ("merge", merge),
        // セット
        ("set", set),
        ("disj", disj),
        // 高階
        ("map", map),
        ("filter", filter),
        ("reduce", reduce),
        ("apply", apply_builtin),
        ("partial", partial),
        ("comp", comp),
        ("identity", identity),
        ("constantly", constantly),
        // 文字列 / 出力
        ("str", str_fn),
        ("pr-str", pr_str_fn),
        ("print", print_fn),
        ("println", println_fn),
        ("read-string", read_string),
        // 変換
        ("int", int),
        ("float", float),
        ("keyword", keyword),
        ("symbol", symbol),
        ("name", name),
        // STM (Phase 3, SPEC §8)
        ("atom", atom),
        ("deref", deref),
        ("swap!", swap_bang),
        ("reset!", reset_bang),
        ("ref", ref_fn),
        ("ref-set", ref_set),
        ("alter", alter),
        ("commute", commute),
        ("ensure", ensure),
        // Phase 4
        ("not", not_fn),
        ("concat", concat_fn),
        ("meta", meta_fn),
        ("with-meta", with_meta_fn),
        ("throw", throw_fn),
    ];
    for &(bname, func) in BUILTINS.iter() {
        env.set(bname.to_string(), Value::MalFn(Arc::new(MalFn::Builtin { name: bname, func })));
    }
    env
}

// ---------------------------------------------------------------------------
// ヘルパ
// ---------------------------------------------------------------------------

/// 整数を取り出す。整数以外は型エラー。
fn num_i(v: &Value, who: &str) -> Result<i64, MalError> {
    match v {
        Value::Int(i) => Ok(*i),
        _ => Err(MalError::type_err(format!("{} は整数を要求します: {}", who, pr_str(v)))),
    }
}

/// 数値 (整数または浮動小数) を f64 で取り出す。
fn num_f(v: &Value, who: &str) -> Result<f64, MalError> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(MalError::type_err(format!("{} は数値を要求します: {}", who, pr_str(v)))),
    }
}

/// 数値の順序比較。整数同士は i64 で、それ以外は f64 で比較する。
fn num_cmp(a: &Value, b: &Value, who: &str) -> Result<Ordering, MalError> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        _ => num_f(a, who)?
            .partial_cmp(&num_f(b, who)?)
            .ok_or_else(|| MalError::type_err(format!("{}: NaN は比較できません", who)))?,
    })
}

fn pred1(args: &[Value], f: impl Fn(&Value) -> bool) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("述語は 1 引数です"));
    }
    Ok(Value::Bool(f(&args[0])))
}

/// シーケンス (リスト・ベクタ・nil) の要素を取り出す。
fn seq_elements(v: &Value, who: &str) -> Result<Vec<Value>, MalError> {
    match v {
        Value::Nil => Ok(vec![]),
        Value::List(l) => Ok(list::to_vec(l)),
        Value::Vector(vv) => Ok(vv.to_vec()),
        _ => Err(MalError::type_err(format!("{} はシーケンスを要求します: {}", who, pr_str(v)))),
    }
}

// ---------------------------------------------------------------------------
// 算術
// ---------------------------------------------------------------------------

fn add(args: &[Value]) -> Result<Value, MalError> {
    if args.iter().any(|a| matches!(a, Value::Float(_))) {
        let mut acc = 0.0;
        for a in args {
            acc += num_f(a, "+")?;
        }
        Ok(Value::Float(acc))
    } else {
        let mut acc = 0i64;
        for a in args {
            acc = acc.checked_add(num_i(a, "+")?).ok_or_else(|| MalError::type_err("整数オーバーフロー"))?;
        }
        Ok(Value::Int(acc))
    }
}

fn sub(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("- には 1 つ以上の引数が必要です"));
    }
    if args.iter().any(|a| matches!(a, Value::Float(_))) {
        let mut acc = num_f(&args[0], "-")?;
        for a in &args[1..] {
            acc -= num_f(a, "-")?;
        }
        Ok(Value::Float(acc))
    } else {
        let first = num_i(&args[0], "-")?;
        if args.len() == 1 {
            return first.checked_neg().map(Value::Int).ok_or_else(|| MalError::type_err("整数オーバーフロー"));
        }
        let mut acc = first;
        for a in &args[1..] {
            acc = acc.checked_sub(num_i(a, "-")?).ok_or_else(|| MalError::type_err("整数オーバーフロー"))?;
        }
        Ok(Value::Int(acc))
    }
}

fn mul(args: &[Value]) -> Result<Value, MalError> {
    if args.iter().any(|a| matches!(a, Value::Float(_))) {
        let mut acc = 1.0;
        for a in args {
            acc *= num_f(a, "*")?;
        }
        Ok(Value::Float(acc))
    } else {
        let mut acc = 1i64;
        for a in args {
            acc = acc.checked_mul(num_i(a, "*")?).ok_or_else(|| MalError::type_err("整数オーバーフロー"))?;
        }
        Ok(Value::Int(acc))
    }
}

fn div(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("/ には 1 つ以上の引数が必要です"));
    }
    if args.iter().all(|a| matches!(a, Value::Int(_))) {
        let first = num_i(&args[0], "/")?;
        let mut acc = first;
        for a in &args[1..] {
            let d = num_i(a, "/")?;
            if d == 0 {
                return Err(MalError::type_err("ゼロ除算"));
            }
            if acc % d != 0 {
                // 割り切れない → 浮動小数に切り替えて計算し直す (SPEC §3.2)
                let mut f = first as f64;
                for a2 in &args[1..] {
                    f /= num_f(a2, "/")?;
                }
                return Ok(Value::Float(f));
            }
            acc /= d;
        }
        Ok(Value::Int(acc))
    } else {
        let mut acc = num_f(&args[0], "/")?;
        for a in &args[1..] {
            acc /= num_f(a, "/")?;
        }
        Ok(Value::Float(acc))
    }
}

fn quot(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("quot は (quot a b) の形です"));
    }
    let a = num_i(&args[0], "quot")?;
    let b = num_i(&args[1], "quot")?;
    if b == 0 {
        return Err(MalError::type_err("ゼロ除算"));
    }
    Ok(Value::Int(a / b))
}

fn rem(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("rem は (rem a b) の形です"));
    }
    let a = num_i(&args[0], "rem")?;
    let b = num_i(&args[1], "rem")?;
    if b == 0 {
        return Err(MalError::type_err("ゼロ除算"));
    }
    Ok(Value::Int(a % b))
}

fn inc(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("inc は 1 引数です"));
    }
    match &args[0] {
        Value::Int(i) => i.checked_add(1).map(Value::Int).ok_or_else(|| MalError::type_err("整数オーバーフロー")),
        Value::Float(f) => Ok(Value::Float(f + 1.0)),
        _ => Err(MalError::type_err("inc は数値を要求します")),
    }
}

fn dec(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("dec は 1 引数です"));
    }
    match &args[0] {
        Value::Int(i) => i.checked_sub(1).map(Value::Int).ok_or_else(|| MalError::type_err("整数オーバーフロー")),
        Value::Float(f) => Ok(Value::Float(f - 1.0)),
        _ => Err(MalError::type_err("dec は数値を要求します")),
    }
}

fn max(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("max には 1 つ以上の引数が必要です"));
    }
    let mut best = args[0].clone();
    for a in &args[1..] {
        if num_cmp(a, &best, "max")? == Ordering::Greater {
            best = a.clone();
        }
    }
    Ok(best)
}

fn min(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("min には 1 つ以上の引数が必要です"));
    }
    let mut best = args[0].clone();
    for a in &args[1..] {
        if num_cmp(a, &best, "min")? == Ordering::Less {
            best = a.clone();
        }
    }
    Ok(best)
}

fn abs(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("abs は 1 引数です"));
    }
    match &args[0] {
        Value::Int(i) => i.checked_abs().map(Value::Int).ok_or_else(|| MalError::type_err("整数オーバーフロー (i64::MIN)")),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        _ => Err(MalError::type_err("abs は数値を要求します")),
    }
}

// ---------------------------------------------------------------------------
// 比較
// ---------------------------------------------------------------------------

fn eq(args: &[Value]) -> Result<Value, MalError> {
    Ok(Value::Bool(args.windows(2).all(|w| values_equal(&w[0], &w[1]))))
}

fn neq(args: &[Value]) -> Result<Value, MalError> {
    Ok(Value::Bool(!args.windows(2).all(|w| values_equal(&w[0], &w[1]))))
}

fn lt(args: &[Value]) -> Result<Value, MalError> {
    for w in args.windows(2) {
        if num_cmp(&w[0], &w[1], "<")? != Ordering::Less {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn gt(args: &[Value]) -> Result<Value, MalError> {
    for w in args.windows(2) {
        if num_cmp(&w[0], &w[1], ">")? != Ordering::Greater {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn le(args: &[Value]) -> Result<Value, MalError> {
    for w in args.windows(2) {
        if num_cmp(&w[0], &w[1], "<=")? == Ordering::Greater {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn ge(args: &[Value]) -> Result<Value, MalError> {
    for w in args.windows(2) {
        if num_cmp(&w[0], &w[1], ">=")? == Ordering::Less {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// 述語
// ---------------------------------------------------------------------------

fn p_nil(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Nil))
}
fn p_true(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Bool(true)))
}
fn p_false(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Bool(false)))
}
fn p_number(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Int(_) | Value::Float(_)))
}
fn p_int(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Int(_)))
}
fn p_float(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Float(_)))
}
fn p_string(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Str(_)))
}
fn p_keyword(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Keyword(_)))
}
fn p_symbol(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Symbol(_)))
}
fn p_list(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::List(_)))
}
fn p_vector(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Vector(_)))
}
fn p_map(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Map(_)))
}
fn p_set(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::Set(_)))
}
fn p_fn(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::MalFn(_)))
}
/// seq? はリスト・ベクタで真 (シーケンスとして扱えるもの)。空でも真。
fn p_seq(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| matches!(v, Value::List(_) | Value::Vector(_)))
}
fn p_empty(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Nil => true,
        Value::List(l) => list::is_empty(l),
        Value::Vector(vv) => vv.is_empty(),
        Value::Map(m) => m.is_empty(),
        Value::Set(s) => s.is_empty(),
        Value::Str(s) => s.is_empty(),
        _ => false,
    })
}
fn p_pos(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Int(i) => *i > 0,
        Value::Float(f) => *f > 0.0,
        _ => false,
    })
}
fn p_neg(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Int(i) => *i < 0,
        Value::Float(f) => *f < 0.0,
        _ => false,
    })
}
fn p_zero(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Int(i) => *i == 0,
        Value::Float(f) => *f == 0.0,
        _ => false,
    })
}
fn p_even(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Int(i) => *i % 2 == 0,
        _ => false,
    })
}
fn p_odd(args: &[Value]) -> Result<Value, MalError> {
    pred1(args, |v| match v {
        Value::Int(i) => *i % 2 != 0,
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// リスト / シーケンス
// ---------------------------------------------------------------------------

fn list(args: &[Value]) -> Result<Value, MalError> {
    Ok(Value::List(list::from_vec(args.to_vec())))
}

fn cons(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("cons は (cons x coll) の形です"));
    }
    match &args[1] {
        // リストへの cons は O(1)
        Value::List(coll) => Ok(Value::List(list::cons(args[0].clone(), coll.clone()))),
        Value::Nil => Ok(Value::List(list::from_vec(vec![args[0].clone()]))),
        Value::Vector(v) => {
            let mut elems = v.to_vec();
            elems.insert(0, args[0].clone());
            Ok(Value::List(list::from_vec(elems)))
        }
        _ => Err(MalError::type_err("cons の第 2 引数はコレクションです")),
    }
}

fn first(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("first は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::List(l) => Ok(l.as_ref().map(|c| c.head.clone()).unwrap_or(Value::Nil)),
        Value::Vector(v) => Ok(v.get(0).unwrap_or(Value::Nil)),
        _ => Err(MalError::type_err("first はリスト・ベクタにのみ対応します")),
    }
}

fn rest(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("rest は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::List(None)),
        // リストの rest は tail を返すだけ (O(1))
        Value::List(l) => Ok(Value::List(l.as_ref().and_then(|c| c.tail.clone()))),
        Value::Vector(v) => Ok(Value::List(list::from_vec(v.to_vec().into_iter().skip(1).collect()))),
        _ => Err(MalError::type_err("rest はリスト・ベクタにのみ対応します")),
    }
}

fn next(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("next は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        // Clojure 準拠: 残りがない場合は nil
        Value::List(l) => match l.as_ref().and_then(|c| c.tail.as_ref()) {
            Some(_) => Ok(Value::List(l.as_ref().and_then(|c| c.tail.clone()))),
            None => Ok(Value::Nil),
        },
        Value::Vector(v) => {
            let elems = v.to_vec();
            if elems.len() > 1 {
                Ok(Value::List(list::from_vec(elems[1..].to_vec())))
            } else {
                Ok(Value::Nil)
            }
        }
        _ => Err(MalError::type_err("next はリスト・ベクタにのみ対応します")),
    }
}

fn last(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("last は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::List(l) => {
            let mut cur = l.as_ref();
            let mut last = None;
            while let Some(c) = cur {
                last = Some(c.head.clone());
                cur = c.tail.as_ref();
            }
            Ok(last.unwrap_or(Value::Nil))
        }
        Value::Vector(v) => {
            let len = v.len();
            if len == 0 {
                Ok(Value::Nil)
            } else {
                Ok(v.get(len - 1).unwrap_or(Value::Nil))
            }
        }
        _ => Err(MalError::type_err("last はリスト・ベクタにのみ対応します")),
    }
}

fn conj(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("conj にはコレクションが必要です"));
    }
    let (head, items) = args.split_first().unwrap();
    match head {
        Value::Nil => Ok(Value::List(list::from_vec(items.to_vec()))),
        Value::List(l) => {
            // Clojure 準拠: 先頭に順方向で追加 (最後の引数が先頭に来る)
            let mut out = l.clone();
            for x in items {
                out = list::cons(x.clone(), out);
            }
            Ok(Value::List(out))
        }
        Value::Vector(v) => {
            let mut out = (**v).clone();
            for x in items {
                out = out.conj(x.clone());
            }
            Ok(Value::Vector(Arc::new(out)))
        }
        Value::Map(m) => {
            let mut out = (**m).clone();
            for item in items {
                let Value::Vector(e) = item else {
                    return Err(MalError::type_err("マップへの conj は [k v] の形です"));
                };
                if e.len() != 2 {
                    return Err(MalError::type_err("マップへの conj は [k v] の形です"));
                }
                out = out.assoc(e.get(0).unwrap_or(Value::Nil), e.get(1).unwrap_or(Value::Nil));
            }
            Ok(Value::Map(Arc::new(out)))
        }
        Value::Set(s) => {
            let mut out = (**s).clone();
            for x in items {
                out = out.conj(x.clone());
            }
            Ok(Value::Set(Arc::new(out)))
        }
        _ => Err(MalError::type_err("conj はコレクションを要求します")),
    }
}

fn seq(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("seq は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::List(l) => {
            if l.is_none() {
                Ok(Value::Nil)
            } else {
                Ok(Value::List(l.clone()))
            }
        }
        Value::Vector(v) => {
            let elems = v.to_vec();
            if elems.is_empty() {
                Ok(Value::Nil)
            } else {
                Ok(Value::List(list::from_vec(elems)))
            }
        }
        _ => Err(MalError::type_err("seq はリスト・ベクタにのみ対応します")),
    }
}

fn count(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("count は 1 引数です"));
    }
    let n = match &args[0] {
        Value::Nil => 0,
        Value::List(l) => list::len(l),
        Value::Vector(v) => v.len(),
        Value::Map(m) => m.len(),
        Value::Set(s) => s.len(),
        Value::Str(s) => s.chars().count(),
        _ => return Err(MalError::type_err("count はコレクション・文字列にのみ対応します")),
    };
    Ok(Value::Int(n as i64))
}

fn nth(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("nth は (nth coll i) の形です"));
    }
    let i = num_i(&args[1], "nth")?;
    if i < 0 {
        return Err(MalError::range("nth のインデックスは非負である必要があります"));
    }
    let idx = i as usize;
    match &args[0] {
        Value::List(l) => {
            let mut cur = l.as_ref();
            let mut j = 0usize;
            while let Some(c) = cur {
                if j == idx {
                    return Ok(c.head.clone());
                }
                j += 1;
                cur = c.tail.as_ref();
            }
            Err(MalError::range(format!("nth: インデックス {} が範囲外 (長さ {})", i, list::len(l))))
        }
        Value::Vector(v) => v.get(idx).ok_or_else(|| {
            MalError::range(format!("nth: インデックス {} が範囲外 (長さ {})", i, v.len()))
        }),
        _ => Err(MalError::type_err("nth はリスト・ベクタにのみ対応します")),
    }
}

// ---------------------------------------------------------------------------
// ベクタ
// ---------------------------------------------------------------------------

fn vector(args: &[Value]) -> Result<Value, MalError> {
    Ok(Value::Vector(Arc::new(PVector::from_vec(args.to_vec()))))
}

fn vec(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("vec は 1 引数です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Vector(Arc::new(PVector::empty()))),
        Value::List(l) => Ok(Value::Vector(Arc::new(PVector::from_vec(list::to_vec(l))))),
        Value::Vector(v) => Ok(Value::Vector(Arc::clone(v))),
        _ => Err(MalError::type_err("vec はリスト・ベクタにのみ対応します")),
    }
}

// ---------------------------------------------------------------------------
// マップ
// ---------------------------------------------------------------------------

fn hash_map(args: &[Value]) -> Result<Value, MalError> {
    if !args.len().is_multiple_of(2) {
        return Err(MalError::arity("hash-map は偶数個の引数が必要です"));
    }
    let mut pairs = Vec::with_capacity(args.len() / 2);
    let mut i = 0;
    while i < args.len() {
        pairs.push((args[i].clone(), args[i + 1].clone()));
        i += 2;
    }
    Ok(Value::Map(Arc::new(PHam::from_vec(pairs))))
}

fn get(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("get は (get coll k) の形です"));
    }
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::Map(m) => Ok(m.get(&args[1]).unwrap_or(Value::Nil)),
        Value::Vector(v) => {
            if let Value::Int(i) = &args[1] {
                if *i >= 0 {
                    return Ok(v.get(*i as usize).unwrap_or(Value::Nil));
                }
            }
            Ok(Value::Nil)
        }
        _ => Err(MalError::type_err("get はマップ・ベクタにのみ対応します")),
    }
}

fn assoc(args: &[Value]) -> Result<Value, MalError> {
    if args.len() < 3 || args.len().is_multiple_of(2) {
        return Err(MalError::arity("assoc は (assoc m k v ...) の形です"));
    }
    let mut out: PHam = match &args[0] {
        Value::Nil => PHam::empty(),
        Value::Map(m) => (**m).clone(),
        _ => return Err(MalError::type_err("assoc の第 1 引数はマップです")),
    };
    let mut i = 1;
    while i < args.len() {
        out = out.assoc(args[i].clone(), args[i + 1].clone());
        i += 2;
    }
    Ok(Value::Map(Arc::new(out)))
}

fn dissoc(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("dissoc にはマップが必要です"));
    }
    let Value::Map(m) = &args[0] else {
        return Err(MalError::type_err("dissoc の第 1 引数はマップです"));
    };
    let mut out = (**m).clone();
    for k in &args[1..] {
        out = out.dissoc(k);
    }
    Ok(Value::Map(Arc::new(out)))
}

fn contains(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("contains? は (contains? coll k) の形です"));
    }
    match &args[0] {
        Value::Map(m) => Ok(Value::Bool(m.get(&args[1]).is_some())),
        Value::Set(s) => Ok(Value::Bool(s.contains(&args[1]))),
        _ => Err(MalError::type_err("contains? はマップ・セットにのみ対応します")),
    }
}

fn keys(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("keys は 1 引数です"));
    }
    let Value::Map(m) = &args[0] else {
        return Err(MalError::type_err("keys はマップにのみ対応します"));
    };
    Ok(Value::List(list::from_vec(
        m.to_vec().into_iter().map(|(k, _)| k).collect(),
    )))
}

fn vals(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("vals は 1 引数です"));
    }
    let Value::Map(m) = &args[0] else {
        return Err(MalError::type_err("vals はマップにのみ対応します"));
    };
    Ok(Value::List(list::from_vec(
        m.to_vec().into_iter().map(|(_, v)| v).collect(),
    )))
}

fn merge(args: &[Value]) -> Result<Value, MalError> {
    let mut out = PHam::empty();
    for a in args {
        let Value::Map(m) = a else {
            return Err(MalError::type_err("merge はマップのみ受け付けます"));
        };
        for (k, v) in m.to_vec() {
            out = out.assoc(k, v);
        }
    }
    Ok(Value::Map(Arc::new(out)))
}

// ---------------------------------------------------------------------------
// セット
// ---------------------------------------------------------------------------

fn set(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("set は 1 引数です"));
    }
    let elems: Vec<Value> = match &args[0] {
        Value::Nil => vec![],
        Value::List(l) => list::to_vec(l),
        Value::Vector(v) => v.to_vec(),
        _ => return Err(MalError::type_err("set はリスト・ベクタにのみ対応します")),
    };
    Ok(Value::Set(Arc::new(PSet::from_vec(elems))))
}

fn disj(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("disj にはセットが必要です"));
    }
    let Value::Set(s) = &args[0] else {
        return Err(MalError::type_err("disj の第 1 引数はセットです"));
    };
    let mut out = (**s).clone();
    for e in &args[1..] {
        out = out.disj(e);
    }
    Ok(Value::Set(Arc::new(out)))
}

// ---------------------------------------------------------------------------
// 高階関数
// ---------------------------------------------------------------------------

fn map(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("map は (map f coll) の形です"));
    }
    let f = &args[0];
    let mut out = Vec::new();
    for e in seq_elements(&args[1], "map")? {
        out.push(apply_fn(f, &[e])?);
    }
    Ok(Value::List(list::from_vec(out)))
}

fn filter(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("filter は (filter pred coll) の形です"));
    }
    let pred = &args[0];
    let mut out = Vec::new();
    for e in seq_elements(&args[1], "filter")? {
        if apply_fn(pred, std::slice::from_ref(&e))?.truthy() {
            out.push(e);
        }
    }
    Ok(Value::List(list::from_vec(out)))
}

fn reduce(args: &[Value]) -> Result<Value, MalError> {
    match args.len() {
        2 => {
            let f = &args[0];
            let mut elems = seq_elements(&args[1], "reduce")?;
            if elems.is_empty() {
                return Err(MalError::arity("reduce: 空のシーケンスには初期値が必要です"));
            }
            let mut acc = elems.remove(0);
            for e in elems {
                acc = apply_fn(f, &[acc, e])?;
            }
            Ok(acc)
        }
        3 => {
            let f = &args[0];
            let mut acc = args[1].clone();
            for e in seq_elements(&args[2], "reduce")? {
                acc = apply_fn(f, &[acc, e])?;
            }
            Ok(acc)
        }
        _ => Err(MalError::arity("reduce は (reduce f coll) または (reduce f init coll) です")),
    }
}

fn apply_builtin(args: &[Value]) -> Result<Value, MalError> {
    if args.len() < 2 {
        return Err(MalError::arity("apply には関数とコレクションが必要です"));
    }
    let (coll, prefix) = args.split_last().unwrap();
    let f = &prefix[0];
    let mut all: Vec<Value> = prefix[1..].to_vec();
    match coll {
        Value::Nil => {}
        Value::List(l) => all.extend(list::to_vec(l)),
        Value::Vector(v) => all.extend(v.to_vec()),
        _ => return Err(MalError::type_err("apply の最後の引数はコレクションです")),
    }
    apply_fn(f, &all)
}

fn partial(args: &[Value]) -> Result<Value, MalError> {
    if args.is_empty() {
        return Err(MalError::arity("partial には関数が必要です"));
    }
    if !matches!(args[0], Value::MalFn(_)) {
        return Err(MalError::type_err("partial の第 1 引数は関数です"));
    }
    Ok(Value::MalFn(Arc::new(MalFn::Partial { f: args[0].clone(), fixed: args[1..].to_vec() })))
}

fn comp(args: &[Value]) -> Result<Value, MalError> {
    Ok(Value::MalFn(Arc::new(MalFn::Comp { fns: args.to_vec() })))
}

fn identity(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("identity は 1 引数です"));
    }
    Ok(args[0].clone())
}

fn constantly(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("constantly は 1 引数です"));
    }
    Ok(Value::MalFn(Arc::new(MalFn::Constantly(args[0].clone()))))
}

// ---------------------------------------------------------------------------
// 文字列 / 出力
// ---------------------------------------------------------------------------

fn str_fn(args: &[Value]) -> Result<Value, MalError> {
    let mut out = String::new();
    for a in args {
        match a {
            Value::Nil => {}
            Value::Str(s) => out.push_str(s),
            _ => out.push_str(&pr_str(a)),
        }
    }
    Ok(Value::Str(out))
}

fn pr_str_fn(args: &[Value]) -> Result<Value, MalError> {
    let parts: Vec<String> = args.iter().map(pr_str).collect();
    Ok(Value::Str(parts.join(" ")))
}

fn print_fn(args: &[Value]) -> Result<Value, MalError> {
    let mut out = String::new();
    for a in args {
        out.push_str(&display_str(a));
    }
    print!("{}", out);
    std::io::stdout().flush().ok();
    Ok(Value::Nil)
}

fn println_fn(args: &[Value]) -> Result<Value, MalError> {
    let mut out = String::new();
    for a in args {
        out.push_str(&display_str(a));
    }
    println!("{}", out);
    Ok(Value::Nil)
}

fn read_string(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("read-string は 1 引数です"));
    }
    let Value::Str(s) = &args[0] else {
        return Err(MalError::type_err("read-string は文字列を要求します"));
    };
    read_str(s)
}

// ---------------------------------------------------------------------------
// 変換
// ---------------------------------------------------------------------------

fn int(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("int は 1 引数です"));
    }
    match &args[0] {
        Value::Int(_) => Ok(args[0].clone()),
        Value::Float(f) => Ok(Value::Int(*f as i64)),
        _ => Err(MalError::type_err("int は数値を要求します")),
    }
}

fn float(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("float は 1 引数です"));
    }
    match &args[0] {
        Value::Float(_) => Ok(args[0].clone()),
        Value::Int(i) => Ok(Value::Float(*i as f64)),
        _ => Err(MalError::type_err("float は数値を要求します")),
    }
}

fn keyword(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("keyword は 1 引数です"));
    }
    match &args[0] {
        Value::Str(s) | Value::Symbol(s) => Ok(Value::Keyword(s.clone())),
        _ => Err(MalError::type_err("keyword は文字列・シンボルを要求します")),
    }
}

fn symbol(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("symbol は 1 引数です"));
    }
    match &args[0] {
        Value::Str(s) | Value::Keyword(s) => Ok(Value::Symbol(s.clone())),
        _ => Err(MalError::type_err("symbol は文字列・キーワードを要求します")),
    }
}

fn name(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("name は 1 引数です"));
    }
    match &args[0] {
        Value::Keyword(s) | Value::Symbol(s) => Ok(Value::Str(s.clone())),
        _ => Err(MalError::type_err("name はキーワード・シンボルを要求します")),
    }
}

// ---------------------------------------------------------------------------
// STM (SPEC §8)
// ---------------------------------------------------------------------------

fn atom(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("atom は 1 引数です"));
    }
    Ok(Value::Atom(crate::stm::Atom::new(args[0].clone())))
}

fn deref(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("deref は 1 引数です"));
    }
    match &args[0] {
        Value::Atom(a) => Ok(a.deref()),
        Value::Ref(r) => r.read(false),
        Value::Future(f) => f.deref(),
        _ => Err(MalError::type_err("deref は atom・ref・future にのみ対応します")),
    }
}

fn swap_bang(args: &[Value]) -> Result<Value, MalError> {
    if args.len() < 2 {
        return Err(MalError::arity("swap! は (swap! atom f & args) の形です"));
    }
    let Value::Atom(a) = &args[0] else {
        return Err(MalError::type_err("swap! の第 1 引数は atom です"));
    };
    if !matches!(args[1], Value::MalFn(_)) {
        return Err(MalError::type_err("swap! の第 2 引数は関数です"));
    }
    a.swap(&args[1], &args[2..])
}

fn reset_bang(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("reset! は (reset! atom x) の形です"));
    }
    let Value::Atom(a) = &args[0] else {
        return Err(MalError::type_err("reset! の第 1 引数は atom です"));
    };
    Ok(a.reset(&args[1]))
}

fn ref_fn(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("ref は 1 引数です"));
    }
    Ok(Value::Ref(crate::stm::Ref::new(args[0].clone())))
}

fn ref_set(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("ref-set は (ref-set ref x) の形です"));
    }
    let Value::Ref(r) = &args[0] else {
        return Err(MalError::type_err("ref-set の第 1 引数は ref です"));
    };
    r.ref_set(args[1].clone())?;
    Ok(args[1].clone())
}

fn alter(args: &[Value]) -> Result<Value, MalError> {
    if args.len() < 2 {
        return Err(MalError::arity("alter は (alter ref f & args) の形です"));
    }
    let Value::Ref(r) = &args[0] else {
        return Err(MalError::type_err("alter の第 1 引数は ref です"));
    };
    if !matches!(args[1], Value::MalFn(_)) {
        return Err(MalError::type_err("alter の第 2 引数は関数です"));
    }
    r.alter(&args[1], &args[2..])
}

fn commute(args: &[Value]) -> Result<Value, MalError> {
    if args.len() < 2 {
        return Err(MalError::arity("commute は (commute ref f & args) の形です"));
    }
    let Value::Ref(r) = &args[0] else {
        return Err(MalError::type_err("commute の第 1 引数は ref です"));
    };
    if !matches!(args[1], Value::MalFn(_)) {
        return Err(MalError::type_err("commute の第 2 引数は関数です"));
    }
    r.commute(&args[1], &args[2..])
}

fn ensure(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("ensure は 1 引数です"));
    }
    let Value::Ref(r) = &args[0] else {
        return Err(MalError::type_err("ensure の引数は ref です"));
    };
    r.read(true)
}

// ---------------------------------------------------------------------------
// Phase 4: not / concat / meta / with-meta / throw
// ---------------------------------------------------------------------------

fn not_fn(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("not は 1 引数です"));
    }
    Ok(Value::Bool(!args[0].truthy()))
}

/// リストを連結する (quasiquote の展開で使う)。
fn concat_fn(args: &[Value]) -> Result<Value, MalError> {
    let mut out = Vec::new();
    for a in args {
        match a {
            Value::Nil => {}
            Value::List(l) => out.extend(list::to_vec(l)),
            _ => return Err(MalError::type_err("concat はリストのみ受け付けます")),
        }
    }
    Ok(Value::List(list::from_vec(out)))
}

/// メタデータを返す (なければ nil)。`apply` 側でメタは剥がされない唯一の組み込み。
fn meta_fn(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("meta は 1 引数です"));
    }
    match &args[0] {
        Value::WithMeta(w) => Ok(Value::Map(Arc::clone(&w.meta))),
        _ => Ok(Value::Nil),
    }
}

/// 値にメタデータを付ける。既存のメタデータは置き換える。
fn with_meta_fn(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 2 {
        return Err(MalError::arity("with-meta は (with-meta x m) の形です"));
    }
    let meta = match &args[1] {
        Value::Nil => return Ok(args[0].clone()),
        Value::Map(m) => (**m).clone(),
        _ => return Err(MalError::type_err("with-meta の第 2 引数はマップまたは nil です")),
    };
    let value = match &args[0] {
        Value::WithMeta(w) => w.value.clone(),
        v => v.clone(),
    };
    Ok(Value::WithMeta(Arc::new(crate::types::WithMetaValue {
        value,
        meta: Arc::new(meta),
    })))
}

/// ユーザーエラーを投げる (try/catch で捕捉できる)。
fn throw_fn(args: &[Value]) -> Result<Value, MalError> {
    if args.len() != 1 {
        return Err(MalError::arity("throw は 1 引数です"));
    }
    let msg = match &args[0] {
        Value::Str(s) => s.clone(),
        Value::Map(m) => match m.get(&Value::Keyword("message".to_string())) {
            Some(Value::Str(s)) => s.clone(),
            Some(v) => pr_str(&v),
            None => "throw".to_string(),
        },
        v => pr_str(v),
    };
    Err(MalError::new(crate::types::ErrorKind::User, msg))
}
