use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use ssx_content::hash::{ContentHash, hash_file};

use crate::folder::{self, INDEX_FILE, State};
use crate::setup::{self, ARCHIVES, MARKER, Progress, Summary};

use super::super::rows::ROWS_FILE;

const SECTOR: usize = 0x800;
const XDVDFS_MAGIC: &[u8; 20] = b"MICROSOFT*XBOX*MEDIA";

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: BTreeMap<String, Vec<u8>>,
}

impl Dir {
    fn insert(&mut self, path: &str, data: Vec<u8>) {
        match path.split_once('/') {
            Some((dir, rest)) => self.dirs.entry(dir.to_string()).or_default().insert(rest, data),
            None => _ = self.files.insert(path.to_string(), data),
        }
    }

    fn number(&self, next: &mut u32, out: &mut Vec<u32>) {
        out.push(*next);
        *next += 1;
        for d in self.dirs.values() {
            d.number(next, out);
        }
    }
}

fn iso(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut root = Dir::default();
    for (p, d) in files {
        root.insert(p, d.clone());
    }
    let mut tables = Vec::new();
    let mut next = 33;
    root.number(&mut next, &mut tables);
    let mut image = vec![0u8; next as usize * SECTOR];
    let mut vd = XDVDFS_MAGIC.to_vec();
    vd.extend_from_slice(&33u32.to_le_bytes());
    vd.extend_from_slice(&(SECTOR as u32).to_le_bytes());
    vd.extend_from_slice(&0x01CC_D7EE_978F_F808u64.to_le_bytes());
    vd.resize(0x7EC, 0);
    vd.extend_from_slice(XDVDFS_MAGIC);
    image[32 * SECTOR..33 * SECTOR].copy_from_slice(&vd);
    let mut tables = tables.into_iter().peekable();
    write_dir(&root, &mut tables, &mut image);
    image
}

fn write_dir(dir: &Dir, tables: &mut std::iter::Peekable<std::vec::IntoIter<u32>>, image: &mut Vec<u8>) {
    let at = tables.next().unwrap() as usize * SECTOR;
    let mut entries: Vec<(String, u32, u32, u8)> = Vec::new();
    for (name, sub) in &dir.dirs {
        let sector = *tables.peek().unwrap();
        write_dir(sub, tables, image);
        entries.push((name.clone(), sector, SECTOR as u32, 0x10));
    }
    for (name, data) in &dir.files {
        let sector = (image.len() / SECTOR) as u32;
        image.extend_from_slice(data);
        image.resize(image.len().next_multiple_of(SECTOR), 0);
        entries.push((name.clone(), sector, data.len() as u32, 0x80));
    }
    entries.sort_by_key(|e| e.0.to_ascii_lowercase());
    let mut table = Vec::new();
    for (i, (name, sector, size, attr)) in entries.iter().enumerate() {
        let len = (14 + name.len()).next_multiple_of(4);
        let right = if i + 1 < entries.len() { ((table.len() + len) / 4) as u16 } else { 0 };
        table.extend_from_slice(&0u16.to_le_bytes());
        table.extend_from_slice(&right.to_le_bytes());
        table.extend_from_slice(&sector.to_le_bytes());
        table.extend_from_slice(&size.to_le_bytes());
        table.extend_from_slice(&[*attr, name.len() as u8]);
        table.extend_from_slice(name.as_bytes());
        table.resize(table.len().next_multiple_of(4), 0xFF);
    }
    assert!(table.len() <= SECTOR);
    table.resize(SECTOR, 0xFF);
    image[at..at + SECTOR].copy_from_slice(&table);
}

fn xex() -> Vec<u8> {
    let be = |x: &mut Vec<u8>, v: u32| x.extend_from_slice(&v.to_be_bytes());
    let mut x = b"XEX2".to_vec();
    for v in [0, 0x400, 0, 0x100, 1, 0x0001_83FF, 0x80] {
        be(&mut x, v);
    }
    x.resize(0x80, 0);
    be(&mut x, 4 + 11);
    x.extend_from_slice(b"ssxvzf.exe\0");
    x.resize(0x100, 0);
    be(&mut x, 0x184);
    be(&mut x, 0);
    x.resize(x.len() + 0x100 + 12 + 20 + 4 + 20 + 16 + 16 + 4 + 20 + 4 + 4 + 4, 0);
    x.resize(0x400, 0);
    x
}

fn container(data: &[u8], decodes: bool) -> Vec<u8> {
    let mut c = b"chunklzx".to_vec();
    for v in [2, data.len() as u32, 0x10000, 1, 1] {
        c.extend_from_slice(&v.to_be_bytes());
    }
    let (payload, codec): (&[u8], u32) = if decodes { (data, 4) } else { (&[], 3) };
    c.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    c.extend_from_slice(&codec.to_be_bytes());
    c.extend_from_slice(payload);
    c
}

fn big(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let n = members.len();
    let mut dirs: Vec<String> = members.iter().map(|(p, _)| p.rsplit_once('/').unwrap().0.to_string()).collect();
    dirs.sort();
    dirs.dedup();
    let (path_size, dir_size) = (2 + 32, 32);
    let base = (0x30 + n * 17).next_multiple_of(16);
    let names = (n * path_size).next_multiple_of(16) + dirs.len() * dir_size;
    let mut data_at = (base + names).next_multiple_of(16);
    let mut index = Vec::new();
    let mut body = Vec::new();
    for (_, d) in members {
        index.push(((data_at >> 4) as u32, d.len() as u32, d.len() as u32, 0u32));
        body.push((data_at, d));
        data_at = (data_at + d.len()).next_multiple_of(16);
    }
    let mut a = vec![0u8; data_at];
    a[..2].copy_from_slice(b"EB");
    a[2..4].copy_from_slice(&3u16.to_be_bytes());
    a[4..8].copy_from_slice(&(n as u32).to_be_bytes());
    a[10] = 4;
    a[12..16].copy_from_slice(&(base as u32).to_be_bytes());
    a[16..20].copy_from_slice(&(names as u32).to_be_bytes());
    a[20] = path_size as u8;
    a[21] = dir_size as u8;
    a[22..24].copy_from_slice(&(dirs.len() as u16).to_be_bytes());
    a[24..32].copy_from_slice(&(data_at as u64).to_be_bytes());
    for (i, (o, z, s, h)) in index.iter().enumerate() {
        for (k, v) in [o, z, s, h].iter().enumerate() {
            a[0x30 + i * 16 + k * 4..][..4].copy_from_slice(&v.to_be_bytes());
        }
    }
    for (i, (p, _)) in members.iter().enumerate() {
        let (dir, name) = p.rsplit_once('/').unwrap();
        let at = base + i * path_size;
        let d = dirs.iter().position(|x| x == dir).unwrap() as u16;
        a[at..at + 2].copy_from_slice(&d.to_be_bytes());
        a[at + 2..at + 2 + name.len()].copy_from_slice(name.as_bytes());
    }
    let dirs_at = base + (n * path_size).next_multiple_of(16);
    for (i, d) in dirs.iter().enumerate() {
        a[dirs_at + i * dir_size..][..d.len()].copy_from_slice(d.as_bytes());
    }
    for (at, d) in body {
        a[at..at + d.len()].copy_from_slice(d);
    }
    a
}

struct Fixture {
    dir: folder::tests::Temp,
    iso: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
}

fn pattern(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|k| (k.wrapping_mul(31) ^ seed.wrapping_mul(97) ^ (k >> 9)) as u8).collect()
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = folder::tests::Temp::new(&format!("setup-{name}"));
        let mut disc = vec![("default.xex".to_string(), xex())];
        let mut files = BTreeMap::new();
        for (i, a) in ARCHIVES.iter().enumerate() {
            let plain = pattern(100 + i * 7, i);
            let decoded = format!("{a}: decoded member").into_bytes();
            let mut members = vec![
                (format!("data/a_{i}.bin"), plain.clone()),
                ("data/sub/b.bin".to_string(), container(&decoded, true)),
            ];
            files.insert(format!("big/{a}/data/a_{i}.bin"), plain);
            files.insert(format!("big/{a}/data/sub/b.bin"), decoded);
            if *a == "data0" {
                let large = pattern(2 * (1 << 20) + 12_345, 99);
                members.push(("data/large.bin".to_string(), large.clone()));
                files.insert("big/data0/data/large.bin".to_string(), large);
            }
            if *a == "cache3" {
                members.push(("data/common/ghosts/bad.bin".to_string(), container(b"never", false)));
            }
            disc.push((format!("{a}.big"), big(&members)));
        }
        for (name, len) in [("alps.vp6", 3000), ("intro.vp6", (1 << 20) + 777)] {
            let bytes = pattern(len, len);
            disc.push((format!("data/common/fmv/{name}"), bytes.clone()));
            files.insert(format!("disc/data/common/fmv/{name}"), bytes);
        }
        let iso = dir.0.join("ssx.iso");
        fs::write(&iso, self::iso(&disc)).unwrap();
        Fixture { dir, iso, files }
    }

    fn out(&self) -> PathBuf {
        self.dir.0.join("GameData")
    }

    fn run(&self) -> Summary {
        setup::run(&self.iso, &self.out(), &AtomicBool::new(false), &mut |_| {}).unwrap()
    }

    fn run_cancelled_at(&self, n: usize) {
        let cancel = AtomicBool::new(false);
        let err = setup::run(&self.iso, &self.out(), &cancel, &mut |p: &Progress| {
            if p.archive_index == n {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
        assert!(err.is::<setup::Cancelled>(), "{err:#}");
    }

    fn marker(&self) -> Value {
        setup::read_marker(&self.out()).unwrap()
    }

    fn set_marker(&self, m: &Value) {
        fs::write(self.out().join(MARKER), serde_json::to_string_pretty(m).unwrap()).unwrap();
    }

    fn assert_complete(&self) {
        let out = self.out();
        let checked = folder::check(&out);
        assert_eq!(checked.state, State::Complete { marked: true, files: self.files.len() });
        let index = checked.index.unwrap();
        for (path, bytes) in &self.files {
            let (size, hash) = index.entries[path];
            assert_eq!((size, hash), (bytes.len() as u64, ContentHash::of(bytes)), "{path} in the index");
            assert_eq!(hash_file(&out.join(path)).unwrap(), (size, hash), "{path} on disk");
            assert!(!fs::symlink_metadata(out.join(path)).unwrap().is_symlink(), "{path}");
        }
        assert!(!out.join(ROWS_FILE).exists(), "the rows file is gone");
        assert!(!out.join("big/cache3/data/common/ghosts/bad.bin").exists(), "a failed member is not written");
    }

    fn as_v020(&self) {
        let mut m = self.marker();
        m.as_object_mut().unwrap().remove("content_index");
        for a in ARCHIVES {
            m["finished"][a].as_object_mut().unwrap().remove("rows");
            m["finished"][a].as_object_mut().unwrap().remove("replaced");
        }
        m["movies"].as_object_mut().unwrap().remove("rows");
        self.set_marker(&m);
        fs::remove_file(self.out().join(INDEX_FILE)).unwrap();
        assert_eq!(folder::state(&self.out()), State::NoIndex);
    }
}

fn index_text(out: &Path) -> String {
    fs::read_to_string(out.join(INDEX_FILE)).unwrap()
}

#[test]
fn a_setup_writes_every_file_and_the_index_of_the_images_bytes() {
    let f = Fixture::new("fresh");
    let s = f.run();
    assert_eq!(s.extracted.len(), ARCHIVES.len());
    assert_eq!((s.members_written, s.members_kept, s.members_replaced), (f.files.len() - 2, 0, 0));
    assert_eq!((s.movies_written, s.movies_kept), (2, 0));
    assert_eq!(s.failures.len(), 1, "{:?}", s.failures);
    assert_eq!(s.content_files, f.files.len());
    f.assert_complete();
    let again = f.run();
    assert_eq!(again.already_done.len(), ARCHIVES.len());
    assert_eq!(again.members_written + again.members_kept + again.movies_written + again.movies_kept, 0);
    f.assert_complete();
}

#[test]
fn a_v020_folder_is_repaired() {
    let f = Fixture::new("v020");
    f.run();
    let fresh = index_text(&f.out());
    f.as_v020();
    let out = f.out();
    let member = |a: &str, i: usize| out.join(format!("big/{a}/data/a_{i}.bin"));
    let mut zeroed = fs::read(member("cache0", 3)).unwrap();
    zeroed.fill(0);
    fs::write(member("cache0", 3), zeroed).unwrap();
    fs::write(member("cache1", 4), b"short").unwrap();
    fs::remove_file(member("cache4", 6)).unwrap();
    let mut large = fs::read(out.join("big/data0/data/large.bin")).unwrap();
    large[(1 << 20) + 5] ^= 1;
    fs::write(out.join("big/data0/data/large.bin"), large).unwrap();
    fs::write(out.join("big/fecache/data/sub/b.bin"), b"fecache: decoded membeR").unwrap();
    #[cfg(unix)]
    let outside = {
        let outside = f.dir.0.join("outside.bin");
        fs::write(&outside, b"not the game's").unwrap();
        fs::remove_file(member("cache5", 7)).unwrap();
        std::os::unix::fs::symlink(&outside, member("cache5", 7)).unwrap();
        outside
    };
    let s = f.run();
    assert!(s.already_done.is_empty(), "no rows pinned: every archive is read again");
    let replaced = if cfg!(unix) { 5 } else { 4 };
    assert_eq!(s.members_replaced, replaced, "zeroed, truncated, one bit, other decoded bytes, a link");
    assert_eq!(s.members_written, 1, "the removed one");
    assert_eq!(s.members_kept, f.files.len() - 2 - replaced - 1);
    assert_eq!((s.movies_kept, s.movies_written, s.movies_replaced), (2, 0, 0));
    f.assert_complete();
    assert_eq!(index_text(&out), fresh, "the repaired index is the fresh setup's");
    #[cfg(unix)]
    assert_eq!(fs::read(outside).unwrap(), b"not the game's", "a link is replaced, never written through");
}

#[test]
fn a_format_1_folder_gets_its_movies_and_index() {
    let f = Fixture::new("format1");
    f.run();
    let fresh = index_text(&f.out());
    let mut m = f.marker();
    let o = m.as_object_mut().unwrap();
    o.remove("content_index");
    o.remove("movies");
    o.insert("format".into(), 1.into());
    for a in ARCHIVES {
        m["finished"][a].as_object_mut().unwrap().remove("rows");
    }
    f.set_marker(&m);
    fs::remove_file(f.out().join(INDEX_FILE)).unwrap();
    fs::remove_dir_all(f.out().join("disc")).unwrap();
    assert_eq!(folder::state(&f.out()), State::NoMovies);
    let s = f.run();
    assert_eq!((s.movies_written, s.members_kept), (2, f.files.len() - 2));
    f.assert_complete();
    assert_eq!(index_text(&f.out()), fresh);
}

#[test]
fn a_damaged_index_reads_only_the_parts_it_lost() {
    let f = Fixture::new("damaged");
    f.run();
    let fresh = index_text(&f.out());
    let row = format!("big/cache6/data/a_8.bin\t{}\t", 100 + 8 * 7);
    let hash = ContentHash::of(&f.files["big/cache6/data/a_8.bin"]).to_string();
    fs::write(f.out().join(INDEX_FILE), fresh.replace(&format!("{row}{hash}"), &format!("{row}{}", "0".repeat(64))))
        .unwrap();
    assert!(matches!(folder::state(&f.out()), State::IndexDamaged(_)));
    let s = f.run();
    assert_eq!(s.extracted, ["cache6"]);
    assert_eq!(s.already_done.len(), ARCHIVES.len() - 1);
    assert_eq!((s.members_kept, s.members_written, s.movies_kept), (2, 0, 0), "cache6 compared, the movies taken");
    f.assert_complete();
    assert_eq!(index_text(&f.out()), fresh);
    fs::write(f.out().join(INDEX_FILE), &fresh[..fresh.len() / 2]).unwrap();
    let s = f.run();
    assert!(s.already_done.is_empty());
    assert_eq!((s.members_kept, s.members_written, s.members_replaced), (f.files.len() - 2, 0, 0));
    assert_eq!(index_text(&f.out()), fresh);
}

#[test]
fn a_complete_folder_with_files_gone_reads_only_those_parts_again() {
    let f = Fixture::new("files-gone");
    f.run();
    let fresh = index_text(&f.out());
    let out = f.out();
    fs::remove_file(out.join("big/cache4/data/a_6.bin")).unwrap();
    let large = out.join("big/data0/data/large.bin");
    let cut = fs::read(&large).unwrap()[..1000].to_vec();
    fs::write(&large, cut).unwrap();
    fs::remove_file(out.join("disc/data/common/fmv/alps.vp6")).unwrap();
    let state = folder::state(&out);
    assert!(matches!(&state, State::FilesDamaged(w) if w.starts_with("3 files: 2 missing, 1 of another size")));
    assert!(!state.playable() && state.needs_repair());
    let s = f.run();
    assert_eq!(s.extracted, ["data0", "cache4"], "the two archives with a damaged file");
    assert_eq!(s.already_done.len(), ARCHIVES.len() - 2);
    assert_eq!((s.members_written, s.members_replaced, s.members_kept), (1, 1, 3), "data0's 3 and cache4's 2 files");
    assert_eq!((s.movies_written, s.movies_kept, s.movies_replaced), (1, 1, 0));
    f.assert_complete();
    assert_eq!(index_text(&out), fresh, "the repaired index is the fresh setup's");
}

#[test]
fn a_stopped_setup_resumes_with_the_parts_it_finished() {
    let f = Fixture::new("resume");
    f.run_cancelled_at(3);
    assert_eq!(folder::state(&f.out()), State::Unfinished);
    assert!(f.out().join(ROWS_FILE).is_file());
    let s = f.run();
    assert_eq!(s.already_done, ARCHIVES[..3]);
    assert_eq!(s.extracted, ARCHIVES[3..]);
    f.assert_complete();
    let fresh = Fixture::new("resume-fresh");
    fresh.run();
    assert_eq!(index_text(&f.out()), index_text(&fresh.out()));
}

#[test]
fn a_changed_rows_file_is_not_trusted() {
    let f = Fixture::new("rows");
    f.run_cancelled_at(2);
    let rows = fs::read_to_string(f.out().join(ROWS_FILE)).unwrap();
    let hash = ContentHash::of(&f.files["big/data0/data/a_0.bin"]).to_string();
    assert!(rows.contains(&hash));
    fs::write(f.out().join(ROWS_FILE), rows.replace(&hash, &"1".repeat(64))).unwrap();
    let s = f.run();
    assert_eq!(s.already_done, ["bootcache"]);
    assert_eq!(s.extracted[0], "data0");
    f.assert_complete();
}

#[test]
fn a_stopped_repair_keeps_the_folder_playable() {
    let f = Fixture::new("stopped-repair");
    f.run();
    f.as_v020();
    f.run_cancelled_at(5);
    let state = folder::state(&f.out());
    assert_eq!(state, State::NoIndex);
    assert!(state.playable());
    let s = f.run();
    assert_eq!(s.already_done, ARCHIVES[..5]);
    f.assert_complete();
}

#[cfg(unix)]
#[test]
fn a_linked_folder_is_refused() {
    let f = Fixture::new("linked");
    f.run();
    let elsewhere = f.dir.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_dir_all(f.out().join("big/cache7")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.out().join("big/cache7")).unwrap();
    let err = setup::run(&f.iso, &f.out(), &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("a link or a file where a folder of the setup belongs"), "{err:#}");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "nothing written through the link");
}

#[test]
fn another_disc_is_refused() {
    let f = Fixture::new("other");
    let mut image = fs::read(&f.iso).unwrap();
    let at = image.windows(10).position(|w| w == b"ssxvzf.exe").unwrap();
    image[at..at + 10].copy_from_slice(b"other0.exe");
    fs::write(&f.iso, image).unwrap();
    let err = setup::run(&f.iso, &f.out(), &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("this is not SSX (2012)"), "{err:#}");
    assert!(!f.out().exists());
}

#[test]
fn the_index_is_pinned_by_the_marker() {
    let f = Fixture::new("pinned");
    f.run();
    let m = f.marker();
    let text = index_text(&f.out());
    assert_eq!(m["content_index"]["blake3"], ContentHash::of(text.as_bytes()).to_string());
    assert_eq!(m["content_index"]["files"], f.files.len());
    for a in ARCHIVES {
        assert!(m["finished"][a]["rows"].is_string(), "{a}");
    }
    assert!(m["movies"]["rows"].is_string());
    let _: &Path = &f.out();
}
