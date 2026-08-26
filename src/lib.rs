//! mal — Clojure ライク Lisp サブセット (学習用)。
//!
//! ライブラリとして公開し、`examples/` や統合テストから利用できるようにする。

pub mod core;
pub mod env;
pub mod eval;
pub mod persistent;
pub mod printer;
pub mod reader;
pub mod types;
