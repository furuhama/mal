# mal — Clojure ライク Lisp サブセット (Rust 実装)

純粋関数型言語の強み——不変性・永続データ構造・STM による並行性——を理解するための学習プロジェクト。

- **仕様**: [SPEC.md](SPEC.md)
- **設計メモ**: [docs/design.md](docs/design.md)

## 状態

- **Phase 1（コア言語）実装済み**: reader / printer / eval / 特殊形式 / コア関数 / REPL
- **Phase 2（永続データ構造）実装済み**: PVector（32-way トライ + tail）/ マップ（Array ≤8 → HAMT）/ セット / cons セルリスト + ベンチマーク
- **Phase 3（並行性と STM）実装済み**: atom / ref / dosync（検証つきトランザクション・再試行）/ future + 口座振込デモ
- **Phase 4（マクロ・メタデータ・例外）実装済み**: defmacro / quasiquote（`` ` `` `~` `~@`）/ with-meta / try-catch-finally / throw

## 使い方

```sh
cargo run            # REPL 起動
cargo run -- file.mal   # ファイル実行
cargo run -- demos/bank-transfer.mal   # STM デモ (並行口座振込)
cargo run -- demos/macros.mal          # マクロデモ
cargo test           # ユニット + ゴールデン + STM ストレステスト
cargo run --release --example bench   # 永続データ構造のベンチマーク
```

## REPL で試す

```clojure
(defmacro unless [c & body] `(if (not ~c) (do ~@body) nil))
(unless false :yes)              ; => :yes
(defn add "足し算" [a b] (+ a b))
(meta add)                       ; => {:doc "足し算"}
(try (foo) (catch e (:kind e)))  ; => :unbound
(def c (atom 0)) (swap! c inc)   ; atom も使える
```

## ロードマップ

1. **Phase 1**: コア言語（reader / printer / eval / 特殊形式 / コア関数 / REPL）
2. **Phase 2**: 永続データ構造（自作）
3. **Phase 3**: atom / ref / STM / future + デモ
4. **Phase 4**（任意）: defmacro / metadata / try-catch

## 参考

- [kanaka/mal](https://github.com/kanaka/mal) — Make a Lisp チュートリアル（step 構成の参考）
- [cljrs-interp](https://lib.rs/crates/cljrs-interp) — Rust 製 Clojure インタプリタ（参考実装例）
