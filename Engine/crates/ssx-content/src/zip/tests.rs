use std::io::Cursor;

use super::testkit::*;
use super::*;

fn open(bytes: Vec<u8>) -> Result<ZipArchive<Cursor<Vec<u8>>>, ZipError> {
    ZipArchive::open(Cursor::new(bytes), &ZipLimits::default())
}

fn refused_with(bytes: Vec<u8>, want: &str) {
    let e = match open(bytes) {
        Err(e) => e.to_string(),
        Ok(mut z) => {
            let mut err = None;
            for i in 0..z.entries().len() {
                if let Err(e) = z.extract(i, &mut std::io::sink()) {
                    err = Some(e.to_string());
                    break;
                }
            }
            err.unwrap_or_else(|| panic!("accepted; expected {want:?}"))
        }
    };
    assert!(e.contains(want), "{e:?} lacks {want:?}");
}

fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let it = files.iter().map(|(n, b)| Ok(PackFile { name: n.to_string(), bytes: b.to_vec() }));
    write_zip(&mut out, it, &ZipLimits::default()).unwrap();
    out
}

#[test]
fn the_writer_is_deterministic_and_its_archives_read_back() {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let noise: Vec<u8> = (0..70_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    let zeros = vec![0u8; 3 << 20];
    let text = "[resource]\nname = \"a\"\n".repeat(20);
    let files: [(&str, &[u8]); 4] =
        [("resource.toml", text.as_bytes()), ("t/noise.png", &noise), ("t/zero.dds", &zeros), ("z", b"")];
    let a = pack(&files);
    assert_eq!(a, pack(&files), "byte-identical on a second run");
    let mut z = open(a).unwrap();
    let methods: Vec<Method> = z.entries().iter().map(|e| e.method).collect();
    assert_eq!(methods, [Method::Deflated, Method::Stored, Method::Stored, Method::Stored]);
    for (i, (_, bytes)) in files.iter().enumerate() {
        assert_eq!(z.read(i, u64::MAX).unwrap(), *bytes);
    }
    let unsorted = [("b", &b"1"[..]), ("a", b"2")].map(|(n, b)| Ok(PackFile { name: n.into(), bytes: b.to_vec() }));
    assert!(write_zip(Vec::new(), unsorted, &ZipLimits::default()).is_err());
}

#[test]
fn archives_of_other_tools_are_read() {
    let mut a = deflated("textures/a.png", b"aaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    a.flags = FLAG_DESCRIPTOR;
    a.after = [DESCRIPTOR_SIG, a.crc, a.data.len() as u32, a.size].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut b = stored("textures/b.png", b"bbbb");
    b.flags = FLAG_DESCRIPTOR;
    b.after = [b.crc, 4, 4].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut dir = stored("textures/", b"");
    (dir.made_by, dir.attr) = (0x000A, 0x10);
    let mut z = open(assemble(b"", &[dir, a, b], b"a comment")).unwrap();
    assert_eq!(z.entries()[0].name, "textures");
    assert!(z.entries()[0].folder);
    assert_eq!(z.read(2, 4).unwrap(), b"bbbb");
}

#[test]
fn names_that_leave_the_package_are_refused() {
    for (name, why) in [
        ("../evil.toml", "\"..\" segment"),
        ("a/../../evil.toml", "\"..\" segment"),
        ("/etc/passwd", "absolute"),
        ("C:/evil.toml", "contains ':'"),
        ("C:\\evil.toml", "contains ':'"),
        ("\\\\server\\share\\x", "contains '\\\\'"),
        ("a//b", "\"\" segment"),
        ("./a", "\".\" segment"),
        ("con.png", "reserved device name"),
        ("x/aux/a.png", "reserved device name"),
        ("a.", "ending in '.'"),
        ("a b.png", "contains ' '"),
        ("é.png", "contains 'é'"),
        ("a\u{1}.png", "contains"),
    ] {
        refused_with(assemble(b"", &[stored(name, b"x")], b""), why);
    }
    let deep = vec!["d"; 40].join("/");
    refused_with(assemble(b"", &[stored(&deep, b"x")], b""), "nested deeper");
    let mut latin1 = stored("a.png", b"x");
    latin1.name = b"\xE9.png".to_vec();
    refused_with(assemble(b"", &[latin1], b""), "not UTF-8");
}

#[test]
fn links_and_special_files_are_refused() {
    let mut link = stored("a.png", b"../../etc/passwd");
    link.attr = 0o120_777 << 16;
    refused_with(assemble(b"", &[link.clone()], b""), "symbolic link");
    link.made_by = 0x1314;
    refused_with(assemble(b"", &[link], b""), "symbolic link");
    let mut fifo = stored("a.png", b"");
    fifo.attr = 0o010_644 << 16;
    refused_with(assemble(b"", &[fifo], b""), "special file");
    let mut reparse = stored("a.png", b"x");
    (reparse.made_by, reparse.attr) = (0x000A, 0x400);
    refused_with(assemble(b"", &[reparse], b""), "reparse point");
    let mut file_as_dir = stored("a.png", b"x");
    file_as_dir.attr = 0o040_755 << 16;
    refused_with(assemble(b"", &[file_as_dir], b""), "does not end in '/'");
}

#[test]
fn duplicate_and_case_colliding_names_are_refused() {
    let two = |a: &str, b: &str| assemble(b"", &[stored(a, b"1"), stored(b, b"2")], b"");
    refused_with(two("a.png", "a.png"), "named twice");
    refused_with(two("A.png", "a.png"), "differ only in case");
    refused_with(two("Tex/a.png", "tex/b.png"), "differ only in case");
    refused_with(two("a", "a/b.png"), "is a file and a folder");
}

#[test]
fn sizes_counts_and_ratios_are_capped() {
    let small = ZipLimits { max_entries: 2, max_file_bytes: 10, max_total_bytes: 15, ..ZipLimits::default() };
    let three = assemble(b"", &[stored("a", b"1"), stored("b", b"2"), stored("c", b"3")], b"");
    assert!(ZipArchive::open(Cursor::new(three), &small).unwrap_err().to_string().contains("3 entries"));
    let big = assemble(b"", &[stored("a", &[1; 11])], b"");
    assert!(ZipArchive::open(Cursor::new(big), &small).unwrap_err().to_string().contains("a file is at most 10"));
    let total = assemble(b"", &[stored("a", &[1; 8]), stored("b", &[1; 8])], b"");
    assert!(ZipArchive::open(Cursor::new(total), &small).unwrap_err().to_string().contains("in all"));
    refused_with(assemble(b"", &[deflated("bomb.dds", &vec![0; 2 << 20])], b""), "decompression bomb");
    open(assemble(b"", &[deflated("small.toml", &[b' '; 1 << 20])], b"")).unwrap();
    let mut lie = deflated("a.toml", &[b'x'; 5000]);
    lie.size = 100;
    refused_with(assemble(b"", &[lie.clone()], b""), "expands past its stated 100 bytes");
    lie.size = 6000;
    refused_with(assemble(b"", &[lie], b""), "5000 bytes where 6000 were stated");
}

#[test]
fn overlapping_and_hidden_bytes_are_refused() {
    let a = stored("a", b"aaaa");
    let mut b = stored("b", b"aaaa");
    (b.local, b.offset) = (false, Some(0));
    refused_with(assemble(b"", &[a.clone(), b], b""), "overlapping entries");
    refused_with(assemble(b"MZ\x90\x00 a self-extractor", std::slice::from_ref(&a), b""), "starts at byte");
    let mut hidden = a.clone();
    hidden.after = b"hidden".to_vec();
    refused_with(assemble(b"", &[hidden, stored("b", b"b")], b""), "hidden bytes");
    let mut names = a.clone();
    names.local_name = Some(b"evil".to_vec());
    refused_with(assemble(b"", &[names], b""), "the local header names");
    let mut desc = stored("a", b"aaaa");
    desc.flags = FLAG_DESCRIPTOR;
    desc.after = [DESCRIPTOR_SIG, 0, 4, 4].iter().flat_map(|v| v.to_le_bytes()).collect();
    refused_with(assemble(b"", &[desc], b""), "data descriptor differs");
}

#[test]
fn damaged_streams_are_refused() {
    let mut bad_crc = stored("a", b"aaaa");
    bad_crc.crc ^= 1;
    refused_with(assemble(b"", &[bad_crc], b""), "CRC-32");
    let full = deflated("a", &[b'q'; 3000]);
    let mut cut = full.clone();
    cut.data.truncate(cut.data.len() - 3);
    refused_with(assemble(b"", &[cut], b""), "cut short");
    let mut trailing = full.clone();
    trailing.data.extend(b"tail");
    refused_with(assemble(b"", &[trailing], b""), "after the end of its deflate stream");
    let mut garbage = full;
    garbage.data = vec![0xFF; 40];
    refused_with(assemble(b"", &[garbage], b""), "corrupt deflate stream");
    let mut stored_lie = stored("a", b"aaaa");
    stored_lie.size = 9;
    refused_with(assemble(b"", &[stored_lie], b""), "stored, but 4 bytes");
}

#[test]
fn foreign_features_are_refused_by_name() {
    let mut enc = stored("a", b"x");
    enc.flags = 1;
    refused_with(assemble(b"", &[enc], b""), "encrypted");
    let mut bz = stored("a", b"x");
    bz.method = 12;
    refused_with(assemble(b"", &[bz], b""), "BZIP2 (method 12)");
    let mut z64 = stored("a", b"x");
    z64.extra = [1u16, 8].iter().flat_map(|v| v.to_le_bytes()).chain([0; 8]).collect();
    refused_with(assemble(b"", &[z64], b""), "ZIP64");
    let mut odd = stored("a", b"x");
    odd.flags = 0x0020;
    refused_with(assemble(b"", &[odd], b""), "unknown general-purpose flags");
    let mut bad_extra = stored("a", b"x");
    bad_extra.extra = vec![0x55, 0x54, 9, 0];
    refused_with(assemble(b"", &[bad_extra], b""), "malformed extra field");
}

#[test]
fn broken_or_ambiguous_end_records_are_refused() {
    refused_with(b"PK".to_vec(), "too short");
    refused_with(vec![0x42; 4000], "no end of central directory");
    let good = assemble(b"", &[stored("a", b"x")], b"");
    refused_with(good[..good.len() - 1].to_vec(), "no end of central directory");
    let inner = assemble(b"", &[stored("b", b"y")], b"");
    let hidden = &inner[inner.len() - 22..];
    refused_with(assemble(b"", &[stored("a", b"x")], hidden), "several end of central directory records");
    refused_with(assemble(b"", &[], b""), "an empty archive");
    let mut locator = assemble(b"", &[stored("a", b"x")], b"");
    let end = locator.len() - 22;
    locator.splice(end..end, ZIP64_LOCATOR_SIG.to_le_bytes().into_iter().chain([0; 16]));
    refused_with(locator, "ZIP64");
}

#[test]
fn a_well_compressed_file_larger_than_one_output_block_reads_back() {
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8 / 64).collect();
    let bytes = pack(&[("models/kicker.glb", &big)]);
    assert!(bytes.len() < 64 << 10, "the case needs a stream within one input block: {}", bytes.len());
    let mut z = open(bytes).unwrap();
    assert_eq!(z.read(0, 1 << 20).unwrap(), big);
}

#[test]
fn a_player_dump_may_name_folders_with_inner_spaces() {
    let dump = || {
        assemble(
            b"",
            &[
                stored("DLC- 4541096D/0B4D5E7856AD993977D79B0FABAABB2F1A16C6B245", b"1"),
                stored("Title Update/tu", b"2"),
            ],
            b"",
        )
    };
    let limits = ZipLimits { names: super::ZipNames::PlayerDump, ..ZipLimits::default() };
    let mut a = ZipArchive::open(Cursor::new(dump()), &limits).unwrap();
    assert_eq!(a.entries()[0].name, "DLC- 4541096D/0B4D5E7856AD993977D79B0FABAABB2F1A16C6B245");
    assert_eq!(a.read(1, 16).unwrap(), b"2");
    refused_with(dump(), "contains ' '");
    for bad in [" a/b", "a /b", "a/ b", "con .txt", "con/x", "a/../b", "a\\b", "a: b"] {
        let one = assemble(b"", &[stored(bad, b"1")], b"");
        assert!(ZipArchive::open(Cursor::new(one), &limits).is_err(), "{bad:?}");
    }
}
