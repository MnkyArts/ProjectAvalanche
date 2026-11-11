use std::collections::HashMap;
use std::sync::Arc;

use ssx_formats::gfx::place::PlaceObject;
use ssx_formats::gfx::shape::{DefineShape, FillStyle, Shape, ShapeRecord, Styles};
use ssx_formats::gfx::types::{Matrix, Rect, Rgba};

use super::*;
use crate::geom::RectF;
use crate::library::{FrameDef, MovieDef};

pub(crate) fn po(depth: u16, ch: Option<u16>, mv: bool, tx: i32) -> PlaceObject {
    PlaceObject {
        version: 2,
        flags: u8::from(mv) | if ch.is_some() { 2 } else { 0 } | 4,
        flags3: 0,
        depth,
        class_name: None,
        character_id: ch,
        matrix: Some(Matrix { tx, ..Matrix::IDENTITY }),
        color_transform: None,
        ratio: None,
        name: None,
        clip_depth: None,
        filters: Vec::new(),
        blend_mode: None,
        cache_as_bitmap: None,
        visible: None,
        background: None,
        clip_actions: None,
    }
}

pub(crate) fn rect_shape(id: u16, w: i32, h: i32) -> DefineShape {
    let records = vec![
        ShapeRecord::StyleChange { move_to: Some((0, 0)), fill0: None, fill1: Some(1), line: None, new_styles: None },
        ShapeRecord::Straight { dx: w, dy: 0 },
        ShapeRecord::Straight { dx: 0, dy: h },
        ShapeRecord::Straight { dx: -w, dy: 0 },
        ShapeRecord::Straight { dx: 0, dy: -h },
    ];
    DefineShape {
        version: 3,
        id,
        bounds: Rect { x_min: 0, x_max: w, y_min: 0, y_max: h },
        edge_bounds: None,
        unk_reserved: 0,
        uses_fill_winding_rule: false,
        uses_non_scaling_strokes: false,
        uses_scaling_strokes: false,
        shape: Shape {
            styles: Styles { fills: vec![FillStyle::Solid(Rgba { r: 255, g: 0, b: 0, a: 255 })], lines: Vec::new() },
            records,
        },
    }
}

fn frames(tags: Vec<Vec<FrameTag>>) -> Vec<FrameDef> {
    tags.into_iter().map(|t| FrameDef { tags: t, ..Default::default() }).collect()
}

fn place(p: PlaceObject) -> FrameTag {
    FrameTag::Place(Arc::new(p))
}

pub(crate) fn movie(lib: &mut Library, root: Vec<Vec<FrameTag>>) -> MovieId {
    let id = lib.movies().len();
    let mut chars = HashMap::new();
    chars.insert(1, Character::Shape(Arc::new(rect_shape(1, 200, 200))));
    chars.insert(2, Character::Shape(Arc::new(rect_shape(2, 100, 100))));
    let sprite = frames(vec![
        vec![place(po(1, Some(2), false, 0))],
        vec![place(po(1, None, true, 20))],
        vec![place(po(1, None, true, 40))],
    ]);
    let mut sprite_frames = sprite;
    sprite_frames[0].labels.push("Intro".into());
    sprite_frames[2].labels.push("Out".into());
    chars.insert(10, Character::Sprite(Arc::new(TimelineDef { movie: id, id: 10, frames: sprite_frames })));
    let mut exports = HashMap::new();
    exports.insert("Clip".to_string(), 10);
    lib.insert_movie(MovieDef {
        path: format!("test{id}.gfx"),
        dir: String::new(),
        version: 8,
        frame_rate: 30.0,
        stage: RectF::new(0.0, 0.0, 25600.0, 14400.0),
        background: None,
        swf_name: "Test".into(),
        root: Arc::new(TimelineDef { movie: id, id: 0, frames: frames(root) }),
        chars,
        exports,
        imports: HashMap::new(),
        scaling_grids: HashMap::new(),
    })
}

pub(crate) fn empty_lib() -> Library {
    struct NoFiles;
    impl crate::fs::FileSource for NoFiles {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn exists(&self, _: &str) -> bool {
            false
        }
        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn image(&self, path: &str) -> anyhow::Result<crate::image::ImageData> {
            anyhow::bail!("{path}: no files")
        }
    }
    Library::new(Box::new(NoFiles))
}

fn depth(d: u16) -> i32 {
    i32::from(d) + TIMELINE_DEPTH_OFFSET
}

#[test]
fn a_rewind_gives_the_kept_instance_its_placement() {
    use ssx_formats::gfx::types::ColorTransform;
    let mut lib = empty_lib();
    let mut tint = po(1, None, true, 300);
    tint.color_transform = Some(ColorTransform { mult: [0, 0, 0, 256], add: [153, 0, 0, 0] });
    let m = movie(&mut lib, vec![vec![place(po(1, Some(1), false, 0))], vec![], vec![place(tint)]]);
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    let c = p.child_at_depth(root, depth(1)).unwrap();
    p.goto_frame(&lib, root, 3, false);
    assert_ne!(p.get(c).unwrap().cxform, Cxform::IDENTITY);
    assert_eq!(p.get(c).unwrap().matrix.tx, 300.0);
    p.goto_frame(&lib, root, 1, false);
    assert_eq!(p.child_at_depth(root, depth(1)), Some(c), "the instance is kept");
    assert_eq!(p.get(c).unwrap().cxform, Cxform::IDENTITY);
    assert_eq!(p.get(c).unwrap().matrix.tx, 0.0);
    p.goto_frame(&lib, root, 3, false);
    p.set_cxform(c, Cxform { mult: [1.0, 1.0, 1.0, 0.5], add: [0.0; 4] });
    p.goto_frame(&lib, root, 1, false);
    assert_eq!(p.get(c).unwrap().cxform.mult[3], 0.5);
}

#[test]
fn place_move_remove_and_loop() {
    let mut lib = empty_lib();
    let m = movie(
        &mut lib,
        vec![
            vec![place(po(1, Some(1), false, 0))],
            vec![place(po(1, None, true, 100)), place(po(2, Some(2), false, 0))],
            vec![FrameTag::Remove(2)],
        ],
    );
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    let a = p.child_at_depth(root, depth(1)).expect("frame 1 placed depth 1");
    assert_eq!(p.frames(root), Some((1, 3)));
    p.advance_frame(&lib);
    assert_eq!(p.get(a).unwrap().matrix.tx, 100.0, "the move keeps the instance");
    assert!(p.child_at_depth(root, depth(2)).is_some());
    p.advance_frame(&lib);
    assert!(p.child_at_depth(root, depth(2)).is_none(), "removed on frame 3");
    p.advance_frame(&lib);
    assert_eq!(p.frames(root), Some((1, 3)));
    assert_eq!(p.child_at_depth(root, depth(1)), Some(a));
    assert_eq!(p.get(a).unwrap().matrix.tx, 0.0);
}

#[test]
fn forward_goto_skips_transient_objects_and_rewind_drops_later_ones() {
    let mut lib = empty_lib();
    let m = movie(
        &mut lib,
        vec![
            vec![place(po(1, Some(1), false, 0))],
            vec![place(po(5, Some(10), false, 0))],
            vec![FrameTag::Remove(5), place(po(1, None, true, 30))],
            vec![place(po(6, Some(2), false, 7))],
        ],
    );
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    let a = p.child_at_depth(root, depth(1)).unwrap();
    let before = p.instance_count();
    p.goto_frame(&lib, root, 4, false);
    assert_eq!(p.instance_count(), before + 1);
    assert_eq!(p.get(a).unwrap().matrix.tx, 30.0);
    let b = p.child_at_depth(root, depth(6)).unwrap();
    assert_eq!(p.get(b).unwrap().placed_frame, Some(4));
    p.goto_frame(&lib, root, 2, false);
    assert!(p.get(b).is_none());
    assert_eq!(p.child_at_depth(root, depth(1)), Some(a));
    assert_eq!(p.get(a).unwrap().matrix.tx, 0.0);
    let s = p.child_at_depth(root, depth(5)).unwrap();
    assert_eq!(p.frames(s), Some((1, 3)), "a placed sprite starts at its frame 1");
    assert_eq!(p.frames(root), Some((2, 4)));
}

#[test]
fn nested_sprites_play_on_their_own_and_labels_resolve() {
    let mut lib = empty_lib();
    let m = movie(&mut lib, vec![vec![], vec![place(po(3, Some(10), false, 0))], vec![]]);
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    p.advance_frame(&lib);
    let s = p.child_at_depth(root, depth(3)).unwrap();
    assert_eq!(p.frames(s), Some((1, 3)), "not advanced in the tick that made it");
    p.advance_frame(&lib);
    assert_eq!(p.frames(s), Some((2, 3)));
    let inner = p.child_at_depth(s, depth(1)).unwrap();
    assert_eq!(p.get(inner).unwrap().matrix.tx, 20.0);
    assert!(p.goto_label(&lib, s, "out", false));
    assert_eq!(p.get(inner).unwrap().matrix.tx, 40.0);
    assert!(!p.goto_label(&lib, s, "missing", false));
    p.stop(root);
    p.advance_frame(&lib);
    assert_eq!(p.frames(root), Some((3, 3)), "the root stopped, its child sprite plays on");
    assert_eq!(p.frames(s), Some((3, 3)), "stopped by gotoAndStop");
    p.play(s);
    p.advance_frame(&lib);
    assert_eq!(p.frames(s), Some((1, 3)), "looped");
}

#[test]
fn script_api_attach_duplicate_swap() {
    let mut lib = empty_lib();
    let m = movie(&mut lib, vec![vec![place(po(1, Some(1), false, 0))]]);
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    assert_eq!(p.next_highest_depth(root), 0);
    let c = p.attach_movie(&lib, root, "Clip", "mcClip", 5).unwrap();
    assert_eq!(p.child_by_name(root, "mcClip", true), Some(c));
    assert_eq!(p.path(c), "_level0.mcClip");
    assert_eq!(p.next_highest_depth(root), 6);
    let d = p.duplicate_movie_clip(&lib, c, "mcCopy", 7).unwrap();
    assert_eq!(p.get(d).unwrap().character, p.get(c).unwrap().character);
    p.swap_depths(c, 7);
    assert_eq!(p.child_at_depth(root, 7), Some(c));
    assert_eq!(p.child_at_depth(root, 5), Some(d));
    let e = p.create_empty_movie_clip(root, "holder", 20).unwrap();
    assert!(p.children(e).is_empty());
    p.remove(c);
    assert!(p.get(c).is_none() && p.child_at_depth(root, 7).is_none());
    let f = p.attach_movie(&lib, root, "Clip", "again", 8).unwrap();
    assert!(p.get(c).is_none() && p.get(f).is_some());
}

#[test]
fn advance_catches_up_at_most_two_frames() {
    let mut lib = empty_lib();
    let m = movie(&mut lib, vec![vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]]);
    let mut p = Player::new();
    let root = p.load_level(&lib, m, 0);
    assert_eq!(p.tick(&lib, 0.5 / 30.0), 0);
    assert_eq!(p.tick(&lib, 4.6 / 30.0), 3);
    assert_eq!(p.frames(root), Some((4, 8)));
    assert_eq!(p.tick(&lib, 0.0), 2);
    assert_eq!(p.frames(root), Some((6, 8)));
}

#[test]
fn no_invisible_advance_freezes_the_hidden_clip_but_not_its_children() {
    let mut lib = empty_lib();
    let m = movie(&mut lib, vec![vec![]]);
    let mut p = Player::new();
    p.script_events = true;
    p.no_invisible_advance = true;
    let root = p.load_level(&lib, m, 0);
    let buffer = p.attach_movie(&lib, root, "Clip", "mcBuffer", 1).unwrap();
    let loader = p.attach_movie(&lib, buffer, "Clip", "mcImageLoader", 2).unwrap();
    for id in [buffer, loader] {
        p.get_mut(id).unwrap().scripted = true;
    }
    p.get_mut(buffer).unwrap().visible = false;
    p.advance_frame(&lib);
    p.take_actions();
    p.advance_frame(&lib);
    let entered: Vec<InstanceId> = p
        .take_actions()
        .into_iter()
        .filter_map(|a| match a {
            QueuedAction::EnterFrame { instance } => Some(instance),
            _ => None,
        })
        .collect();
    assert_eq!(entered, vec![loader], "the hidden clip's child still gets onEnterFrame");
    assert_eq!(p.frames(buffer), Some((1, 3)), "the hidden clip's timeline holds");
    assert_eq!(p.frames(loader), Some((3, 3)), "its visible child plays on");
}
