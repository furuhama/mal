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
    List(Option<Arc<ListCell>>), // cons セル（連結リスト）
    Vector(Arc<PVector>),        // 永続トライ (Phase 2)
    Map(Arc<PHam>),              // Array ≤ 8 → HAMT (Phase 2)
    Set(Arc<PSet>),
    MalFn(Arc<MalFn>),           // Builtin | User{params, body, env}
    Atom(Arc<Atom>),             // Phase 3
    Ref(Arc<Ref>),               // Phase 3
    Future(Arc<Future>),         // Phase 3
}
```

- 共有は `Arc` で行う (Phase 3 から `future` がスレッドを跨ぐため。`Rc` → `Arc` に移行済み)。
  **循環参照**（atom が自分自身を指す等）はリークする。学習プロジェクトとして許容する。
- キーワード・シンボルのインターン（`Symbol` テーブル）は後回し。まずは `String`。
- 等価性: `=` は値の深い比較。`Int`/`Float` は数値として比較。関数・atom・ref・future は `Arc` ポインタの同一性で比較。

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

- 特殊形式は `match` で分岐。関数適用は「全要素を評価 → 先頭が `MalFn` なら適用」。
- **TCO**: `loop`/`recur` と `fn` 本体末尾の `recur` は、末尾位置検査つきでループに変換する（スタックを消費しない）。末尾位置判定は `tail` フラグを `eval` に渡して行い、`recur` は `EvalErr::Recur(args)` という制御フローで外側の `loop` / fn 適用に伝える。
- エラー: `MalError { kind: ErrorKind, message: String, pos: Option<Pos> }`。`Display` を実装し、REPL で表示する。

## 4. リーダー

- トークナイザ + 再帰下降パーサ。位置情報（行・列）を保持し、エラーメッセージに含める。
- `'x` → `(quote x)`、`@x` → `(deref x)` の糖衣はパーサ側で展開する。

## 5. 永続データ構造 (Phase 2) — 実装済み

`src/persistent.rs`。すべて `&self` → 新インスタンスの永続操作。共有は `Rc`。

### 5.1 PVector (32-way トライ + tail)

```rust
struct PVector { root: Option<Rc<Node>>, tail: Vec<Value>, shift: u32, len: usize }
enum Node {
    Branch(Vec<Option<Rc<Node>>>), // 非葉: 常に 32 スロット
    Leaf(Vec<Value>),              // 葉: 常に 32 スロット
}
```

- 末尾 32 要素は `tail` に保持 (conj は償却 O(1) に近い)。tail が満杯になったら古い tail をツリーの葉として押し込む (`push_tail` がパスをコピー)。
- ルートが満杯 (32 子) になったら 1 段階成長 (`shift += 5`)。
- `get`/`assoc` はシフト演算でパスを辿る。`(i >> shift) & 31` でスロット決定。
- 構造共有: 更新はパス上のノードだけをコピーするので O(log₃₂ n)。

### 5.2 PHam (Array ≤ 8 → HAMT)

```rust
struct PHam { inner: Ham }
enum Ham {
    Array(Vec<(Value, Value)>),   // ≤ 8 エントリ: 挿入順を保持 (Clojure の array-map 相当)
    Trie(Rc<TrieNode>),           // 超えたら HAMT に昇格
}
enum TrieNode {
    Bitmap { bitmap: u32, children: Vec<Rc<TrieNode>> }, // 32-way、bitmap でスロット管理
    Leaf { hash: u64, key: Value, value: Value },
    Collision { hash: u64, entries: Vec<(Value, Value)> }, // 同一ハッシュ
}
```

- 挿入: bitmap の立っていないビット → Leaf を追加。Leaf と衝突 → `merge_node` でスロットが分かれるまで深掘り、同一ハッシュなら Collision。
- 削除: 子が消えたらビットを落とし、子が 1 つになったら昇格。
- **ハッシュは `=` と整合させる**: Int/Float は `hash_float_like` (整数値なら i64 正規化) で同一ハッシュ。これにより `(get {:1 "one"} 1.0)` が正しく動く。

### 5.3 PSet / List

- PSet: PHam の値なし版 (値は常に Nil)。
- List: `Option<Rc<ListCell>>` の cons セル (types.rs)。`first`/`rest`/`cons` が O(1)。

### 5.4 ベンチマーク (release, 100 万要素, 2025 計測)

| 操作 | 時間 |
|---|---|
| PVector conj | 536 ns/op |
| PVector get | 18 ns/op |
| PVector assoc | 1,729 ns/op |
| PHam assoc | 6,385 ns/op |
| PHam get | 487 ns/op |

`cargo run --release --example bench` で再計測できる。

## 6. STM (Phase 3) — 実装済み

`src/stm.rs`。**前提**: Phase 3 から値はスレッドを跨ぐため、全共有構造を `Rc` → `Arc` に、
`Env` の `RefCell` → `Mutex` に移行した (Value は Send + Sync)。

### 6.1 Ref / Atom の内部表現

```rust
struct RefState { value: Value, version: u64 }
struct Ref { state: Mutex<RefState> }
struct Atom { state: Mutex<Value> } // swap! はロック内で関数適用
```

- 検証は「トランザクション中に読んだバージョン」と「コミット時の現在バージョン」の比較。
- コミットは**グローバルロック** (`static COMMIT_LOCK: Mutex<()>`) で直列化。
- コミット再入ガード: `thread_local IN_COMMIT` フラグで、alter の関数内からの
  ネスト dosync (同一トランザクションに統合されるためロック再取得なし) による
  デッドロックを防ぐ。

### 6.2 トランザクション

```rust
struct Transaction {
    reads: RefCell<Vec<(Arc<Ref>, ReadEntry)>>, // value / version / ensured をキャッシュ
    writes: RefCell<Vec<Write>>,                // Set | Alter(f, args) | Commute(f, args)
}
thread_local! { static TX: RefCell<Option<Rc<Transaction>>>; } // スレッドローカル (Rc)
```

- **読み取り** (`read_ref`): ①自分の write-set に一致があればその値 (トランザクションは自分の書き込みを見る)、
  ②read-set のキャッシュ、③実値 + バージョンをキャッシュ。
- **書き込み**: write-set にステージ。実値には触れない。
- **コミット** (`run_dosync`):
  1. COMMIT_LOCK 取得
  2. read-set の各 ref の現在バージョンが記録と一致するか検証
  3. 一致 → write-set を適用 (`Alter`/`Commute` は**コミット時の最新値**に関数を適用)、
     バージョン増加、ロック解放、成功
  4. 不一致 → ログ破棄、トランザクション全体を再実行 (上限 10000 回、超過はエラー)
- **alter / commute は read-set に含めない** → 並行する書き込み同士は競合しない
  (バンク振込が再試行嵐にならない)。読み取り (`deref`) と組み合わせた
  「読み取り → 書き込み」パターンでのみ検証が効く。
- `ensure` は read-set に記録する (通常の読み取りと同一の検証で実現)。
- `dosync` のネストは同じトランザクションに統合される。
- **注意**: トランザクションは再実行されうるため、`dosync` 内の副作用
  (`println` / `atom` の更新) は複数回実行されうる。純粋なコードを書くこと。

### 6.3 Future

- `std::thread::spawn` で body を評価。結果は `Mutex<Option<Result<Value, MalError>>>` +
  `Condvar` で保持し、`deref` がブロックして結果 (またはエラーの再送出) を返す。

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
