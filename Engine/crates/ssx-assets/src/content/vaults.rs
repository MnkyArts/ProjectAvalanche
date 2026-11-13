use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ssx_formats::attrib::collection::{self, Collection, DecodedCollection, Field, FieldData};
use ssx_formats::attrib::{Schema, Value, Vault, fnv1};

pub fn fnv1_continue(mut h: u32, data: &[u8]) -> u32 {
    for &b in data {
        h = h.wrapping_mul(16_777_619) ^ u32::from(b);
    }
    h
}

#[derive(Default)]
pub struct Names {
    by_hash: HashMap<u32, Vec<String>>,
}

const SEPARATORS: &[u8] = b".\\/ ,;:='\"()[]{}<>";

impl Names {
    pub fn insert(&mut self, name: &str) {
        let list = self.by_hash.entry(fnv1(name.as_bytes())).or_default();
        if !list.iter().any(|n| n == name) {
            list.push(name.to_string());
        }
    }

    pub fn add_strings_from(&mut self, data: &[u8]) {
        for run in data.split(|b| !(0x20..0x7f).contains(b)).filter(|r| r.len() >= 2) {
            let mut add = |bytes: &[u8]| {
                if bytes.len() >= 2 {
                    self.insert(std::str::from_utf8(bytes).unwrap_or_default());
                }
            };
            add(run);
            for token in run.split(|b| SEPARATORS.contains(b)).filter(|t| t.len() < run.len()) {
                add(token);
            }
            if let Some(i) = run.windows(13).position(|w| w == b"ClassRefSpec_") {
                add(&run[i + 13..]);
            }
        }
    }

    pub fn add_token_windows(&mut self, max: usize) -> usize {
        let sources: Vec<String> = self.by_hash.values().flatten().cloned().collect();
        let before: usize = self.by_hash.values().map(Vec::len).sum();
        for s in sources {
            let lower = s.to_ascii_lowercase();
            for text in [s.as_str(), lower.as_str()] {
                let tokens: Vec<&str> = text.split('_').collect();
                for i in 0..tokens.len() {
                    for j in i + 1..=(i + max).min(tokens.len()) {
                        let w = tokens[i..j].join("_");
                        if w.len() >= 2 {
                            self.insert(&w);
                        }
                    }
                }
            }
        }
        self.by_hash.values().map(Vec::len).sum::<usize>() - before
    }

    pub fn candidates(&self, hash: u32) -> &[String] {
        self.by_hash.get(&hash).map_or(&[], Vec::as_slice)
    }

    pub fn unique(&self, hash: u32) -> Option<&str> {
        match self.candidates(hash) {
            [one] => Some(one),
            _ => None,
        }
    }
}

pub struct VaultFile {
    pub file: String,
    pub cols: Vec<Collection>,
    pub decoded: Vec<DecodedCollection>,
    pub index: HashMap<(u32, u32), usize>,
}

pub struct VaultIndex {
    pub names: Names,
    pub vaults: Vec<VaultFile>,
    pub classes: HashMap<u32, String>,
    pub unknown_classes: Vec<(u32, usize)>,
}

pub fn files_with_ext(root: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    let mut dirs = vec![root.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).with_context(|| format!("listing {}", d.display()))? {
            let e = e.with_context(|| format!("listing {}", d.display()))?;
            let p = e.path();
            let t = e.file_type().with_context(|| format!("reading {}", p.display()))?;
            if t.is_dir() {
                dirs.push(p);
            } else if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn archives(big: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(big).with_context(|| format!("listing {}", big.display()))? {
        let e = e.with_context(|| format!("listing {}", big.display()))?;
        if e.file_type().with_context(|| format!("reading {}", e.path().display()))?.is_dir() {
            out.push(e.path());
        }
    }
    out.sort();
    Ok(out)
}

pub fn rel(root: &Path, path: &Path) -> String {
    let r = path.strip_prefix(root).unwrap_or(path);
    r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

impl VaultIndex {
    pub fn load(big: &Path) -> Result<Self> {
        let schema_vlt = big.join("bootcache/data/xenon/attribdb/attribdb_schema.vlt");
        let read = |p: &Path| std::fs::read(p).with_context(|| format!("reading {}", p.display()));
        let (svlt, sbin) = (read(&schema_vlt)?, read(&schema_vlt.with_extension("bin"))?);
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
        let mut names = Names::default();
        let mut classes = HashMap::new();
        for t in &schema.types {
            names.insert(&t.name);
        }
        let known: std::collections::HashSet<u32> = schema.classes.iter().map(|c| c.key).collect();
        let mut vaults = Vec::new();
        let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut unknown: BTreeMap<u32, usize> = BTreeMap::new();
        for vlt_path in files_with_ext(big, "vlt")? {
            let (vlt, bin) = (read(&vlt_path)?, read(&vlt_path.with_extension("bin"))?);
            names.add_strings_from(&vlt);
            names.add_strings_from(&bin);
            let v = Vault::parse(&vlt, &bin).with_context(|| format!("parsing {}", vlt_path.display()))?;
            let cols = collection::collections(&v)?;
            let mut kept_cols = Vec::with_capacity(cols.len());
            let mut decoded = Vec::with_capacity(cols.len());
            for c in cols {
                if !seen.insert((c.class, c.key)) {
                    continue;
                }
                if !known.contains(&c.class) {
                    *unknown.entry(c.class).or_default() += 1;
                    continue;
                }
                let d =
                    c.decode(&v, &schema).with_context(|| format!("decoding {} of {}", c.key, vlt_path.display()))?;
                kept_cols.push(c);
                decoded.push(d);
            }
            if kept_cols.is_empty() {
                continue;
            }
            let index = collection::index(&kept_cols);
            vaults.push(VaultFile { file: rel(big, &vlt_path), cols: kept_cols, decoded, index });
        }
        for ext in ["fxo", "lua"] {
            for p in files_with_ext(big, ext)? {
                names.add_strings_from(&read(&p)?);
            }
        }
        for c in &schema.classes {
            if let Some(n) = names.unique(c.key) {
                classes.insert(c.key, n.to_string());
            }
        }
        Ok(VaultIndex { names, vaults, classes, unknown_classes: unknown.into_iter().collect() })
    }

    pub fn class_name(&self, class: u32) -> Option<&str> {
        self.classes.get(&class).map(String::as_str)
    }

    pub fn collection_name(&self, c: &Collection) -> Option<&str> {
        let class = self.class_name(c.class)?;
        let mut confirmed =
            self.names.candidates(c.key).iter().filter(|n| fnv1(format!("{class}/{n}").as_bytes()) == c.export_key);
        match (confirmed.next(), confirmed.next()) {
            (Some(n), None) => Some(n.as_str()),
            _ => None,
        }
    }

    pub fn of_class(&self, class: u32) -> impl Iterator<Item = (&VaultFile, usize)> + '_ {
        self.vaults.iter().flat_map(move |v| {
            v.cols.iter().enumerate().filter(move |(_, c)| c.class == class).map(move |(i, _)| (v, i))
        })
    }

    pub fn fields<'a>(&self, v: &'a VaultFile, i: usize) -> Result<Vec<&'a Field>> {
        Ok(collection::with_inherited(&v.cols, &v.decoded, &v.index, i)?.into_iter().map(|(_, f)| f).collect())
    }

    pub fn recover_names(&mut self, window: usize, rounds: usize) -> Vec<usize> {
        let confirmed = |idx: &VaultIndex| {
            idx.vaults.iter().flat_map(|v| &v.cols).filter(|c| idx.collection_name(c).is_some()).count()
        };
        let mut out = vec![confirmed(self)];
        self.names.add_token_windows(window);
        out.push(confirmed(self));
        let windows = self.lower_windows(window);
        for _ in 0..rounds {
            self.add_suffixed_names(&windows);
            let n = confirmed(self);
            if Some(&n) == out.last() {
                break;
            }
            out.push(n);
        }
        out
    }

    fn lower_windows(&self, max: usize) -> Vec<(String, u32)> {
        let mut set = std::collections::HashSet::new();
        for s in self.names.by_hash.values().flatten() {
            let lower = s.to_ascii_lowercase();
            let tokens: Vec<&str> = lower.split('_').collect();
            for i in 0..tokens.len() {
                for j in i + 1..=(i + max).min(tokens.len()) {
                    set.insert(tokens[i..j].join("_"));
                }
            }
        }
        let mut out: Vec<(String, u32)> = set
            .into_iter()
            .map(|w| {
                let h = fnv1(w.as_bytes());
                (w, h)
            })
            .collect();
        out.sort();
        out
    }

    fn add_suffixed_names(&mut self, windows: &[(String, u32)]) {
        let mut suffixes: HashMap<u32, std::collections::BTreeSet<String>> = HashMap::new();
        let mut open: HashMap<u32, std::collections::HashSet<u32>> = HashMap::new();
        for v in &self.vaults {
            for c in &v.cols {
                match self.collection_name(c) {
                    Some(n) => {
                        if let Some((_, last)) = n.rsplit_once('_') {
                            suffixes.entry(c.class).or_default().insert(last.to_ascii_lowercase());
                        }
                    }
                    None => drop(open.entry(c.class).or_default().insert(c.key)),
                }
            }
        }
        for (class, name) in &self.classes {
            if let Some((_, last)) = name.rsplit_once('_') {
                suffixes.entry(*class).or_default().insert(last.to_string());
            }
        }
        let mut by_suffix: std::collections::BTreeMap<String, std::collections::HashSet<u32>> = Default::default();
        for (class, keys) in &open {
            for s in suffixes.get(class).into_iter().flatten() {
                by_suffix.entry(format!("_{s}")).or_default().extend(keys);
            }
        }
        let mut found = Vec::new();
        for (w, state) in windows {
            for (suffix, keys) in &by_suffix {
                if keys.contains(&fnv1_continue(*state, suffix.as_bytes())) {
                    found.push(format!("{w}{suffix}"));
                }
            }
        }
        for n in found {
            self.names.insert(&n);
        }
    }

    pub fn class_stats(&self) -> BTreeMap<String, (usize, usize)> {
        let mut out: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for v in &self.vaults {
            for c in &v.cols {
                let k = self.class_name(c.class).map_or_else(|| format!("{:08x}", c.class), str::to_string);
                let e = out.entry(k).or_default();
                e.0 += 1;
                e.1 += usize::from(self.collection_name(c).is_some());
            }
        }
        out
    }
}

pub fn text(d: &FieldData) -> Option<&str> {
    let v = match d {
        FieldData::Single(v) => v,
        FieldData::Array(vs) => vs.first()?,
    };
    if let Value::Text(Some(t)) = v { Some(t.as_str()) } else { None }
}
