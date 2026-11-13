use std::path::{Path, PathBuf};

use ssx_assets::content::dlc::{DlcSet, set_active_roots};
use ssx_assets::vault_db::{with_all_db, with_attribdb};
use ssx_formats::attrib::fixtures::{CLASS, one_collection_pair, schema_pair};
use ssx_formats::attrib::fnv1;

fn write_pair(dir: &Path, stem: &str, (vlt, bin): (Vec<u8>, Vec<u8>)) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{stem}.vlt")), vlt).unwrap();
    std::fs::write(dir.join(format!("{stem}.bin")), bin).unwrap();
}

fn info(name: &str) -> Vec<u8> {
    let mut v = b"DLC_INFO".to_vec();
    v.push(2);
    let string = |v: &mut Vec<u8>, s: &str| {
        v.push(s.len() as u8);
        v.extend_from_slice(s.as_bytes());
        v.push(0);
    };
    string(&mut v, name);
    v.extend_from_slice(&[1, 0]);
    string(&mut v, "Track Content");
    v.push(2);
    for (k, val) in [("vaultname", name), ("bigname", name)] {
        string(&mut v, k);
        string(&mut v, val);
    }
    v.extend_from_slice(&[1, 0]);
    string(&mut v, "SSX Test");
    string(&mut v, "a test package");
    v
}

fn package(dlc_root: &Path, name: &str) -> PathBuf {
    let root = dlc_root.join(name);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("info.dlc"), info(name)).unwrap();
    let mut toc = b"DLC_TOC_".to_vec();
    toc.extend_from_slice(&1u32.to_be_bytes());
    toc.extend_from_slice(&0u32.to_be_bytes());
    std::fs::write(root.join("dlc.toc"), toc).unwrap();
    let data = root.join("data/xenon/attribdb");
    let decode = root.join("bootcache/data/xenon/attribdb");
    write_pair(&data, &format!("dlc_{name}"), one_collection_pair(&format!("{name}/row")));
    write_pair(&data, "ge_attribdb", one_collection_pair(&format!("{name}/ge")));
    write_pair(&decode, "ge_attribdb", one_collection_pair(&format!("{name}/ge")));
    write_pair(&decode, "attribdb_schema", schema_pair());
    root
}

#[test]
fn a_package_whose_vaults_do_not_decode_is_not_mounted() {
    let dir = std::env::temp_dir().join(format!("ssx-dlc-damaged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let big = dir.join("big");
    let disc = big.join("bootcache/data/xenon/attribdb");
    write_pair(&disc, "attribdb_schema", schema_pair());
    write_pair(&disc, "attribdb", one_collection_pair("disc/row"));
    write_pair(&disc, "ge_attribdb", one_collection_pair("disc/ge"));
    let dlc = dir.join("dlc");
    let (good, bad) = (package(&dlc, "dlc_good"), package(&dlc, "dlc_bad"));
    for copy in
        [bad.join("data/xenon/attribdb/ge_attribdb.vlt"), bad.join("bootcache/data/xenon/attribdb/ge_attribdb.vlt")]
    {
        let bytes = std::fs::read(&copy).unwrap();
        std::fs::write(&copy, &bytes[..bytes.len() / 2]).unwrap();
    }

    let set = DlcSet::load(&dlc);
    assert_eq!(set.packages.iter().map(|p| p.folder.as_str()).collect::<Vec<_>>(), ["dlc_bad", "dlc_good"]);
    let damaged = set.packages[0].damaged.as_deref().expect("the damaged package is marked");
    assert!(damaged.contains("ge_attribdb.vlt"), "{damaged}");
    assert!(set.packages[1].damaged.is_none());
    assert_eq!(set.roots(), vec![good.clone()], "only the good package is a root");
    assert_eq!(set.mounted().len(), 1);
    assert!(set.lines().iter().any(|l| l.to_string().contains("DAMAGED")), "the mount notes name it");

    set_active_roots(set.roots());
    let found = with_all_db(&big, |db| {
        Ok([
            ("disc/row", true),
            ("dlc_good/row", true),
            ("dlc_good/ge", true),
            ("disc/ge", false),
            ("dlc_bad/row", false),
        ]
        .map(|(name, want)| (db.find(CLASS, fnv1(name.as_bytes())).is_some(), want)))
    })
    .expect("the vaults decode without the damaged package");
    for (i, (got, want)) in found.into_iter().enumerate() {
        assert_eq!(got, want, "lookup {i}");
    }
    assert!(with_attribdb(&big, |db| Ok(db.find(CLASS, fnv1(b"dlc_good/ge")).is_some())).unwrap());

    set_active_roots(vec![bad, good]);
    assert!(with_all_db(&big, |_| Ok(())).is_err(), "the damaged package breaks the databases when mounted");
    set_active_roots(Vec::new());
    let _ = std::fs::remove_dir_all(&dir);
}
