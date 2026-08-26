//! 永続データ構造 (SPEC §7, docs/design.md §5)。
//!
//! - **ベクタ** (`PVector`): 32-way 分岐トライ + tail 最適化 (Clojure PersistentVector 相当)。
//!   更新・参照は O(log_32 n)。
//! - **マップ** (`PHam`): 8 エントリ以下は挿入順を保つ Array、超えると HAMT
//!   (hash array mapped trie) に昇格 (Clojure の array-map → hash-map と同じ方式)。
//! - **セット** (`PSet`): マップの値なし版 (値は常に `Nil`)。
//! - リストは cons セル (単方向連結リスト)。`types::list` 参照。

use crate::types::{list, values_equal, Value};
use std::sync::Arc;

const BRANCH: usize = 32;
const SHIFT_BITS: u32 = 5;
const MASK: u32 = 31;

// ===========================================================================
// ベクタ (32-way 分岐トライ + tail)
// ===========================================================================

#[derive(Debug)]
enum Node {
    /// 非葉ノード。常に 32 スロット (子ノードへの Option)。
    Branch(Vec<Option<Arc<Node>>>),
    /// 葉ノード。常に 32 スロット (値)。
    Leaf(Vec<Value>),
}

#[derive(Debug, Clone)]
pub struct PVector {
    root: Option<Arc<Node>>,
    tail: Vec<Value>, // 末尾の最大 32 要素
    shift: u32,       // ルート直下のシフト量 (5 の倍数)
    len: usize,
}

fn empty_branch() -> Vec<Option<Arc<Node>>> {
    let mut v = Vec::with_capacity(BRANCH);
    v.resize(BRANCH, None);
    v
}

impl PVector {
    pub fn empty() -> PVector {
        PVector { root: None, tail: Vec::new(), shift: SHIFT_BITS, len: 0 }
    }

    pub fn from_vec(v: Vec<Value>) -> PVector {
        let mut pv = PVector::empty();
        for x in v {
            pv = pv.conj(x);
        }
        pv
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// i 番目の要素。範囲外は None。O(log_32 n)。
    pub fn get(&self, i: usize) -> Option<Value> {
        if i >= self.len {
            return None;
        }
        let tail_start = self.len - self.tail.len();
        if i >= tail_start {
            return self.tail.get(i - tail_start).cloned();
        }
        let mut node: &Arc<Node> = self.root.as_ref()?;
        let mut shift = self.shift;
        while shift > 0 {
            let slot = ((i as u32 >> shift) & MASK) as usize;
            node = match &**node {
                Node::Branch(children) => children[slot].as_ref()?,
                Node::Leaf(_) => return None,
            };
            shift -= SHIFT_BITS;
        }
        match &**node {
            Node::Leaf(vals) => vals.get(((i as u32) & MASK) as usize).cloned(),
            Node::Branch(_) => None,
        }
    }

    /// 末尾に追加。償却 O(log_32 n)。
    pub fn conj(&self, v: Value) -> PVector {
        let mut new_tail = self.tail.clone();
        if new_tail.len() < BRANCH {
            new_tail.push(v);
            return PVector { root: self.root.clone(), shift: self.shift, tail: new_tail, len: self.len + 1 };
        }
        // tail が満杯: 古い tail をツリーに押し込む
        let idx = self.len - BRANCH;
        let (root, shift) = match &self.root {
            None => {
                let root = push_tail(Arc::new(Node::Branch(empty_branch())), SHIFT_BITS, idx, &new_tail);
                (Some(root), SHIFT_BITS)
            }
            Some(r) if root_full(r) => {
                // ルートが満杯: 1 段階成長させる
                let grown = Arc::new(Node::Branch(child_at(r.clone(), 0)));
                let root = push_tail(grown, self.shift + SHIFT_BITS, idx, &new_tail);
                (Some(root), self.shift + SHIFT_BITS)
            }
            Some(r) => {
                let root = push_tail(Arc::clone(r), self.shift, idx, &new_tail);
                (Some(root), self.shift)
            }
        };
        PVector { root, shift, tail: vec![v], len: self.len + 1 }
    }

    /// i 番目を置き換える。範囲外は None。元のベクタは不変。
    pub fn assoc(&self, i: usize, v: Value) -> Option<PVector> {
        if i >= self.len {
            return None;
        }
        let tail_start = self.len - self.tail.len();
        if i >= tail_start {
            let mut new_tail = self.tail.clone();
            new_tail[i - tail_start] = v;
            return Some(PVector { root: self.root.clone(), shift: self.shift, tail: new_tail, len: self.len });
        }
        let root = assoc_path(self.root.as_ref()?, self.shift, i, v);
        Some(PVector { root: Some(root), shift: self.shift, tail: self.tail.clone(), len: self.len })
    }

    /// 全要素を順に取り出す。
    pub fn to_vec(&self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.len);
        if let Some(root) = &self.root {
            collect_node(root, &mut out);
        }
        out.extend(self.tail.iter().cloned());
        out
    }
}

pub fn vector_equal(a: &PVector, b: &PVector) -> bool {
    if a.len != b.len {
        return false;
    }
    a.to_vec().iter().zip(b.to_vec().iter()).all(|(x, y)| values_equal(x, y))
}

fn root_full(root: &Arc<Node>) -> bool {
    match &**root {
        Node::Branch(children) => children.iter().all(|c| c.is_some()),
        Node::Leaf(_) => true,
    }
}

fn child_at(root: Arc<Node>, slot: usize) -> Vec<Option<Arc<Node>>> {
    let mut v = empty_branch();
    v[slot] = Some(root);
    v
}

/// idx を先頭とする tail を、ツリーの正しい位置に葉として押し込む。
fn push_tail(node: Arc<Node>, shift: u32, idx: usize, tail: &[Value]) -> Arc<Node> {
    if shift == 0 {
        return Arc::new(Node::Leaf(tail.to_vec()));
    }
    let Node::Branch(children) = &*node else {
        unreachable!("push_tail は Branch のみ受け取る")
    };
    let slot = ((idx as u32 >> shift) & MASK) as usize;
    let new_child = match &children[slot] {
        Some(child) => push_tail(Arc::clone(child), shift - SHIFT_BITS, idx, tail),
        None => push_tail(Arc::new(Node::Branch(empty_branch())), shift - SHIFT_BITS, idx, tail),
    };
    let mut new_children = children.clone();
    new_children[slot] = Some(new_child);
    Arc::new(Node::Branch(new_children))
}

fn assoc_path(node: &Arc<Node>, shift: u32, i: usize, v: Value) -> Arc<Node> {
    if shift == 0 {
        let Node::Leaf(vals) = &**node else {
            unreachable!("葉レベルは Leaf")
        };
        let mut new_vals = vals.clone();
        new_vals[((i as u32) & MASK) as usize] = v;
        return Arc::new(Node::Leaf(new_vals));
    }
    let Node::Branch(children) = &**node else {
        unreachable!("分岐レベルは Branch")
    };
    let slot = ((i as u32 >> shift) & MASK) as usize;
    let new_child = assoc_path(children[slot].as_ref().expect("パス上にノードがある"), shift - SHIFT_BITS, i, v);
    let mut new_children = children.clone();
    new_children[slot] = Some(new_child);
    Arc::new(Node::Branch(new_children))
}

fn collect_node(node: &Arc<Node>, out: &mut Vec<Value>) {
    match &**node {
        Node::Leaf(vals) => out.extend(vals.iter().cloned()),
        Node::Branch(children) => {
            for c in children.iter().flatten() {
                collect_node(c, out);
            }
        }
    }
}

// ===========================================================================
// マップ (Array → HAMT)
// ===========================================================================

/// Array から HAMT に昇格するしきい値 (Clojure と同じく 8)。
const ARRAY_THRESHOLD: usize = 8;

#[derive(Debug, Clone)]
pub struct PHam {
    inner: Ham,
}

#[derive(Debug, Clone)]
enum Ham {
    /// 挿入順を保つ小さいマップ (≤ 8 エントリ)。
    Array(Vec<(Value, Value)>),
    /// HAMT。
    Trie(Arc<TrieNode>),
}

#[derive(Debug)]
enum TrieNode {
    /// 分岐ノード。bitmap の立ったビット位置に対応する子を持つ。
    Bitmap { bitmap: u32, children: Vec<Arc<TrieNode>> },
    /// 単一エントリの葉。
    Leaf { hash: u64, key: Value, value: Value },
    /// 同一ハッシュの衝突を保持するノード。
    Collision { hash: u64, entries: Vec<(Value, Value)> },
}

impl PHam {
    pub fn empty() -> PHam {
        PHam { inner: Ham::Array(vec![]) }
    }

    /// ペア列から構築する。同一キーは後勝ちで重複排除する。
    pub fn from_vec(pairs: Vec<(Value, Value)>) -> PHam {
        let mut m = PHam::empty();
        for (k, v) in pairs {
            m = m.assoc(k, v);
        }
        m
    }

    pub fn len(&self) -> usize {
        match &self.inner {
            Ham::Array(a) => a.len(),
            Ham::Trie(t) => trie_len(t),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// キーを検索する。O(log_32 n)。
    pub fn get(&self, key: &Value) -> Option<Value> {
        match &self.inner {
            Ham::Array(a) => a.iter().find(|(k, _)| values_equal(k, key)).map(|(_, v)| v.clone()),
            Ham::Trie(t) => trie_get(t, hash_value(key), key, 0),
        }
    }

    /// キーを追加・置換する。元のマップは不変。
    pub fn assoc(&self, key: Value, value: Value) -> PHam {
        match &self.inner {
            Ham::Array(a) => {
                let mut new_a = a.clone();
                if let Some(slot) = new_a.iter_mut().find(|(k, _)| values_equal(k, &key)) {
                    slot.1 = value;
                    PHam { inner: Ham::Array(new_a) }
                } else if new_a.len() < ARRAY_THRESHOLD {
                    new_a.push((key, value));
                    PHam { inner: Ham::Array(new_a) }
                } else {
                    // しきい値を超えた: HAMT に昇格して挿入
                    let mut trie = Arc::new(TrieNode::Bitmap { bitmap: 0, children: vec![] });
                    for (k, v) in a.iter().cloned() {
                        trie = trie_assoc(trie, hash_value(&k), k, v, 0);
                    }
                    trie = trie_assoc(trie, hash_value(&key), key, value, 0);
                    PHam { inner: Ham::Trie(trie) }
                }
            }
            Ham::Trie(t) => {
                let trie = trie_assoc(Arc::clone(t), hash_value(&key), key, value, 0);
                PHam { inner: Ham::Trie(trie) }
            }
        }
    }

    /// キーを削除する。元のマップは不変。
    pub fn dissoc(&self, key: &Value) -> PHam {
        match &self.inner {
            Ham::Array(a) => {
                let new_a: Vec<_> = a.iter().filter(|(k, _)| !values_equal(k, key)).cloned().collect();
                PHam { inner: Ham::Array(new_a) }
            }
            Ham::Trie(t) => match trie_dissoc(Arc::clone(t), hash_value(key), key, 0) {
                Some(t) => PHam { inner: Ham::Trie(t) },
                None => PHam::empty(),
            },
        }
    }

    /// 全エントリ。Array は挿入順、Trie はハッシュ順。
    pub fn to_vec(&self) -> Vec<(Value, Value)> {
        match &self.inner {
            Ham::Array(a) => a.clone(),
            Ham::Trie(t) => {
                let mut out = Vec::new();
                trie_collect(t, &mut out);
                out
            }
        }
    }
}

pub fn ham_equal(a: &PHam, b: &PHam) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.to_vec().iter().all(|(k, v)| b.get(k).is_some_and(|bv| values_equal(&bv, v)))
}

fn trie_len(t: &Arc<TrieNode>) -> usize {
    match &**t {
        TrieNode::Leaf { .. } => 1,
        TrieNode::Collision { entries, .. } => entries.len(),
        TrieNode::Bitmap { children, .. } => children.iter().map(trie_len).sum(),
    }
}

fn trie_get(t: &Arc<TrieNode>, hash: u64, key: &Value, shift: u32) -> Option<Value> {
    match &**t {
        TrieNode::Leaf { hash: h, key: k, value } if *h == hash && values_equal(k, key) => {
            Some(value.clone())
        }
        TrieNode::Leaf { .. } => None,
        TrieNode::Collision { hash: h, entries } if *h == hash => {
            entries.iter().find(|(k, _)| values_equal(k, key)).map(|(_, v)| v.clone())
        }
        TrieNode::Collision { .. } => None,
        TrieNode::Bitmap { bitmap, children } => {
            let slot = ((hash >> shift) & MASK as u64) as u32;
            let bit = 1u32 << slot;
            if bitmap & bit == 0 {
                return None;
            }
            let idx = (bitmap & (bit - 1)).count_ones() as usize;
            trie_get(&children[idx], hash, key, shift + SHIFT_BITS)
        }
    }
}

fn trie_assoc(t: Arc<TrieNode>, hash: u64, key: Value, value: Value, shift: u32) -> Arc<TrieNode> {
    match &*t {
        TrieNode::Bitmap { bitmap, children } => {
            let slot = ((hash >> shift) & MASK as u64) as u32;
            let bit = 1u32 << slot;
            let idx = (*bitmap & (bit - 1)).count_ones() as usize;
            let mut new_bm = *bitmap;
            let mut new_ch = children.clone();
            if *bitmap & bit == 0 {
                new_bm |= bit;
                new_ch.insert(idx, Arc::new(TrieNode::Leaf { hash, key, value }));
            } else {
                new_ch[idx] = trie_assoc(Arc::clone(&children[idx]), hash, key, value, shift + SHIFT_BITS);
            }
            Arc::new(TrieNode::Bitmap { bitmap: new_bm, children: new_ch })
        }
        TrieNode::Leaf { .. } | TrieNode::Collision { .. } => merge_node(t, hash, key, value, shift),
    }
}

/// 葉 (または衝突ノード) と新しいキーを、両方含む部分木にマージする。
fn merge_node(t: Arc<TrieNode>, hash: u64, key: Value, value: Value, shift: u32) -> Arc<TrieNode> {
    let old_hash = match &*t {
        TrieNode::Leaf { hash, .. } | TrieNode::Collision { hash, .. } => *hash,
        TrieNode::Bitmap { .. } => unreachable!(),
    };
    if old_hash == hash {
        // 同一ハッシュ: Collision に合流 (同一キーなら置換)
        match &*t {
            TrieNode::Leaf { key: k, value: v, .. } => {
                if values_equal(k, &key) {
                    Arc::new(TrieNode::Leaf { hash, key, value })
                } else {
                    Arc::new(TrieNode::Collision { hash, entries: vec![(k.clone(), v.clone()), (key, value)] })
                }
            }
            TrieNode::Collision { entries, .. } => {
                let mut es = entries.clone();
                if let Some(slot) = es.iter_mut().find(|(k, _)| values_equal(k, &key)) {
                    slot.1 = value;
                } else {
                    es.push((key, value));
                }
                Arc::new(TrieNode::Collision { hash, entries: es })
            }
            TrieNode::Bitmap { .. } => unreachable!(),
        }
    } else {
        let s1 = ((old_hash >> shift) & MASK as u64) as u32;
        let s2 = ((hash >> shift) & MASK as u64) as u32;
        let mut bm = 0u32;
        let mut children: Vec<Arc<TrieNode>> = Vec::new();
        if s1 == s2 {
            // 同じスロット: 1 段深くして再帰
            let child = merge_node(t, hash, key, value, shift + SHIFT_BITS);
            add_child(&mut bm, &mut children, s1, child);
        } else {
            add_child(&mut bm, &mut children, s1, t);
            add_child(&mut bm, &mut children, s2, Arc::new(TrieNode::Leaf { hash, key, value }));
        }
        Arc::new(TrieNode::Bitmap { bitmap: bm, children })
    }
}

fn add_child(bm: &mut u32, children: &mut Vec<Arc<TrieNode>>, slot: u32, node: Arc<TrieNode>) {
    let bit = 1u32 << slot;
    let idx = (*bm & (bit - 1)).count_ones() as usize;
    *bm |= bit;
    children.insert(idx, node);
}

fn trie_dissoc(t: Arc<TrieNode>, hash: u64, key: &Value, shift: u32) -> Option<Arc<TrieNode>> {
    match &*t {
        TrieNode::Leaf { hash: h, key: k, .. } => {
            if *h == hash && values_equal(k, key) {
                None
            } else {
                Some(t)
            }
        }
        TrieNode::Collision { hash: h, entries } => {
            if *h != hash {
                return Some(t);
            }
            let new_entries: Vec<_> =
                entries.iter().filter(|(k, _)| !values_equal(k, key)).cloned().collect();
            if new_entries.len() == entries.len() {
                Some(t)
            } else if new_entries.len() == 1 {
                Some(Arc::new(TrieNode::Leaf {
                    hash,
                    key: new_entries[0].0.clone(),
                    value: new_entries[0].1.clone(),
                }))
            } else {
                Some(Arc::new(TrieNode::Collision { hash, entries: new_entries }))
            }
        }
        TrieNode::Bitmap { bitmap, children } => {
            let slot = ((hash >> shift) & MASK as u64) as u32;
            let bit = 1u32 << slot;
            if bitmap & bit == 0 {
                return Some(t);
            }
            let idx = (*bitmap & (bit - 1)).count_ones() as usize;
            match trie_dissoc(Arc::clone(&children[idx]), hash, key, shift + SHIFT_BITS) {
                None => {
                    let new_bm = *bitmap & !bit;
                    let mut new_ch = children.clone();
                    new_ch.remove(idx);
                    if new_ch.len() == 1 {
                        // 子が 1 つだけになったら昇格
                        Some(Arc::clone(&new_ch[0]))
                    } else {
                        Some(Arc::new(TrieNode::Bitmap { bitmap: new_bm, children: new_ch }))
                    }
                }
                Some(new_child) => {
                    let mut new_ch = children.clone();
                    new_ch[idx] = new_child;
                    Some(Arc::new(TrieNode::Bitmap { bitmap: *bitmap, children: new_ch }))
                }
            }
        }
    }
}

fn trie_collect(t: &Arc<TrieNode>, out: &mut Vec<(Value, Value)>) {
    match &**t {
        TrieNode::Leaf { key, value, .. } => out.push((key.clone(), value.clone())),
        TrieNode::Collision { entries, .. } => out.extend(entries.iter().cloned()),
        TrieNode::Bitmap { children, .. } => {
            for c in children {
                trie_collect(c, out);
            }
        }
    }
}

// ===========================================================================
// セット (マップの値なし版)
// ===========================================================================

#[derive(Debug, Clone)]
pub struct PSet(PHam);

impl PSet {
    pub fn empty() -> PSet {
        PSet(PHam::empty())
    }

    pub fn from_vec(v: Vec<Value>) -> PSet {
        let mut s = PSet::empty();
        for x in v {
            s = s.conj(x);
        }
        s
    }

    pub fn conj(&self, v: Value) -> PSet {
        PSet(self.0.assoc(v, Value::Nil))
    }

    pub fn disj(&self, v: &Value) -> PSet {
        PSet(self.0.dissoc(v))
    }

    pub fn contains(&self, v: &Value) -> bool {
        self.0.get(v).is_some()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_vec(&self) -> Vec<Value> {
        self.0.to_vec().into_iter().map(|(k, _)| k).collect()
    }
}

pub fn set_equal(a: &PSet, b: &PSet) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.to_vec().iter().all(|e| b.contains(e))
}

// ===========================================================================
// ハッシュ (`=` と整合する)
// ===========================================================================

/// `=` で等しい値は必ず同じハッシュになる (HAMT の検索整合性)。
/// 逆 (異なる値が同じハッシュ) は衝突として許容される。
pub fn hash_value(v: &Value) -> u64 {
    match v {
        Value::Nil => 0x9e37_79b9_7f4a_7c15,
        Value::Bool(b) => {
            if *b {
                1
            } else {
                2
            }
        }
        Value::Int(i) => hash_float_like(*i as f64),
        Value::Float(f) => hash_float_like(*f),
        Value::Str(s) => fnv(s.as_bytes()),
        Value::Keyword(s) => fnv(s.as_bytes()) ^ 0x51_7c_c1_b7_27_22_0a_95,
        Value::Symbol(s) => fnv(s.as_bytes()) ^ 0x9e_37_79_b9_7f_4a_7c_15,
        Value::List(l) => hash_seq(&list::to_vec(l)),
        Value::Vector(v) => hash_seq(&v.to_vec()),
        Value::Map(m) => m.to_vec().iter().fold(0u64, |acc, (k, val)| {
            acc.wrapping_add(hash_value(k).wrapping_mul(31).wrapping_add(hash_value(val)))
        }),
        Value::Set(s) => s.to_vec().iter().fold(0u64, |acc, e| acc.wrapping_add(hash_value(e))),
        Value::MalFn(f) => mix64(Arc::as_ptr(f) as usize as u64),
        Value::Atom(a) => mix64(Arc::as_ptr(a) as usize as u64),
        Value::Ref(r) => mix64(Arc::as_ptr(r) as usize as u64),
        Value::Future(f) => mix64(Arc::as_ptr(f) as usize as u64),
    }
}

/// Int/Float は数値として等価なので、同じ正規化でハッシュする。
fn hash_float_like(f: f64) -> u64 {
    if f.fract() == 0.0 && f.abs() <= i64::MAX as f64 {
        mix64(f as i64 as u64)
    } else {
        mix64(f.to_bits())
    }
}

fn hash_seq(v: &[Value]) -> u64 {
    let mut h = 0x100000001b3u64.wrapping_mul(0xcbf2_9ce4_8422_2325);
    for e in v {
        h ^= hash_value(e);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn fnv(s: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn mix64(x: u64) -> u64 {
    // splitmix64 風のミキサー
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

// ===========================================================================
// テスト
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `values_equal` ベースのアサーション (Value は PartialEq を持たない設計)。
    fn v_eq(actual: Option<Value>, expected: Value, msg: &str) {
        match actual {
            Some(a) => assert!(values_equal(&a, &expected), "{}: {:?} != {:?}", msg, a, expected),
            None => panic!("{}: None != {:?}", msg, expected),
        }
    }

    #[test]
    fn vector_basic_and_persistence() {
        let mut v = PVector::empty();
        for i in 0..1000usize {
            v = v.conj(Value::Int(i as i64));
        }
        assert_eq!(v.len(), 1000);
        for i in 0..1000usize {
            v_eq(v.get(i), Value::Int(i as i64), "get");
        }
        assert!(v.get(1000).is_none());

        // assoc しても元は不変 (構造共有)
        let v2 = v.assoc(0, Value::Int(9999)).unwrap();
        v_eq(v.get(0), Value::Int(0), "元は不変");
        v_eq(v2.get(0), Value::Int(9999), "更新後");
        v_eq(v2.get(1), Value::Int(1), "共有部分");
        v_eq(v2.get(999), Value::Int(999), "末尾");
        assert!(v.assoc(1000, Value::Int(1)).is_none());
    }

    #[test]
    fn vector_growth() {
        // 32・1024・32768 を跨ぐ成長
        for n in [0usize, 1, 31, 32, 33, 100, 1023, 1024, 1025, 32768] {
            let mut v = PVector::empty();
            for i in 0..n {
                v = v.conj(Value::Int(i as i64));
            }
            assert_eq!(v.len(), n, "len({})", n);
            for i in 0..n {
                v_eq(v.get(i), Value::Int(i as i64), &format!("get({}, n={})", i, n));
            }
        }
    }

    #[test]
    fn map_basic_and_persistence() {
        let mut m = PHam::empty();
        for i in 0..1000i64 {
            m = m.assoc(Value::Int(i), Value::Int(i * 2));
        }
        assert_eq!(m.len(), 1000);
        v_eq(m.get(&Value::Int(500)), Value::Int(1000), "get");
        assert!(m.get(&Value::Int(2000)).is_none());

        let m2 = m.assoc(Value::Int(500), Value::Int(-1));
        v_eq(m2.get(&Value::Int(500)), Value::Int(-1), "更新後");
        v_eq(m.get(&Value::Int(500)), Value::Int(1000), "元は不変");

        let m3 = m2.dissoc(&Value::Int(500));
        assert!(m3.get(&Value::Int(500)).is_none());
        assert_eq!(m3.len(), 999);
    }

    #[test]
    fn map_array_order_and_upgrade() {
        // 8 以下は挿入順 (Array)
        let mut m = PHam::empty();
        for i in 0..8 {
            m = m.assoc(Value::Int(i), Value::Int(i));
        }
        let keys: Vec<_> = m.to_vec().into_iter().map(|(k, _)| k).collect();
        let expected: Vec<_> = (0..8).map(Value::Int).collect();
        assert_eq!(keys.len(), expected.len());
        for (k, e) in keys.iter().zip(expected.iter()) {
            assert!(values_equal(k, e), "{:?} != {:?}", k, e);
        }

        // 9 個目で HAMT に昇格しても全エントリを保持
        let m2 = m.assoc(Value::Int(100), Value::Int(100));
        assert_eq!(m2.len(), 9);
        for i in 0..8 {
            v_eq(m2.get(&Value::Int(i)), Value::Int(i), "昇格後");
        }
        v_eq(m2.get(&Value::Int(100)), Value::Int(100), "新しいキー");
    }

    #[test]
    fn map_key_equality_numeric() {
        // Int(1) と Float(1.0) は同一キー (SPEC §6.3 補足)
        let m = PHam::empty().assoc(Value::Int(1), Value::Str("one".to_string()));
        v_eq(m.get(&Value::Float(1.0)), Value::Str("one".to_string()), "Int キーを Float で検索");
        let m2 = m.assoc(Value::Float(1.0), Value::Str("float".to_string()));
        assert_eq!(m2.len(), 1);
        v_eq(m2.get(&Value::Int(1)), Value::Str("float".to_string()), "Float で上書きを Int で検索");
    }

    #[test]
    fn set_basic() {
        let mut s = PSet::empty();
        for i in 0..100i64 {
            s = s.conj(Value::Int(i % 50));
        }
        assert_eq!(s.len(), 50);
        assert!(s.contains(&Value::Int(0)));
        assert!(!s.contains(&Value::Int(50)));
        let s2 = s.disj(&Value::Int(0));
        assert!(!s2.contains(&Value::Int(0)));
        assert_eq!(s2.len(), 49);
        assert!(s.contains(&Value::Int(0)), "元は不変");
    }

    #[test]
    fn hash_consistency() {
        use crate::types::values_equal;
        // 等しい値は同じハッシュ
        let pairs = [
            (Value::Int(1), Value::Float(1.0)),
            (Value::Int(-5), Value::Float(-5.0)),
            (Value::Str("a".into()), Value::Str("a".into())),
            (Value::Keyword("k".into()), Value::Keyword("k".into())),
            (Value::List(list::from_vec(vec![Value::Int(1)])), Value::List(list::from_vec(vec![Value::Int(1)]))),
            (Value::Vector(Arc::new(PVector::from_vec(vec![Value::Int(1)]))), Value::Vector(Arc::new(PVector::from_vec(vec![Value::Int(1)])))),
        ];
        for (a, b) in pairs {
            assert!(values_equal(&a, &b), "{:?} == {:?}", a, b);
            assert_eq!(hash_value(&a), hash_value(&b), "ハッシュ不一致: {:?} vs {:?}", a, b);
        }
    }
}
