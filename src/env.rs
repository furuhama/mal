//! 環境 (レキシカルスコープの連鎖)。docs/design.md §2 参照。

use crate::types::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug)]
pub struct Env {
    bindings: RefCell<HashMap<String, Value>>,
    parent: Option<Rc<Env>>,
}

impl Env {
    /// ルート環境を生成する。
    pub fn new() -> Rc<Env> {
        Rc::new(Env { bindings: RefCell::new(HashMap::new()), parent: None })
    }

    /// `parent` を親とする子環境を生成する。
    pub fn child(parent: &Rc<Env>) -> Rc<Env> {
        Rc::new(Env { bindings: RefCell::new(HashMap::new()), parent: Some(Rc::clone(parent)) })
    }

    /// 現在の環境に束縛する (シャドウイングはしない)。
    pub fn set(&self, name: String, value: Value) {
        self.bindings.borrow_mut().insert(name, value);
    }

    /// シンボルを解決する。親環境をたどる。
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.bindings.borrow().get(name) {
            Some(v.clone())
        } else {
            self.parent.as_ref().and_then(|p| p.get(name))
        }
    }
}
