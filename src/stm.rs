//! STM (SPEC §8, docs/design.md §6)。
//!
//! - **Atom**: `Mutex<Value>` を CAS 的に (ロック内で) 更新する単一参照。
//! - **Ref**: バージョン付きの値。トランザクション内でのみ変更できる。
//! - **トランザクション** (`dosync`): スレッドローカルのログ (read-set / write-set) に
//!   操作をステージし、コミット時に read-set のバージョンが変わっていないか検証する。
//!   変更されていればトランザクション全体を再実行 (上限 10000 回)。
//! - **Future**: 別スレッドで body を実行し、`deref` で結果を待つ。
//!
//! 注意 (Clojure と同じ): トランザクションは再実行されうるため、`dosync` 内の
//! 副作用 (`println` や `atom` の更新) は複数回実行されうる。純粋なコードを書くこと。

use crate::env::Env;
use crate::eval::{apply, EvalErr};
use crate::types::{ErrorKind, MalError, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

// ===========================================================================
// Atom (SPEC §8.2)
// ===========================================================================

/// 単一参照の原子的更新。`swap!` はロック内で関数を適用する
/// (仕様の CAS ループと等価な原子的更新)。
#[derive(Debug)]
pub struct Atom {
    state: Mutex<Value>,
}

impl Atom {
    pub fn new(v: Value) -> Arc<Atom> {
        Arc::new(Atom { state: Mutex::new(v) })
    }

    /// 現在値の一貫したスナップショット。
    pub fn deref(&self) -> Value {
        self.state.lock().unwrap().clone()
    }

    /// 現在値に f を適用して原子的に更新。新しい値を返す。
    pub fn swap(&self, f: &Value, args: &[Value]) -> Result<Value, MalError> {
        let mut guard = self.state.lock().unwrap();
        let cur = guard.clone();
        let mut all = Vec::with_capacity(1 + args.len());
        all.push(cur);
        all.extend_from_slice(args);
        let new = apply(f, &all)?;
        *guard = new.clone();
        Ok(new)
    }

    pub fn reset(&self, v: &Value) -> Value {
        *self.state.lock().unwrap() = v.clone();
        v.clone()
    }
}

// ===========================================================================
// Ref とトランザクション (SPEC §8.3)
// ===========================================================================

#[derive(Debug)]
pub struct Ref {
    state: Mutex<RefState>,
}

#[derive(Debug)]
pub struct RefState {
    pub value: Value,
    pub version: u64,
}

impl Ref {
    pub fn new(v: Value) -> Arc<Ref> {
        Arc::new(Ref { state: Mutex::new(RefState { value: v, version: 0 }) })
    }

    /// 読み取り。トランザクション内では read-set に記録し、
    /// トランザクション内一貫性 (同一値の返却) を保証する。
    pub fn read(self: &Arc<Ref>, ensure: bool) -> Result<Value, MalError> {
        match current_tx() {
            Some(tx) => tx.read_ref(self, ensure),
            None => Ok(self.state.lock().unwrap().value.clone()),
        }
    }

    /// 書き込みをステージ (トランザクション外ではエラー)。
    pub fn ref_set(self: &Arc<Ref>, v: Value) -> Result<(), MalError> {
        let tx = require_tx("ref-set は dosync 内でのみ使用できます")?;
        tx.stage(Write::Set(Arc::clone(self), v));
        Ok(())
    }

    /// alter / commute。コミット時に最新値へ関数を適用する。
    /// 返り値は「現時点での適用結果」 (Clojure 準拠: alter は新しい値を返す)。
    pub fn alter(self: &Arc<Ref>, f: &Value, args: &[Value]) -> Result<Value, MalError> {
        let tx = require_tx("alter は dosync 内でのみ使用できます")?;
        tx.stage(Write::Alter(Arc::clone(self), f.clone(), args.to_vec()));
        let preview = {
            let cur = self.state.lock().unwrap().value.clone();
            let mut all = Vec::with_capacity(1 + args.len());
            all.push(cur);
            all.extend_from_slice(args);
            apply(f, &all)?
        };
        Ok(preview)
    }

    pub fn commute(self: &Arc<Ref>, f: &Value, args: &[Value]) -> Result<Value, MalError> {
        let tx = require_tx("commute は dosync 内でのみ使用できます")?;
        tx.stage(Write::Commute(Arc::clone(self), f.clone(), args.to_vec()));
        let preview = {
            let cur = self.state.lock().unwrap().value.clone();
            let mut all = Vec::with_capacity(1 + args.len());
            all.push(cur);
            all.extend_from_slice(args);
            apply(f, &all)?
        };
        Ok(preview)
    }
}

/// ステージされた書き込み。
enum Write {
    Set(Arc<Ref>, Value),
    Alter(Arc<Ref>, Value, Vec<Value>), // f, args
    Commute(Arc<Ref>, Value, Vec<Value>),
}

/// read-set のエントリ (読んだ値とバージョンをキャッシュする)。
struct ReadEntry {
    value: Value,
    version: u64,
    ensured: bool,
}

/// トランザクションログ。スレッドローカルに保持する。
struct Transaction {
    reads: RefCell<Vec<(Arc<Ref>, ReadEntry)>>,
    writes: RefCell<Vec<Write>>,
}

thread_local! {
    static TX: RefCell<Option<Rc<Transaction>>> = const { RefCell::new(None) };
}

/// コミットを直列化するグローバルロック。
static COMMIT_LOCK: Mutex<()> = Mutex::new(());

// コミット再入ガード (alter の関数内から dosync が呼ばれた場合のデッドロック防止)。
thread_local! {
    static IN_COMMIT: Cell<bool> = const { Cell::new(false) };
}

fn current_tx() -> Option<Rc<Transaction>> {
    TX.with(|t| t.borrow().clone())
}

fn require_tx(msg: &str) -> Result<Rc<Transaction>, MalError> {
    current_tx().ok_or_else(|| MalError::new(ErrorKind::Stm, msg))
}

impl Transaction {
    fn new() -> Transaction {
        Transaction { reads: RefCell::new(Vec::new()), writes: RefCell::new(Vec::new()) }
    }

    /// トランザクション内での読み取り。
    /// 1. 自分の書き込み (write-set) があればその値 (Clojure: トランザクションは自分の書き込みを見る)
    /// 2. read-set のキャッシュ
    /// 3. ref の実値
    fn read_ref(&self, r: &Arc<Ref>, ensure: bool) -> Result<Value, MalError> {
        // 1. write-set を最後の一致から探す
        for (ref_, write) in self.writes.borrow().iter().enumerate().rev() {
            if !Arc::ptr_eq(write_ref(write), r) {
                continue;
            }
            let v = match write {
                Write::Set(_, v) => v.clone(),
                Write::Alter(_, f, args) | Write::Commute(_, f, args) => {
                    let cur = r.state.lock().unwrap().value.clone();
                    let mut all = Vec::with_capacity(1 + args.len());
                    all.push(cur);
                    all.extend_from_slice(args);
                    apply(f, &all)?
                }
            };
            let _ = ref_;
            return Ok(v);
        }
        // 2. read-set のキャッシュ
        for (ref_, entry) in self.reads.borrow().iter() {
            if Arc::ptr_eq(ref_, r) {
                if ensure && !entry.ensured {
                    // ensured フラグを立てる (実装上の検証は通常の読み取りと同じ)
                    if let Some((_, e)) = self
                        .reads
                        .borrow_mut()
                        .iter_mut()
                        .find(|(x, _)| Arc::ptr_eq(x, r))
                    {
                        e.ensured = true;
                    }
                }
                return Ok(entry.value.clone());
            }
        }
        // 3. 実値を読んでキャッシュ
        let state = r.state.lock().unwrap();
        let entry = ReadEntry {
            value: state.value.clone(),
            version: state.version,
            ensured: ensure,
        };
        self.reads.borrow_mut().push((Arc::clone(r), entry));
        Ok(state.value.clone())
    }

    fn stage(&self, w: Write) {
        self.writes.borrow_mut().push(w);
    }

    /// read-set のバージョンがすべて現在値と一致するか検証する。
    fn validate(&self) -> bool {
        self.reads.borrow().iter().all(|(r, e)| r.state.lock().unwrap().version == e.version)
    }

    /// write-set を適用する (COMMIT_LOCK 保持中に呼ぶこと)。
    fn apply_writes(&self) -> Result<(), MalError> {
        let writes = std::mem::take(&mut *self.writes.borrow_mut());
        for w in writes {
            match w {
                Write::Set(r, v) => {
                    let mut s = r.state.lock().unwrap();
                    s.value = v;
                    s.version += 1;
                }
                Write::Alter(r, f, args) | Write::Commute(r, f, args) => {
                    let mut s = r.state.lock().unwrap();
                    let cur = s.value.clone();
                    let mut all = Vec::with_capacity(1 + args.len());
                    all.push(cur);
                    all.extend_from_slice(&args);
                    s.value = apply(&f, &all)?;
                    s.version += 1;
                }
            }
        }
        Ok(())
    }
}

fn write_ref(w: &Write) -> &Arc<Ref> {
    match w {
        Write::Set(r, _) => r,
        Write::Alter(r, _, _) => r,
        Write::Commute(r, _, _) => r,
    }
}

/// `dosync` の実行。トランザクションを開始し、検証つきでコミットする。
/// ネストした dosync は同じトランザクションに統合される。
pub fn run_dosync(env: &Arc<Env>, body: &[Value]) -> Result<Value, MalError> {
    // ネスト: すでにトランザクション中なら同じトランザクションで評価するだけ
    if TX.with(|t| t.borrow().is_some()) {
        return eval_body(env, body, false).map_err(to_mal_error);
    }
    let max_retries = 10_000u32;
    let mut retries = 0u32;
    loop {
        let tx = Rc::new(Transaction::new());
        TX.with(|t| *t.borrow_mut() = Some(Rc::clone(&tx)));
        let result = eval_body(env, body, false);
        match result {
            Ok(v) => {
                let _guard = COMMIT_LOCK.lock().unwrap();
                if tx.validate() {
                    let commit_result = IN_COMMIT.with(|c| {
                        let already = c.replace(true);
                        let r = tx.apply_writes();
                        c.set(already);
                        r
                    });
                    TX.with(|t| *t.borrow_mut() = None);
                    match commit_result {
                        Ok(()) => return Ok(v),
                        Err(e) => return Err(e),
                    }
                }
                // 検証失敗 → 再試行
                TX.with(|t| *t.borrow_mut() = None);
            }
            Err(e) => {
                TX.with(|t| *t.borrow_mut() = None);
                return Err(to_mal_error(e));
            }
        }
        retries += 1;
        if retries > max_retries {
            return Err(MalError::new(
                ErrorKind::Stm,
                format!("トランザクションの再試行回数が上限 ({} 回) を超えました", max_retries),
            ));
        }
    }
}

fn eval_body(env: &Arc<Env>, body: &[Value], tail: bool) -> Result<Value, EvalErr> {
    crate::eval::eval_body_pub(env, body, tail)
}

fn to_mal_error(e: EvalErr) -> MalError {
    match e {
        EvalErr::Mal(m) => m,
        EvalErr::Recur(_) => MalError::syntax("recur は dosync / future の本体の末尾では使用できません"),
    }
}

// ===========================================================================
// Future (SPEC §8.4)
// ===========================================================================

/// 別スレッドで実行される計算。`deref` で結果を待つ (ブロック)。
#[derive(Debug)]
pub struct Future {
    result: Mutex<Option<Result<Value, MalError>>>,
    cond: Condvar,
}

impl Future {
    pub fn spawn(env: Arc<Env>, body: Vec<Value>) -> Arc<Future> {
        let fut = Arc::new(Future { result: Mutex::new(None), cond: Condvar::new() });
        let fut2 = Arc::clone(&fut);
        thread::spawn(move || {
            let result = eval_body(&env, &body, false).map_err(to_mal_error);
            let mut guard = fut2.result.lock().unwrap();
            *guard = Some(result);
            fut2.cond.notify_all();
        });
        fut
    }

    /// 結果を待って返す。future 内のエラーはここで再送出される。
    pub fn deref(&self) -> Result<Value, MalError> {
        let mut guard = self.result.lock().unwrap();
        while guard.is_none() {
            guard = self.cond.wait(guard).unwrap();
        }
        guard.as_ref().unwrap().clone()
    }
}
