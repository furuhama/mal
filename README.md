# mal — Clojure ライク Lisp サブセット (Rust 実装)

純粋関数型言語の強み——不変性・永続データ構造・STM による並行性——を理解するための学習プロジェクト。

- **仕様**: [SPEC.md](SPEC.md)
- **設計メモ**: [docs/design.md](docs/design.md)

## 状態

仕様ドラフト作成中。実装はレビュー合意後に着手する。

## ロードマップ

1. **Phase 1**: コア言語（reader / printer / eval / 特殊形式 / コア関数 / REPL）
2. **Phase 2**: 永続データ構造（自作）
3. **Phase 3**: atom / ref / STM / future + デモ
4. **Phase 4**（任意）: defmacro / metadata / try-catch

## 参考

- [kanaka/mal](https://github.com/kanaka/mal) — Make a Lisp チュートリアル（step 構成の参考）
- [cljrs-interp](https://lib.rs/crates/cljrs-interp) — Rust 製 Clojure インタプリタ（参考実装例）
