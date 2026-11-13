use std::path::Path;

use super::*;

fn dir(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("ssx-dlc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn info(name: &str, version: Option<&str>) -> Vec<u8> {
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
    let mut pairs: Vec<(&str, &str)> = vec![("vaultname", name), ("bigname", name)];
    if let Some(version) = version {
        pairs.push(("mountversion", version));
    }
    v.push(pairs.len() as u8);
    for (k, val) in pairs {
        string(&mut v, k);
        string(&mut v, val);
    }
    v.push(1);
    v.push(0);
    string(&mut v, "SSX Test");
    string(&mut v, "a test package");
    v
}

fn toc() -> Vec<u8> {
    let mut v = b"DLC_TOC_".to_vec();
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v
}

fn package(root: &Path, folder: &str, name: &str, version: Option<&str>, content_version: u32) {
    let root = root.join(folder);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("info.dlc"), info(name, version)).unwrap();
    std::fs::write(root.join("dlc.toc"), toc()).unwrap();
    std::fs::write(
        root.join("dimecontentinfo.xml"),
        format!("<dime><name>x</name><contentVersion>{content_version}</contentVersion></dime>"),
    )
    .unwrap();
}

#[test]
fn the_list_is_strict() {
    let good = b"format = 1\n\n[[packages]]\nfolder = \"dlc_a\"\nenabled = true\n";
    let list = DlcList::parse(good, "dlc.toml").unwrap();
    assert_eq!(list.entries, [EnabledEntry { folder: "dlc_a".into(), enabled: true }]);
    let round = list.to_toml();
    assert_eq!(DlcList::parse(round.as_bytes(), "dlc.toml").unwrap(), list);
    for bad in [
        &b"format = 2\n"[..],
        b"format = 1\n[[packages]]\nfolder = \"../x\"\nenabled = true\n",
        b"format = 1\n[[packages]]\nfolder = \"a/b\"\nenabled = true\n",
        b"format = 1\n[[packages]]\nfolder = \"dlc_a\"\nenabled = true\n[[packages]]\nfolder = \"DLC_A\"\nenabled = false\n",
        b"format = 1\nunknown = 1\n",
    ] {
        assert!(DlcList::parse(bad, "dlc.toml").is_err(), "{}", String::from_utf8_lossy(bad));
    }
    assert!(DlcList::parse(&vec![b'x'; MAX_LIST_BYTES as usize + 1], "dlc.toml").is_err());
}

#[test]
fn the_lower_name_hash_mounts_first_and_duplicates_are_reported() {
    let root = dir("order");
    package(&root, "old", "dlc_same", Some("030"), 1);
    package(&root, "new", "dlc_same", Some("030"), 8);
    package(&root, "other", "dlc_other", Some("070"), 1);
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let set = DlcSet::load(&root);
    assert!(set.list_error.is_none(), "{:?}", set.list_error);
    let names: Vec<&str> = set.packages.iter().map(|p| p.folder.as_str()).collect();
    assert_eq!(
        names,
        ["other", "new"],
        "mount order: dlc_other's lower name hash first; the duplicate's newer copy only"
    );
    assert_eq!(set.packages[0].content_version, Some(1), "other");
    assert_eq!(set.packages[1].content_version, Some(8), "the newer duplicate");
    assert!(set.skipped.iter().any(|s| s.folder == "old" && s.reason.contains("newer")), "{:?}", set.skipped);
    assert_eq!(set.roots().len(), 2);
    let list = DlcList {
        entries: vec![
            EnabledEntry { folder: "new".into(), enabled: false },
            EnabledEntry { folder: "other".into(), enabled: true },
        ],
    };
    list.write(&root).unwrap();
    let set = DlcSet::load(&root);
    assert_eq!(set.roots(), [root.join("other")]);
    assert!(!set.packages[1].enabled);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_broken_list_mounts_no_listed_package() {
    let root = dir("broken-list");
    package(&root, "a", "dlc_a", None, 1);
    std::fs::write(root.join("dlc.toml"), b"format = 1\n[[packages]]\nfolder = \"../a\"\n").unwrap();
    let set = DlcSet::load(&root);
    assert!(set.list_error.is_some());
    assert!(set.packages.is_empty(), "{:?}", set.packages);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unlockables_are_split_trimmed_and_deduplicated() {
    let root = dir("unlock");
    package(&root, "one", "dlc_one", None, 1);
    package(&root, "two", "dlc_two", None, 2);
    let write = |folder: &str, name: &str, text: &str| {
        let d = root.join(folder).join("data/common/dlc");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(name), text).unwrap();
    };
    write("one", "unlockables_dlc_one.txt", "eddie, geotag_presell\n");
    write("one", "unlockables_shared.txt", "old_token");
    write("two", "unlockables_shared.txt", "new_token");
    write("two", "unlockables_dlc_two.txt", "eddie,travis");
    let set = DlcSet::load(&root);
    let u = set.unlockables();
    assert_eq!(u.tokens, ["eddie", "travis", "new_token", "geotag_presell"], "{:?}", u);
    assert!(u.problems.is_empty(), "{:?}", u.problems);
    assert_eq!(set.roots(), [root.join("two"), root.join("one")]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hostile_unlockables_are_refused_and_reported() {
    let root = dir("unlock-bad");
    package(&root, "one", "dlc_one", None, 1);
    let d = root.join("one/data/common/dlc");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("unlockables_a.txt"), "good,../escape,ok-token,ok.token,ok_token").unwrap();
    let u = unlockables_from_roots(&[root.join("one")]);
    assert_eq!(u.tokens, ["good", "ok-token", "ok.token", "ok_token"]);
    assert_eq!(u.problems.len(), 1, "{:?}", u.problems);
    assert!(u.problems[0].contains("../escape"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dime_manifest_names_its_version_and_id() {
    let (version, id) =
        dime(b"<dime><name>SSX</name><contentVersion>8</contentVersion><uniqueDlcId>0001001</uniqueDlcId></dime>");
    assert_eq!((version, id.as_deref()), (Some(8), Some("0001001")));
    assert_eq!(dime(b"<dime><contentVersion>x</contentVersion></dime>").0, None);
    assert_eq!(dime(b"").0, None);
}
