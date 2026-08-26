//! 環境 (レキシカルスコープの連鎖)。docs/design.md §2 参照。
//!
//! Phase 3 より `Mutex` で束縛を保持する。`future` などのスレッドから
//! 同一環境に並行アクセスされるため (Env は Send + Sync である必要がある)。

use crate::types::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Env {
    bindings: Mutex<HashMap<String, Value>>,
    parent: Option<Arc<Env>>,
}

impl Env {
    /// ルート環境を生成する。
    pub fn new() -> Arc<Env> {
        Arc::new(Env { bindings: Mutex::new(HashMap::new()), parent: None })
    }

    /// `parent` を親とする子環境を生成する。
    pub fn child(parent: &Arc<Env>) -> Arc<Env> {
        Arc::new(Env { bindings: Mutex::new(HashMap::new()), parent: Some(Arc::clone(parent)) })
    }

    /// 現在の環境に束縛する (シャドウイングはしない)。
    pub fn set(&self, name: String, value: Value) {
        self.bindings.lock().unwrap().insert(name, value);
    }

    /// シンボルを解決する。親環境をたどる。
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.bindings.lock().unwrap().get(name) {
            Some(v.clone())
        } else {
            self.parent.as_ref().and_then(|p| p.get(name))
        }
    }
}
