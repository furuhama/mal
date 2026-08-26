//! STM の並行ストレステスト (SPEC §8)。
//!
//! 複数スレッドから atom / ref / dosync を同時に操作し、
//! 更新が失われないこと・不変条件が保たれることを検証する。

use std::sync::Arc;

use mal::env::Env;
use mal::eval::eval_top;
use mal::printer::pr_str;
use mal::reader::read_forms;
use mal::types::Value;

/// 複数の式を順に評価し、最後の結果を返す。
fn eval_str(env: &Arc<Env>, src: &str) -> Value {
    let mut last = Value::Nil;
    for form in read_forms(src).expect(src) {
        last = eval_top(env, &form).unwrap_or_else(|e| panic!("{}: {}", src, e));
    }
    last
}

fn run_in_threads<F: Fn(&Arc<Env>) + Send + Sync + 'static>(env: &Arc<Env>, n: usize, f: F) {
    let f = Arc::new(f);
    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let env = Arc::clone(env);
        let f = Arc::clone(&f);
        handles.push(std::thread::spawn(move || f(&env)));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn atom_counter_stress() {
    // swap! は原子的なので更新が失われない (8 スレッド × 1000 回 = 8000)
    let env = mal::core::default_env();
    eval_str(&env, "(def c (atom 0))");
    run_in_threads(&env, 8, |env| {
        for _ in 0..1000 {
            eval_str(env, "(swap! c inc)");
        }
    });
    assert_eq!(pr_str(&eval_str(&env, "@c")), "8000");
}

#[test]
fn ref_alter_stress() {
    // alter はコミット時に最新値へ適用されるため更新が失われない
    let env = mal::core::default_env();
    eval_str(&env, "(def r (ref 0))");
    run_in_threads(&env, 8, |env| {
        for _ in 0..1000 {
            eval_str(env, "(dosync (alter r inc))");
        }
    });
    assert_eq!(pr_str(&eval_str(&env, "@r")), "8000");
}

#[test]
fn bank_transfer_invariant() {
    // 口座振込: 総額不変・残高非負の不変条件が STM で保たれる (SPEC §8.5)
    let env = mal::core::default_env();
    eval_str(
        &env,
        "
        (def a (ref 1000))
        (def b (ref 1000))
        (defn transfer [from to amt]
          (dosync
            (alter from - amt)
            (alter to + amt)))
        ",
    );
    run_in_threads(&env, 8, |env| {
        for _ in 0..500 {
            eval_str(env, "(transfer a b 1)");
            eval_str(env, "(transfer b a 1)");
        }
    });
    // 総額不変
    assert_eq!(pr_str(&eval_str(&env, "(+ @a @b)")), "2000");
    // 残高非負
    let na: i64 = pr_str(&eval_str(&env, "@a")).parse().unwrap();
    let nb: i64 = pr_str(&eval_str(&env, "@b")).parse().unwrap();
    assert!(na >= 0 && nb >= 0, "残高が負: {} {}", na, nb);
    assert_eq!(na + nb, 2000);
}

#[test]
fn read_write_conflict_retries() {
    // 読み取り → 書き込みパターン (bump): read-set の検証により競合が検出され、
    // 再試行されるため最終的に全更新が反映される。
    // 検証がないと更新が失われて 4000 未満になる。
    let env = mal::core::default_env();
    eval_str(
        &env,
        "
        (def r (ref 0))
        (defn bump []
          (dosync
            (let [v @r]
              (ref-set r (+ v 1)))))
        ",
    );
    run_in_threads(&env, 8, |env| {
        for _ in 0..500 {
            let form = read_forms("(bump)").unwrap().remove(0);
            eval_top(env, &form).unwrap();
        }
    });
    assert_eq!(pr_str(&eval_str(&env, "@r")), "4000");
}

#[test]
fn future_parallel_sum() {
    // future の並行実行と結果の待ち合わせ
    let env = mal::core::default_env();
    eval_str(
        &env,
        "
        (defn worker [n]
          (loop [i 0 acc 0]
            (if (< i n) (recur (inc i) (+ acc i)) acc)))
        (def fs [(future (worker 1000))
                 (future (worker 1000))
                 (future (worker 1000))])
        ",
    );
    let total = eval_str(&env, "(+ @(nth fs 0) @(nth fs 1) @(nth fs 2))");
    assert_eq!(pr_str(&total), "1498500"); // 3 × 499500
}

#[test]
fn ref_set_outside_dosync_errors() {
    let env = mal::core::default_env();
    eval_str(&env, "(def r (ref 0))");
    let err = eval_top(&env, &read_forms("(ref-set r 5)").unwrap().remove(0)).unwrap_err();
    assert_eq!(err.kind, mal::types::ErrorKind::Stm);
}
