use std::collections::BTreeSet;
use std::path::Path;

pub const KNOWN: [(&str, &str); 6] = [
    ("eng_us", "English"),
    ("fre_fr", "French"),
    ("ger_de", "German"),
    ("spa_es", "Spanish"),
    ("ita_it", "Italian"),
    ("jpn_jp", "Japanese"),
];

const MAX_ROOTS: usize = 64;
const MAX_FILES: usize = 64;

pub fn available(big_root: &Path) -> Vec<(&'static str, &'static str)> {
    let found = scan(big_root);
    KNOWN.iter().copied().filter(|(code, _)| found.contains(*code)).collect()
}

fn scan(big_root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    stems(&big_root.join("data/xenon/loc"), &mut out);
    let Ok(entries) = std::fs::read_dir(big_root) else { return out };
    for entry in entries.flatten().take(MAX_ROOTS) {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            stems(&entry.path().join("data/xenon/loc"), &mut out);
        }
    }
    out
}

fn stems(dir: &Path, out: &mut BTreeSet<String>) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten().take(MAX_FILES) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "db")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            out.insert(stem.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pa-language-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (archive, lang) in [("bootcache", "eng_us"), ("bootcache", "fre_fr"), ("cache0", "jpn_jp")] {
            let dir = root.join(archive).join("data/xenon/loc");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{lang}.db")), b"").unwrap();
        }
        std::fs::create_dir_all(root.join("cache1/data/xenon/loc")).unwrap();
        std::fs::write(root.join("cache1/data/xenon/loc/notes.txt"), b"").unwrap();
        root
    }

    #[test]
    fn the_discs_languages_are_listed_in_the_games_order() {
        let root = folder();
        assert_eq!(available(&root), [("eng_us", "English"), ("fre_fr", "French"), ("jpn_jp", "Japanese")].to_vec());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_folder_offers_nothing() {
        assert!(available(Path::new("/nonexistent/pa-language")).is_empty());
    }
}
