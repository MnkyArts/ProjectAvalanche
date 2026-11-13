use crate::number::to_int;
use crate::value::{GcRef, Value};

const MAXBITS: usize = 26;
const MAXASIZE: i64 = 1 << MAXBITS;

#[derive(Clone, Debug, Default)]
struct Node {
    key: Value,
    val: Value,
    next: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Table {
    array: Vec<Value>,
    node: Vec<Node>,
    lsizenode: u8,
    lastfree: usize,
    pub meta: Option<GcRef>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidKey;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyError {
    Nil,
    NaN,
}

// PORT(choice): these keys hash by the heap object's index; the game hashes them by their memory address, so their
// order in `next` cannot match the game's (addresses are run-time values).
fn pointer_hash(v: &Value) -> u32 {
    match v {
        Value::LightUserData(p) => *p as u32,
        _ => v.gc().map_or(0, |r| 0x4000_0000 | (r.0 << 5)),
    }
}

fn ceil_log2(x: usize) -> usize {
    let mut l = 0;
    while (1usize << l) < x {
        l += 1;
    }
    l
}

fn array_index(k: &Value) -> i64 {
    if let Value::Num(n) = k {
        let i = to_int(*n);
        if f64::from(i) == *n {
            return i64::from(i);
        }
    }
    -1
}

impl Table {
    pub fn new(narray: usize, nhash: usize) -> Table {
        let mut t = Table::default();
        t.array.resize(narray, Value::Nil);
        t.set_node_vector(nhash);
        t
    }

    fn size_node(&self) -> usize {
        1 << self.lsizenode
    }

    fn is_dummy(&self) -> bool {
        self.node.is_empty()
    }

    fn set_node_vector(&mut self, size: usize) {
        if size == 0 {
            self.node = Vec::new();
            self.lsizenode = 0;
            self.lastfree = 0;
        } else {
            let l = ceil_log2(size);
            self.lsizenode = l as u8;
            self.node = vec![Node::default(); 1 << l];
            self.lastfree = 1 << l;
        }
    }

    fn main_position(&self, k: &Value) -> usize {
        let size = self.size_node();
        let hashmod = |n: u32| (n as usize) % ((size - 1) | 1);
        match k {
            Value::Num(n) => {
                if *n == 0.0 {
                    return 0;
                }
                let b = n.to_bits();
                hashmod(((b >> 32) as u32).wrapping_add(b as u32))
            }
            Value::Str(s) => (s.hash() as usize) & (size - 1),
            Value::Bool(b) => usize::from(*b) & (size - 1),
            _ => hashmod(pointer_hash(k)),
        }
    }

    fn find_node(&self, k: &Value) -> Option<usize> {
        if self.is_dummy() {
            return None;
        }
        let mut n = self.main_position(k);
        loop {
            if self.node[n].key.raw_eq(k) {
                return Some(n);
            }
            n = self.node[n].next? as usize;
        }
    }

    pub fn get(&self, k: &Value) -> Value {
        let i = array_index(k);
        if i >= 1 && (i as usize) <= self.array.len() {
            return self.array[i as usize - 1].clone();
        }
        match k {
            Value::Nil => Value::Nil,
            _ => self.find_node(k).map(|n| self.node[n].val.clone()).unwrap_or_default(),
        }
    }

    pub fn get_int(&self, i: i64) -> Value {
        if i >= 1 && (i as usize) <= self.array.len() {
            return self.array[i as usize - 1].clone();
        }
        self.get(&Value::Num(i as f64))
    }

    pub fn get_str(&self, k: &str) -> Value {
        self.get(&Value::str(k))
    }

    pub fn set(&mut self, k: Value, v: Value) -> Result<(), KeyError> {
        let i = array_index(&k);
        if i >= 1 && (i as usize) <= self.array.len() {
            self.array[i as usize - 1] = v;
            return Ok(());
        }
        if let Some(n) = self.find_node(&k) {
            self.node[n].val = v;
            return Ok(());
        }
        match k {
            Value::Nil => return Err(KeyError::Nil),
            Value::Num(n) if n.is_nan() => return Err(KeyError::NaN),
            _ => {}
        }
        let n = self.new_key(k);
        match n {
            Slot::Array(i) => self.array[i] = v,
            Slot::Node(n) => self.node[n].val = v,
        }
        Ok(())
    }

    fn free_pos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            if self.node[self.lastfree].key.is_nil() {
                return Some(self.lastfree);
            }
        }
        None
    }

    fn new_key(&mut self, k: Value) -> Slot {
        let mp = self.main_position(&k);
        if self.is_dummy() || !self.node[mp].val.is_nil() {
            let Some(f) = self.free_pos() else {
                self.rehash(&k);
                let i = array_index(&k);
                if i >= 1 && (i as usize) <= self.array.len() {
                    return Slot::Array(i as usize - 1);
                }
                if let Some(n) = self.find_node(&k) {
                    return Slot::Node(n);
                }
                return self.new_key(k);
            };
            let other = self.main_position(&self.node[mp].key.clone());
            if other != mp {
                let mut o = other;
                while self.node[o].next != Some(mp as u32) {
                    o = self.node[o].next.unwrap_or(0) as usize;
                }
                self.node[o].next = Some(f as u32);
                self.node[f] = self.node[mp].clone();
                self.node[mp].next = None;
                self.node[mp].val = Value::Nil;
                self.node[mp].key = k;
                return Slot::Node(mp);
            }
            self.node[f].next = self.node[mp].next;
            self.node[mp].next = Some(f as u32);
            self.node[f].key = k;
            return Slot::Node(f);
        }
        self.node[mp].key = k;
        Slot::Node(mp)
    }

    fn rehash(&mut self, extra: &Value) {
        let mut nums = [0i64; MAXBITS + 1];
        let mut nasize = self.num_use_array(&mut nums);
        let mut total = nasize;
        total += self.num_use_hash(&mut nums, &mut nasize);
        nasize += count_int(extra, &mut nums);
        total += 1;
        let na = compute_sizes(&nums, &mut nasize);
        self.resize(nasize as usize, (total - na) as usize);
    }

    fn num_use_array(&self, nums: &mut [i64]) -> i64 {
        let mut ause = 0;
        let mut i = 1usize;
        let mut ttlg = 1usize;
        for slot in nums.iter_mut().take(MAXBITS + 1) {
            let mut lc = 0;
            let mut lim = ttlg;
            if lim > self.array.len() {
                lim = self.array.len();
                if i > lim {
                    break;
                }
            }
            while i <= lim {
                if !self.array[i - 1].is_nil() {
                    lc += 1;
                }
                i += 1;
            }
            *slot += lc;
            ause += lc;
            ttlg *= 2;
        }
        ause
    }

    fn num_use_hash(&self, nums: &mut [i64], nasize: &mut i64) -> i64 {
        let mut total = 0;
        let mut ause = 0;
        for n in self.node.iter().rev() {
            if !n.val.is_nil() {
                ause += count_int(&n.key, nums);
                total += 1;
            }
        }
        *nasize += ause;
        total
    }

    fn resize(&mut self, nasize: usize, nhsize: usize) {
        let oldasize = self.array.len();
        let old = std::mem::take(&mut self.node);
        if nasize > oldasize {
            self.array.resize(nasize, Value::Nil);
        }
        self.set_node_vector(nhsize);
        if nasize < oldasize {
            let vanishing: Vec<Value> = self.array.drain(nasize..).collect();
            for (j, v) in vanishing.into_iter().enumerate() {
                if !v.is_nil() {
                    let _ = self.set(Value::Num((nasize + j + 1) as f64), v);
                }
            }
        }
        for n in old.into_iter().rev() {
            if !n.val.is_nil() {
                let _ = self.set(n.key, n.val);
            }
        }
    }

    pub fn resize_array(&mut self, nasize: usize) {
        let nh = if self.is_dummy() { 0 } else { self.size_node() };
        self.resize(nasize, nh);
    }

    pub fn array_len(&self) -> usize {
        self.array.len()
    }

    pub fn length(&self) -> i64 {
        let mut j = self.array.len();
        if j > 0 && self.array[j - 1].is_nil() {
            let mut i = 0;
            while j - i > 1 {
                let m = (i + j) / 2;
                if self.array[m - 1].is_nil() { j = m } else { i = m }
            }
            return i as i64;
        }
        if self.is_dummy() {
            return j as i64;
        }
        let mut i = j as i64;
        let mut j = i + 1;
        while !self.get_int(j).is_nil() {
            i = j;
            j *= 2;
            if j > i64::from(i32::MAX) {
                let mut i = 1;
                while !self.get_int(i).is_nil() {
                    i += 1;
                }
                return i - 1;
            }
        }
        while j - i > 1 {
            let m = (i + j) / 2;
            if self.get_int(m).is_nil() { j = m } else { i = m }
        }
        i
    }

    pub fn next(&self, k: &Value) -> Result<Option<(Value, Value)>, InvalidKey> {
        let mut i = self.find_index(k)?;
        i += 1;
        while (i as usize) < self.array.len() {
            let v = &self.array[i as usize];
            if !v.is_nil() {
                return Ok(Some((Value::Num((i + 1) as f64), v.clone())));
            }
            i += 1;
        }
        let mut n = (i - self.array.len() as i64) as usize;
        while n < self.node.len() {
            if !self.node[n].val.is_nil() {
                return Ok(Some((self.node[n].key.clone(), self.node[n].val.clone())));
            }
            n += 1;
        }
        Ok(None)
    }

    fn find_index(&self, k: &Value) -> Result<i64, InvalidKey> {
        if k.is_nil() {
            return Ok(-1);
        }
        let i = array_index(k);
        if i >= 1 && (i as usize) <= self.array.len() {
            return Ok(i - 1);
        }
        match self.find_node(k) {
            Some(n) => Ok((n + self.array.len()) as i64),
            None => Err(InvalidKey),
        }
    }

    pub fn for_each_ref(&self, mut f: impl FnMut(&Value)) {
        self.array.iter().for_each(&mut f);
        for n in &self.node {
            if !n.val.is_nil() {
                f(&n.key);
                f(&n.val);
            }
        }
    }
}

enum Slot {
    Array(usize),
    Node(usize),
}

fn count_int(k: &Value, nums: &mut [i64]) -> i64 {
    let i = array_index(k);
    if i > 0 && i <= MAXASIZE {
        nums[ceil_log2(i as usize)] += 1;
        1
    } else {
        0
    }
}

fn compute_sizes(nums: &[i64], narray: &mut i64) -> i64 {
    let mut a = 0;
    let mut na = 0;
    let mut n = 0;
    let mut twotoi: i64 = 1;
    let mut i = 0;
    while twotoi / 2 < *narray {
        if nums[i] > 0 {
            a += nums[i];
            if a > twotoi / 2 {
                n = twotoi;
                na = a;
            }
        }
        if a == *narray {
            break;
        }
        i += 1;
        twotoi *= 2;
    }
    *narray = n;
    na
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(t: &Table) -> Vec<String> {
        let mut out = Vec::new();
        let mut k = Value::Nil;
        while let Some((nk, _)) = t.next(&k).expect("next") {
            out.push(match &nk {
                Value::Num(n) => format!("{n}"),
                Value::Str(s) => s.to_str_lossy(),
                _ => "?".into(),
            });
            k = nk;
        }
        out
    }

    #[test]
    fn array_and_hash_parts() {
        let mut t = Table::new(0, 0);
        for i in 1..=10 {
            t.set(Value::Num(i as f64), Value::Num(i as f64)).unwrap();
        }
        assert_eq!(t.length(), 10);
        assert_eq!(t.array_len(), 16);
        t.set(Value::Num(10.0), Value::Nil).unwrap();
        assert_eq!(t.length(), 9);
        assert_eq!(t.set(Value::Nil, Value::Num(1.0)), Err(KeyError::Nil));
        assert_eq!(t.set(Value::Num(f64::NAN), Value::Num(1.0)), Err(KeyError::NaN));
    }

    #[test]
    fn hash_order_is_lua_5_1() {
        let mut t = Table::new(0, 0);
        for (k, v) in [("a", 1.0), ("b", 2.0), ("c", 3.0)] {
            t.set(Value::str(k), Value::Num(v)).unwrap();
        }
        assert_eq!(keys(&t), vec!["a", "c", "b"]);
        let mut t2 = Table::new(0, 4);
        for k in ["x", "y", "z", "w"] {
            t2.set(Value::str(k), Value::Num(1.0)).unwrap();
        }
        assert_eq!(keys(&t2), vec!["y", "x", "w", "z"]);
        let mut t3 = Table::new(0, 0);
        for i in [1.0, 2.0, 4.0] {
            t3.set(Value::Num(i), Value::Num(i)).unwrap();
        }
        assert_eq!(t3.length(), 4);
    }
}
