//! 永続データ構造の簡易ベンチマーク (Phase 2 完了条件の「ベンチマーク」)。
//!
//! ```sh
//! cargo run --release --example bench
//! ```
//!
//! PVector の conj/get/assoc と PHam の assoc/get のスループットを計測する。
//! 構造共有により、サイズが大きくても操作コストが O(log_32 n) で抑えられることを
//! 実測で確認するのが目的。

use std::time::Instant;

use mal::persistent::{PHam, PVector};
use mal::types::{values_equal, Value};

fn ns_per_op(elapsed: std::time::Duration, n: usize) -> f64 {
    elapsed.as_nanos() as f64 / n as f64
}

fn assert_v_eq(actual: Option<Value>, expected: Value) {
    match actual {
        Some(a) => assert!(values_equal(&a, &expected), "{:?} != {:?}", a, expected),
        None => panic!("None != {:?}", expected),
    }
}

fn main() {
    let n = 1_000_000usize;

    println!("== PVector (n = {}) ==", n);
    let t0 = Instant::now();
    let mut v = PVector::empty();
    for i in 0..n {
        v = v.conj(Value::Int(i as i64));
    }
    let t = t0.elapsed();
    println!("  conj x{n}: {:?} ({:.0} ns/op)", t, ns_per_op(t, n));

    let t0 = Instant::now();
    let mut sum = 0i64;
    for i in 0..n {
        if let Some(Value::Int(x)) = v.get(i) {
            sum += x;
        }
    }
    let t = t0.elapsed();
    println!("  get  x{n}: {:?} ({:.0} ns/op, sum={})", t, ns_per_op(t, n), sum);

    let t0 = Instant::now();
    for _ in 0..1000 {
        let _ = v.assoc(500_000, Value::Int(-1));
    }
    let t = t0.elapsed();
    println!("  assoc x1000: {:?} ({:.0} ns/op)", t, ns_per_op(t, 1000));

    // 元のベクタが不変であることも確認
    let v2 = v.assoc(500_000, Value::Int(-1)).unwrap();
    assert_v_eq(v.get(500_000), Value::Int(500_000));
    assert_v_eq(v2.get(500_000), Value::Int(-1));

    println!("== PHam (n = {}) ==", n);
    let t0 = Instant::now();
    let mut m = PHam::empty();
    for i in 0..n {
        m = m.assoc(Value::Int(i as i64), Value::Int(i as i64 * 2));
    }
    let t = t0.elapsed();
    println!("  assoc x{n}: {:?} ({:.0} ns/op)", t, ns_per_op(t, n));

    let t0 = Instant::now();
    let mut hits = 0i64;
    for i in 0..n {
        if m.get(&Value::Int(i as i64)).is_some() {
            hits += 1;
        }
    }
    let t = t0.elapsed();
    println!("  get  x{n}: {:?} ({:.0} ns/op, hits={})", t, ns_per_op(t, n), hits);

    // 構造共有: 1 回の assoc が既存マップに影響しない
    let m2 = m.assoc(Value::Int(1), Value::Int(-1));
    assert_v_eq(m.get(&Value::Int(1)), Value::Int(2));
    assert_v_eq(m2.get(&Value::Int(1)), Value::Int(-1));

    // Array (≤8) → HAMT の昇格境界
    let mut small = PHam::empty();
    for i in 0..9 {
        small = small.assoc(Value::Int(i), Value::Int(i));
    }
    assert_eq!(small.len(), 9);

    println!("完了: すべての不変条件を確認しました");
}
