use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use ssx_content::hash::{ContentHash, ContentIndex};

use super::*;
use crate::setup::{ARCHIVES, MARKER};

pub(crate) struct Temp(pub PathBuf);

impl Temp {
    pub(crate) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ssx-folder-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug)]
enum Setup {
    Format1,
    V020,
    V030,
    Now,
}

fn files(dir: &Path, movies: bool) -> ContentIndex {
    let mut index = ContentIndex::default();
    let mut put = |rel: &str, bytes: &[u8]| {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        index.entries.insert(rel.to_string(), (bytes.len() as u64, ContentHash::of(bytes)));
    };
    for a in ARCHIVES {
        put(&format!("big/{a}/data/m_{a}.bin"), a.as_bytes());
    }
    put("big/cache1/data/xenon/attribdb/ge_attribdb.vlt", b"vault");
    if movies {
        put("disc/data/common/fmv/alps.vp6", b"movie");
    }
    index
}

fn marker_of(format: u64, complete: bool, movies: bool) -> Value {
    let finished: serde_json::Map<String, Value> =
        ARCHIVES.iter().map(|a| (a.to_string(), json!({ "members": 1, "written": 1, "kept": 0 }))).collect();
    let mut m =
        json!({ "format": format, "fingerprint": "660dcce05812cf68", "finished": finished, "complete": complete });
    if movies {
        m["movies"] = json!({ "files": 1, "written": 1, "kept": 0, "bytes": 5 });
    }
    m
}

fn write_marker(dir: &Path, m: &Value) {
    fs::write(dir.join(MARKER), serde_json::to_string_pretty(m).unwrap()).unwrap();
}

fn folder(dir: &Path, setup: Setup) -> ContentIndex {
    let movies = !matches!(setup, Setup::Format1);
    let index = files(dir, movies);
    let mut m = marker_of(if movies { 2 } else { 1 }, true, movies);
    let text = index.to_tsv();
    let bytes: u64 = index.entries.values().map(|(s, _)| s).sum();
    match setup {
        Setup::Format1 | Setup::V020 => {}
        Setup::V030 => m["content_index"] = json!({ "files": index.entries.len(), "bytes": bytes }),
        Setup::Now => {
            let hash = ContentHash::of(text.as_bytes()).to_string();
            m["content_index"] = json!({ "files": index.entries.len(), "bytes": bytes, "blake3": hash });
        }
    }
    if matches!(setup, Setup::V030 | Setup::Now) {
        fs::write(dir.join(INDEX_FILE), &text).unwrap();
    }
    write_marker(dir, &m);
    index
}

#[test]
fn each_earlier_setups_folder_is_told_apart() {
    let cases = [
        (Setup::Format1, State::NoMovies),
        (Setup::V020, State::NoIndex),
        (Setup::V030, State::Complete { marked: true, files: 16 }),
        (Setup::Now, State::Complete { marked: true, files: 16 }),
    ];
    for (setup, want) in cases {
        let t = Temp::new(&format!("{setup:?}"));
        let index = folder(&t.0, setup);
        let got = check(&t.0);
        assert_eq!(got.state, want, "{setup:?}");
        assert_eq!(got.index.is_some(), want.is_complete(), "{setup:?}");
        if want.is_complete() {
            assert_eq!(got.index.unwrap(), index);
        }
    }
}

#[test]
fn a_v020_folder_is_not_complete_and_names_the_repair() {
    let t = Temp::new("v020");
    folder(&t.0, Setup::V020);
    let state = state(&t.0);
    assert!(state.playable() && !state.is_complete() && state.needs_repair());
    let err = content_index(&t.0).unwrap_err();
    assert_eq!(err.state, State::NoIndex);
    let text = err.to_string();
    assert!(text.contains("no content index") && text.contains("Repair game data"), "{text}");
    assert!(!text.contains("os error"), "{text}");
}

#[test]
fn unfinished_missing_and_unmarked_folders() {
    let t = Temp::new("states");
    assert_eq!(state(&t.0), State::Missing);
    assert_eq!(state(&t.0.join("nothing-here")), State::Missing);
    files(&t.0, true);
    assert_eq!(state(&t.0), State::NoIndex, "an archive tree without a marker or an index");
    let index = files(&t.0, true);
    fs::write(t.0.join(INDEX_FILE), index.to_tsv()).unwrap();
    assert_eq!(state(&t.0), State::Complete { marked: false, files: 16 }, "a developer's extraction");
    write_marker(&t.0, &marker_of(2, false, false));
    assert_eq!(state(&t.0), State::Unfinished);
    assert!(!State::Unfinished.playable() && !State::Unfinished.needs_repair());
}

#[test]
fn a_damaged_index_is_found() {
    let t = Temp::new("damaged");
    let index = folder(&t.0, Setup::Now);
    let text = index.to_tsv();
    let row = |p: &str| format!("{p}\t3\t{}\n", ContentHash::of(b"abc"));
    let first = index.entries.keys().next().unwrap().clone();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated", text.as_bytes()[..text.len() - 20].to_vec()),
        ("empty", Vec::new()),
        ("no header", text.lines().skip(1).map(|l| format!("{l}\n")).collect::<String>().into_bytes()),
        ("outside", format!("{text}{}", row("big/data0/../../x")).into_bytes()),
        ("not an archive", format!("{text}{}", row("big/notanarchive/x.bin")).into_bytes()),
        ("no setup file", format!("{text}{}", row("settings.json")).into_bytes()),
        ("a movie elsewhere", format!("{text}{}", row("disc/data/x.vp6")).into_bytes()),
        ("a device name", format!("{text}{}", row("big/data0/con.txt")).into_bytes()),
        ("case", format!("{text}{}", row(&first.to_ascii_uppercase().replacen("BIG/", "big/", 1))).into_bytes()),
        ("twice", format!("{text}{}", row(&first)).into_bytes()),
        ("unsorted", format!("{text}{}", row("big/bootcache/a")).into_bytes()),
        ("crlf", text.replace('\n', "\r\n").into_bytes()),
        ("no final newline", text.trim_end().as_bytes().to_vec()),
        ("not utf-8", [text.as_bytes(), b"big/data0/\xff\t1\tx\n"].concat()),
        ("a bad size", text.replacen("\t5\t", "\t05\t", 1).into_bytes()),
        ("a bad hash", text.replacen(&ContentHash::of(b"movie").to_string(), &"0".repeat(63), 1).into_bytes()),
        ("a row changed", text.replacen(&ContentHash::of(b"movie").to_string(), &"0".repeat(64), 1).into_bytes()),
    ];
    for (what, bytes) in cases {
        fs::write(t.0.join(INDEX_FILE), &bytes).unwrap();
        let s = state(&t.0);
        assert!(matches!(s, State::IndexDamaged(_)), "{what}: {s:?}");
        assert!(s.playable() && s.needs_repair(), "{what}");
        assert!(content_index(&t.0).is_err(), "{what}");
    }
    let mut big = text.clone().into_bytes();
    big.resize(MAX_INDEX_BYTES as usize + 1, b'x');
    fs::write(t.0.join(INDEX_FILE), &big).unwrap();
    assert!(matches!(state(&t.0), State::IndexDamaged(w) if w.contains("larger than")));
    fs::remove_file(t.0.join(INDEX_FILE)).unwrap();
    assert!(matches!(state(&t.0), State::IndexDamaged(w) if w.contains("missing")));
    fs::create_dir(t.0.join(INDEX_FILE)).unwrap();
    assert!(matches!(state(&t.0), State::IndexDamaged(w) if w.contains("not a file")));
    fs::remove_dir(t.0.join(INDEX_FILE)).unwrap();
    #[cfg(unix)]
    {
        let elsewhere = t.0.join("elsewhere.tsv");
        fs::write(&elsewhere, &text).unwrap();
        std::os::unix::fs::symlink(&elsewhere, t.0.join(INDEX_FILE)).unwrap();
        assert!(matches!(state(&t.0), State::IndexDamaged(w) if w.contains("link")), "never read through a link");
        fs::remove_file(t.0.join(INDEX_FILE)).unwrap();
    }
    fs::write(t.0.join(INDEX_FILE), &text).unwrap();
    assert!(state(&t.0).is_complete(), "the original again");
}

#[test]
fn a_complete_folders_files_are_checked_against_its_rows() {
    let t = Temp::new("files");
    folder(&t.0, Setup::Now);
    let member = t.0.join("big/data0/data/m_data0.bin");
    let movie = t.0.join("disc/data/common/fmv/alps.vp6");
    let damaged = |what: &str, want: &str| {
        let s = state(&t.0);
        let State::FilesDamaged(why) = &s else { panic!("{what}: {s:?}") };
        assert!(why.contains(want), "{what}: {why}");
        assert!(!s.playable() && s.needs_repair() && !s.is_complete(), "{what}");
        let err = content_index(&t.0).unwrap_err();
        let text = err.to_string();
        let head = format!("the game data folder {}: files of it are missing or damaged (", t.0.display());
        assert!(text.starts_with(&head) && text.contains("Repair game data"), "{what}: {text}");
    };
    fs::remove_file(&member).unwrap();
    damaged("removed", r#"1 file: 1 missing; first "big/data0/data/m_data0.bin", missing"#);
    fs::write(&member, b"data").unwrap();
    damaged("truncated", r#"1 of another size; first "big/data0/data/m_data0.bin", 4 bytes"#);
    fs::write(&member, b"data0data0").unwrap();
    damaged("grown", "10 bytes");
    fs::remove_file(&member).unwrap();
    fs::create_dir(&member).unwrap();
    damaged("a folder where the file belongs", "1 not a plain file");
    fs::remove_dir(&member).unwrap();
    fs::write(&member, b"data0").unwrap();
    fs::remove_file(&movie).unwrap();
    fs::remove_file(t.0.join("big/cache8/data/m_cache8.bin")).unwrap();
    damaged("a movie and a member", "2 files: 2 missing");
    fs::remove_dir_all(t.0.join("disc")).unwrap();
    fs::write(t.0.join("disc"), b"").unwrap();
    damaged("a file where a folder belongs", "1 not a plain file");
    fs::remove_file(t.0.join("disc")).unwrap();
    fs::create_dir_all(movie.parent().unwrap()).unwrap();
    fs::write(&movie, b"movie").unwrap();
    fs::write(t.0.join("big/cache8/data/m_cache8.bin"), b"cache8").unwrap();
    assert!(state(&t.0).is_complete(), "as the setup left it");
    #[cfg(unix)]
    {
        let elsewhere = t.0.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("m_data0.bin"), b"data0").unwrap();
        fs::remove_file(&member).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("m_data0.bin"), &member).unwrap();
        damaged("a link where the file belongs", "1 not a plain file");
        fs::remove_file(&member).unwrap();
        fs::write(&member, b"data0").unwrap();
        fs::rename(t.0.join("big/data0/data"), elsewhere.join("data")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("data"), t.0.join("big/data0/data")).unwrap();
        damaged("a linked folder", r#"first "big/data0/data/m_data0.bin", not a plain file"#);
        fs::remove_file(t.0.join("big/data0/data")).unwrap();
        fs::rename(elsewhere.join("data"), t.0.join("big/data0/data")).unwrap();
        assert!(state(&t.0).is_complete());
    }
    fs::write(&member, b"DATA0").unwrap();
    assert!(state(&t.0).is_complete());
}

#[test]
fn an_unmarked_trees_files_are_checked_and_hashed_again() {
    let t = Temp::new("files-dev");
    let index = files(&t.0, true);
    fs::write(t.0.join(INDEX_FILE), index.to_tsv()).unwrap();
    fs::remove_file(t.0.join("big/cache3/data/m_cache3.bin")).unwrap();
    assert!(matches!(state(&t.0), State::FilesDamaged(w) if w.contains("m_cache3.bin")));
    let hashed = crate::content::hash::ensure_index(&t.0, 1).unwrap();
    assert_eq!(hashed.entries.len(), index.entries.len() - 1);
    assert_eq!(state(&t.0), State::Complete { marked: false, files: index.entries.len() - 1 });
}

#[test]
fn a_file_damage_names_its_counts_and_first_row() {
    let damage = FileDamage {
        rows: vec![
            ("big/data0/a".into(), Fault::Size(3)),
            ("big/data0/b".into(), Fault::Missing),
            ("big/data1/c".into(), Fault::Missing),
            ("disc/data/common/fmv/x.vp6".into(), Fault::NotPlain),
            ("disc/data/common/fmv/y.vp6".into(), Fault::Unreadable("denied".into())),
        ],
    };
    let text = damage.to_string();
    assert_eq!(
        text,
        r#"5 files: 2 missing, 1 not a plain file, 1 of another size, 1 unreadable; first "big/data0/a", 3 bytes"#
    );
    assert!(damage.below("big/data1/") && damage.below("disc/") && !damage.below("big/cache0/"));
    assert!(FileDamage::default().is_empty());
}

#[test]
fn a_v030_index_is_checked_against_its_counts() {
    let t = Temp::new("v030");
    let index = folder(&t.0, Setup::V030);
    let mut fewer = index.clone();
    fewer.entries.pop_first();
    fs::write(t.0.join(INDEX_FILE), fewer.to_tsv()).unwrap();
    assert!(matches!(state(&t.0), State::IndexDamaged(w) if w.contains("the marker records")));
}

type MarkerEdit = fn(&mut Value);

#[test]
fn a_damaged_marker_is_found() {
    let t = Temp::new("marker");
    folder(&t.0, Setup::Now);
    let good = crate::setup::read_marker(&t.0).unwrap();
    let edits: [(&str, MarkerEdit); 6] = [
        ("an archive missing", |m| _ = m["finished"].as_object_mut().unwrap().remove("cache8")),
        ("no movies", |m| _ = m.as_object_mut().unwrap().remove("movies")),
        ("a newer format", |m| m["format"] = json!(3)),
        ("no format", |m| m["format"] = json!("2")),
        ("a bad index record", |m| m["content_index"]["files"] = json!(-1)),
        ("a bad hash", |m| m["content_index"]["blake3"] = json!("abc")),
    ];
    for (what, edit) in edits {
        let mut m = good.clone();
        edit(&mut m);
        write_marker(&t.0, &m);
        assert!(matches!(state(&t.0), State::MarkerDamaged(_)), "{what}: {:?}", state(&t.0));
    }
    fs::write(t.0.join(MARKER), "{ not json").unwrap();
    let s = state(&t.0);
    assert!(matches!(s, State::MarkerDamaged(_)) && !s.playable() && s.needs_repair(), "{s:?}");
}

#[test]
fn only_the_setups_paths_are_rows() {
    for ok in ["big/data0/data/a.bin", "big/fecache/x/y.dds.$$$", "disc/data/common/fmv/alps.vp6"] {
        assert!(setup_path(ok), "{ok}");
    }
    for bad in [
        "big/data0",
        "big/data1/a",
        "big//a",
        "big/data0/a/../b",
        "disc/data/common/fmv/sub/a.vp6",
        "disc/data/common/fmv/a.bik",
        "disc/default.xex",
        "content.tsv",
        "big/data0/a\\b",
        "big/data0/aux.bin",
    ] {
        assert!(!setup_path(bad), "{bad}");
    }
}

#[test]
fn the_mod_tools_never_index_a_setups_files_from_disk() {
    let t = Temp::new("ensure");
    folder(&t.0, Setup::V020);
    let e = crate::content::hash::ensure_index(&t.0, 1).unwrap_err();
    assert!(format!("{e:#}").contains("Repair game data"), "{e:#}");
    assert!(!t.0.join(INDEX_FILE).exists(), "no index of the files as they are");
    let complete = folder(&t.0, Setup::Now);
    assert_eq!(crate::content::hash::ensure_index(&t.0, 1).unwrap(), complete);
    let dev = Temp::new("ensure-dev");
    let index = files(&dev.0, true);
    assert_eq!(crate::content::hash::ensure_index(&dev.0, 1).unwrap(), index);
    assert_eq!(state(&dev.0), State::Complete { marked: false, files: index.entries.len() });
}
