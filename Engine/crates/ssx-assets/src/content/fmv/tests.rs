use std::path::{Path, PathBuf};
use std::sync::Arc;

use ssx_content::movie::write::*;
use ssx_content::{Layer, PackageSource, Registry};

use super::Fmv;
use super::port::fe_background_id;
use crate::content::GameContent;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssx-fmv-slot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn store(name: &str) -> Arc<GameContent> {
    GameContent::of(&folder(&format!("{name}-absent")))
}

fn webm() -> Vec<u8> {
    let tracks = element(0x1654_AE6B, &video_track(1, 320, 180, &av1_private(BASE_SEQ), Some(33_333_333)));
    file(&[info(1000.0), tracks, cluster(0, &[simple_block(1, 0, 0x80, &unit()), simple_block(1, 34, 0, &unit())])])
}

fn package(dir: &Path, name: &str, section: &str, key: &str) -> PackageSource {
    std::fs::create_dir_all(dir.join("movies")).unwrap();
    std::fs::write(dir.join("movies/loop.webm"), webm()).unwrap();
    let manifest = format!(
        "[resource]\nname = \"{name}\"\nversion = \"1.0.0\"\napi = 1\n[{section}]\n\"{key}\" = \"movies/loop.webm\"\n"
    );
    std::fs::write(dir.join("resource.toml"), manifest).unwrap();
    PackageSource::open(dir).unwrap()
}

fn mount(content: &GameContent, packages: Vec<(i32, PackageSource)>) -> ssx_content::Report {
    let mut r = content.registry.write().unwrap();
    for (priority, p) in packages {
        r.mount(Layer::ClientMods, priority, Box::new(p));
    }
    assert!(content.mount_fmv(&mut r), "a package provides a movie: the disc's movies and the port's slots mounted");
    r.report()
}

fn source(fmv: Option<Fmv>) -> Option<String> {
    match fmv {
        Some(Fmv::Package { source, layer, .. }) => {
            assert_eq!(layer, Layer::ClientMods);
            Some(source)
        }
        Some(Fmv::Disc(path)) => panic!("a disc movie in the slot: {}", path.display()),
        None => None,
    }
}

#[test]
fn the_slot_is_empty_by_default() {
    let slot = fe_background_id();
    let content = store("empty");
    assert_eq!(content.fmv(&slot), None);
    assert!(!content.registry.read().unwrap().has(&slot), "nothing of the movies mounted without a package");
    let dir = folder("empty");
    let report = mount(&content, vec![(0, package(&dir, "other", "add", "movie/intro"))]);
    assert!(content.registry.read().unwrap().has(&slot));
    assert_eq!(content.fmv(&slot), None);
    assert!(report.overrides.iter().all(|o| o.id != slot));
    assert!(report.conflicts.iter().all(|c| c.id != slot));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_package_fills_the_slot_and_the_registry_decides_between_two() {
    let slot = fe_background_id();
    let dir = folder("fill");
    let content = store("fill");
    let report = mount(&content, vec![(0, package(&dir.join("a"), "alpha", "override", slot.as_str()))]);
    assert_eq!(source(content.fmv(&slot)).as_deref(), Some("alpha"));
    let o = report.overrides.iter().find(|o| o.id == slot).expect("the slot's override reported");
    assert_eq!(o.winner, (Layer::ClientMods, "alpha".to_string()));
    assert_eq!(o.over, [(Layer::Port, "ava".to_string())]);
    assert!(report.conflicts.is_empty());

    let content = store("two");
    let report = mount(
        &content,
        vec![
            (0, package(&dir.join("a"), "alpha", "override", slot.as_str())),
            (5, package(&dir.join("b"), "beta", "override", slot.as_str())),
        ],
    );
    assert_eq!(source(content.fmv(&slot)).as_deref(), Some("beta"));
    let c = report.conflicts.iter().find(|c| c.id == slot).expect("the conflict reported");
    assert_eq!((c.layer, c.winner.as_str(), &c.losers[..]), (Layer::ClientMods, "beta", &["alpha".to_string()][..]));
    content.refuse_fmv(&slot, "beta", "a damaged stream (test)");
    assert_eq!(source(content.fmv(&slot)).as_deref(), Some("alpha"), "the next source");
    content.refuse_fmv(&slot, "alpha", "a damaged stream (test)");
    assert_eq!(content.fmv(&slot), None, "every package passed over: the empty slot");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_ports_source_holds_the_empty_slot() {
    use ssx_content::{Asset, Kind, Source};
    let slot = fe_background_id();
    assert_eq!(slot.as_str(), "ava:movie/fe_background");
    let mut r = Registry::new();
    r.mount(Layer::Port, 0, Box::new(super::port::PortMovies));
    assert!(r.has(&slot));
    assert_eq!(super::port::PortMovies.list(Kind::Movie), std::slice::from_ref(&slot));
    assert!(super::port::PortMovies.list(Kind::Texture).is_empty());
    let loaded = super::port::PortMovies.load(&slot).unwrap();
    assert!(matches!(loaded, Asset::Value(v) if v.downcast_ref::<super::port::EmptySlot>().is_some()));
    assert!(!r.has(&"ava:movie/other".parse().unwrap()));
    assert!(super::port::slot_note(&slot).is_some_and(|n| n.contains("fe_background")));
    assert!(super::port::slot_note(&"ssx:movie/boot_movie_eng".parse().unwrap()).is_none());
}
