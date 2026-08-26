# 設計メモ (docs/design.md)

SPEC.md の実装設計。方針は実装中に更新してよい（SPEC との乖離が生じた場合は SPEC を更新する）。

## 1. 値の表現

```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Keyword(String),
    Symbol(String),
    List(Rc<List>),          // cons セル（連結リスト）
    Vector(Rc<Vector>),      // Phase 1: Vec<Value> / Phase 2: 永続トライ
    Map(Rc<Map>),            // Phase 1: Vec<(Value, Value)> / Phase 2: HAMT
    Set(Rc<Set>),
    Fn(Rc<Fn>),              // Builtin | User{params, body, env}
    Atom(Rc<Atom>),
    Ref(Rc<Ref>),
    Future(Rc<Future>),
}
```

- 共有は `Rc` で行う。**循環参照**（atom が自分自身を指す等）はリークする。学習プロジェクトとして許容し、`docs/known-limitations.md` に記録する。
- キーワード・シンボルのインターン（`Symbol` テーブル）は後回し。まずは `String`。
- 等価性: `=` は値の深い比較。`Int`/`Float` は数値として比較。関数・atom・ref・future は `Rc` ポインタの同一性で比較。

## 2. 環境

```rust
pub struct Env {
    bindings: RefCell<HashMap<String, Value>>, // Rc 経由で共有されるため
    parent: Option<Rc<Env>>,
}
```

- クロージャは定義時の環境を保持する（レキシカルスコープ）。
- `def` は現在の環境に束縛する。
- `loop`/`let` は子環境を新規作成して束縛する（`let` は右辺を親環境で評価してからまとめて束縛 → 並行バインディングを実現）。

## 3. 評価器

```rust
pub fn eval(env: &Rc<Env>, form: &Value) -> Result<Value, MalError>
```

- 特殊形式は `match` で分岐。関数適用は「全要素を評価 → 先頭が `Fn` なら適用」。
- **TCO**: `loop`/`recur` は末尾位置を構文検査し、ループに変換する（スタックを消費しない）。
- エラー: `MalError { kind: ErrorKind, message: String, pos: Option<Pos> }`。`Display` を実装し、REPL で表示する。

## 4. リーダー

- トークナイザ + 再帰下降パーサ。位置情報（行・列）を保持し、エラーメッセージに含める。
- `'x` → `(quote x)`、`@x` → `(deref x)` の糖衣はパーサ側で展開する。

## 5. 永続データ構造 (Phase 2)

- **Vector**: 32-way 分岐トライ。ノードは `Vec<Rc<Node>>` または固定 32 要素。Clojure の PersistentVector を参考に tail 最適化を採用するかは実装時に判断する。
- **Map**: HAMT。bitmap + 32 要素の子スロット。キーのハッシュは `Value` にハッシュ関数を実装して行う（`=` と整合させる）。
- 詳細は Phase 2 着手時にこの節を書き換える。

## 6. STM (Phase 3)

### 6.1 Ref / Atom の内部表現

```rust
struct RefState { value: Value, version: u64 }
struct Ref { state: Mutex<RefState> }
```

- 検証は「トランザクション開始時に読んだバージョン」と「コミット時の現在バージョン」の比較で行う。
- コミットは**グローバルロック**（`static COMMIT_LOCK: Mutex<()>`）で直列化する。シンプルで十分（学習目的）。
- Atom は `Mutex<Value>` で保持し、`swap!` はロック内で関数を適用する（仕様の CAS ループと等価な原子的更新。詳細は実装時に再検討）。

### 6.2 トランザクション

- スレッドローカル（`thread_local!`）にトランザクションログを保持:

```rust
struct Transaction {
    read_set: Vec<(usize /* ref id */, u64 /* version */, bool /* ensured */)>,
    write_set: Vec<Write>, // SetValue(Value) | Alter(Fn, Vec<Value>) | Commute(Fn, Vec<Value>)
}
```

- 読み取り: 現在値 + バージョンを read_set に記録（同一 ref の再読み取りは記録済みの値を返す → トランザクション内一貫性）。
- 書き込み: write_set にステージ。実値には触れない。
- コミット:
  1. グローバルロック取得
  2. read_set の各 ref の現在バージョンが記録と一致するか検証
  3. 一致 → write_set を適用（`Alter`/`Commute` は現在値に関数を適用）、バージョン増加、ロック解放、成功
  4. 不一致 → ロック解放、ログ破棄、トランザクション全体を再実行（上限 10000 回）
- `ensure` は read_set に保護フラグを立てる（通常の読み取りと同様の検証で実現可能）。
- `dosync` のネストは（Clojure と同様に）内側を外側のトランザクションに統合する。Phase 3 では「ネストは許容、同一トランザクションとして扱う」。

### 6.3 Future

- `std::thread::spawn` で実行。結果は `Mutex<Option<Result<Value, MalError>>>` + `Condvar` で `deref` まで保持する。

## 7. テスト

- `tests/golden/*.mal` + 期待 stdout のゴールデンテスト（テストハーネスを自作）。
- STM ストレステスト: スレッド 8 本 × 振込 1000 回 → 「総額不変・残高非負」を assert。
- `cargo test` で全部回ることを CI 相当の基準とする。

## 8. ディレクトリ構成（予定）

```
mal/
├── SPEC.md              # サブセット仕様
├── docs/
│   ├── design.md        # 本メモ
│   ├── core-functions.md# コア関数の詳細シグネチャ
│   └── known-limitations.md
├── Cargo.toml
├── src/
│   ├── main.rs          # REPL / ファイル実行
│   ├── types.rs         # Value と等価性・表示
│   ├── reader.rs
│   ├── printer.rs
│   ├── env.rs
│   ├── eval.rs
│   ├── core.rs          # 組み込み関数
│   ├── persistent.rs    # Phase 2: 永続データ構造
│   └── stm.rs           # Phase 3: atom / ref / トランザクション
└── tests/
    ├── golden/
    └── stm_stress.rs
```
