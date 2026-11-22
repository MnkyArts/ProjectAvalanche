use bevy::camera::visibility::VisibilitySystems;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use ssx_assets::lod::LOD_COUNT;

use super::material::SIMPLE_SET;
use crate::render::shadows::{CASCADES, CascadeFit, GameSun};

pub const LOD_FACTORS: [f32; LOD_COUNT] = [0.8, 0.4, 0.0];

pub const SIMPLE_DEPTH: f32 = 125.0;

pub const CUT_OFF: f32 = 600.0;

pub const STREAM_SLOTS: usize = 180;

pub const STREAM_DISTANCE: f32 = 600.0;

pub const PROXY_BAND: f32 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub slots: f32,
    pub stream: f32,
    pub far: Option<f32>,
    pub proxy_push: f32,
    pub proxy_clip: f32,
}

// PORT(choice): the view distance slider (metres; the console's is fixed at 600 m) stands for the 600 m of the cut-off
// ([0x8210FA7C]), the streaming ([0x8210CE04]) and the proxy's push-back (gProxyOverlapBlend's 5 = 600 / 120); nearer,
// the proxy's clip (480 m, 0x82112070) comes 120 m inside it; farther, the slots keep the 600 m cut-off and the far
// detail draws the rest out to it, the proxy clipped at 480 m as the original.
pub fn reach(meters: f32) -> Reach {
    let slots = meters.min(CUT_OFF);
    Reach {
        slots,
        stream: meters / CUT_OFF * STREAM_DISTANCE,
        far: (meters > CUT_OFF).then_some(meters),
        proxy_push: meters,
        proxy_clip: slots - PROXY_BAND,
    }
}

pub const VIEW_MIN_METRIC: f32 = 0.002;
pub const CASCADE_MIN_METRIC: f32 = 0.02;

pub fn stream_priority(box_centre: Vec3, half_diagonal: f32, position: Vec3) -> f32 {
    box_centre.distance(position) - half_diagonal
}

pub fn stream_selection(
    priorities: &[f32],
    records: &[usize],
    distance: f32,
    order: &mut Vec<(f32, usize)>,
    streamed: &mut Vec<bool>,
) {
    order.clear();
    order.extend(records.iter().filter_map(|&i| priorities.get(i).filter(|&&p| p < distance).map(|&p| (p, i))));
    game_sort(order);
    streamed.clear();
    streamed.resize(priorities.len(), false);
    for &(_, i) in order.iter().take(STREAM_SLOTS) {
        streamed[i] = true;
    }
}

pub fn game_sort(v: &mut [(f32, usize)]) {
    const LIMIT: usize = 28;
    let n = v.len();
    if n == 0 {
        return;
    }
    quick_sort_impl(v, 0, n, 2 * n.ilog2());
    if n > LIMIT {
        insertion_sort(&mut v[..LIMIT]);
        insertion_sort_unguarded(v, LIMIT);
    } else {
        insertion_sort(v);
    }

    fn quick_sort_impl(v: &mut [(f32, usize)], first: usize, mut last: usize, mut count: u32) {
        while last - first > LIMIT && count > 0 {
            let pivot = median(v[first], v[first + (last - first) / 2], v[last - 1]);
            let position = partition(v, first, last, pivot);
            count -= 1;
            quick_sort_impl(v, position, last, count);
            last = position;
        }
        if count == 0 {
            heap_sort(&mut v[first..last]);
        }
    }
}

fn less(a: (f32, usize), b: (f32, usize)) -> bool {
    a.0 < b.0
}

fn median(a: (f32, usize), b: (f32, usize), c: (f32, usize)) -> (f32, usize) {
    match (less(a, b), less(b, c), less(a, c)) {
        (true, true, _) => b,
        (true, false, true) => c,
        (true, false, false) => a,
        (false, _, true) => a,
        (false, true, false) => c,
        (false, false, false) => b,
    }
}

fn partition(v: &mut [(f32, usize)], mut first: usize, mut last: usize, pivot: (f32, usize)) -> usize {
    loop {
        while less(v[first], pivot) {
            first += 1;
        }
        last -= 1;
        while less(pivot, v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

fn insertion_sort(v: &mut [(f32, usize)]) {
    for sorted in 1..v.len() {
        let temp = v[sorted];
        let mut next = sorted;
        while next != 0 && less(temp, v[next - 1]) {
            v[next] = v[next - 1];
            next -= 1;
        }
        v[next] = temp;
    }
}

fn insertion_sort_unguarded(v: &mut [(f32, usize)], start: usize) {
    for current in start..v.len() {
        let temp = v[current];
        let mut end = current;
        while end > 0 && less(temp, v[end - 1]) {
            v[end] = v[end - 1];
            end -= 1;
        }
        v[end] = temp;
    }
}

fn heap_sort(v: &mut [(f32, usize)]) {
    let n = v.len();
    if n >= 2 {
        let mut parent = (n - 2) / 2 + 1;
        while parent != 0 {
            parent -= 1;
            adjust_heap(v, parent, n, parent, v[parent]);
        }
    }
    for last in (2..=n).rev() {
        let bottom = v[last - 1];
        v[last - 1] = v[0];
        adjust_heap(v, 0, last - 1, 0, bottom);
    }
}

fn adjust_heap(v: &mut [(f32, usize)], top: usize, size: usize, mut position: usize, value: (f32, usize)) {
    let mut child = 2 * position + 2;
    while child < size {
        if less(v[child], v[child - 1]) {
            child -= 1;
        }
        v[position] = v[child];
        position = child;
        child = 2 * child + 2;
    }
    if child == size {
        v[position] = v[child - 1];
        position = child - 1;
    }
    while position > top {
        let parent = (position - 1) / 2;
        if !less(v[parent], value) {
            break;
        }
        v[position] = v[parent];
        position = parent;
    }
    v[position] = value;
}

pub fn cut_off_depth(distance: f32, tan_min: f32) -> f32 {
    let tan_h = tan_min * crate::camera::GAME_ASPECT;
    distance * (1.0 + tan_h * tan_h).sqrt()
}

pub fn slot_state(metric: f32, near_depth: f32, cut_off: f32) -> Option<(usize, bool)> {
    if metric <= VIEW_MIN_METRIC || metric.is_nan() || near_depth >= cut_off {
        return None;
    }
    choose_lod(metric).map(|k| (k, near_depth > SIMPLE_DEPTH || near_depth.is_nan()))
}

// PORT(choice): the view distance slider above 600 m (the player's setting; the original draws the proxy there): the
// meshes the 180 slots and the 600 m cut-off leave out, at LOD 2 with the simple techniques, in view 0 only (the proxy
// they stand in for casts no shadow).
pub fn far_state(metric: f32, near_depth: f32, far_cut_off: f32) -> Option<(usize, bool)> {
    (metric > VIEW_MIN_METRIC && near_depth < far_cut_off).then_some((LOD_COUNT - 1, true))
}

pub fn choose_lod(metric: f32) -> Option<usize> {
    let m = if -metric >= 0.0 { 0.0 } else { metric };
    LOD_FACTORS.iter().position(|&f| f <= m)
}

pub fn metric_tan(camera: &Camera) -> Option<f32> {
    let clip = camera.clip_from_view();
    let (tan_x, tan_y) = (1.0 / clip.x_axis.x.abs(), 1.0 / clip.y_axis.y.abs());
    let narrower = tan_x < tan_y * crate::camera::GAME_ASPECT * (1.0 - 1e-4);
    let tan_min =
        if narrower { tan_x / crate::camera::GAME_ASPECT } else { 1.0 / clip.x_axis.x.abs().max(clip.y_axis.y.abs()) };
    (tan_min.is_finite() && tan_min > 0.0).then_some(tan_min)
}

pub fn size_metric(centre: Vec3, radius: f32, eye: Vec3, tan: f32) -> f32 {
    let distance = centre.distance(eye);
    if distance > 0.0 { radius / (distance * tan) } else { f32::INFINITY }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Containment {
    Inside,
    Intersecting,
    Outside,
}

#[derive(Clone, Copy, Debug)]
pub struct CascadeView {
    pub light: Mat3,
    pub min: Vec3,
    pub max: Vec3,
}

impl CascadeView {
    pub fn of_box(light: Mat3, min: Vec3, max: Vec3) -> Self {
        CascadeView { light, min: Vec3::new(min.x, min.y, -max.z), max: Vec3::new(max.x, max.y, -min.z) }
    }

    pub fn containment(&self, centre: Vec3, half: Vec3) -> Containment {
        let c = self.light * centre;
        let rows = [self.light.row(0), self.light.row(1), self.light.row(2)];
        let reach = Vec3::from_array(rows.map(|r| r.abs().dot(half)));
        let (from_min, to_max) = (c - self.min, self.max - c);
        let mut result = Containment::Inside;
        for (d, e) in from_min
            .to_array()
            .into_iter()
            .zip(reach.to_array())
            .chain(to_max.to_array().into_iter().zip(reach.to_array()))
        {
            if d < -e {
                return Containment::Outside;
            }
            if d < e {
                result = Containment::Intersecting;
            }
        }
        result
    }

    pub fn metric(&self, radius: f32) -> f32 {
        let size = self.max - self.min;
        radius / (0.5 * size.x.min(size.y))
    }

    pub fn near_depth(&self, centre: Vec3, radius: f32) -> f32 {
        -self.light.row(2).dot(centre) - radius
    }
}

pub fn cascade_slots(
    views: &[CascadeView; CASCADES],
    centre: Vec3,
    half: Vec3,
    radius: f32,
) -> [(Containment, f32, f32); CASCADES] {
    views.map(|v| {
        let metric = v.metric(radius);
        let containment = if metric > CASCADE_MIN_METRIC { v.containment(centre, half) } else { Containment::Outside };
        (containment, metric, v.near_depth(centre, radius))
    })
}

pub fn cascade_lods(slots: &[(Containment, f32, f32); CASCADES], cut_off: f32) -> [Option<usize>; CASCADES] {
    std::array::from_fn(|i| {
        let (containment, metric, near_depth) = slots[i];
        let inside_earlier = slots[..i].iter().any(|s| s.0 == Containment::Inside);
        if containment == Containment::Outside || inside_earlier || near_depth >= cut_off {
            return None;
        }
        choose_lod(metric)
    })
}

// PORT(choice): without a ride (the asset viewer's fly camera) the streaming follows the camera; the game always rides a
// level.
pub fn stream_position(start: Option<Vec3>, eye: Vec3) -> Vec3 {
    start.unwrap_or(eye)
}

pub(super) struct TerrainUnit {
    pub stream_centre: Vec3,
    pub stream_reach: f32,
    pub centre: Vec3,
    pub half: Vec3,
    pub radius: f32,
    pub record: (usize, usize),
    pub parts: [Vec<(usize, usize)>; LOD_COUNT],
    carriers: Option<[usize; SIMPLE_SET + 1]>,
    triangles: [usize; LOD_COUNT],
    main: Option<(usize, bool)>,
    cascades: [Option<usize>; CASCADES],
}

impl TerrainUnit {
    pub fn new(
        stream: (Vec3, f32),
        bounds: (Vec3, Vec3, f32),
        record: (usize, usize),
        parts: [Vec<(usize, usize)>; LOD_COUNT],
        carriers: Option<[usize; SIMPLE_SET + 1]>,
        triangles: [usize; LOD_COUNT],
    ) -> Self {
        let ((stream_centre, stream_reach), (centre, half, radius)) = (stream, bounds);
        TerrainUnit {
            stream_centre,
            stream_reach,
            centre,
            half,
            radius,
            record,
            parts,
            carriers,
            triangles,
            main: None,
            cascades: [None; CASCADES],
        }
    }

    fn lods_in(&self, view: SlotView) -> [bool; LOD_COUNT] {
        let mut lods = [false; LOD_COUNT];
        let mut mark = |lod: Option<usize>| {
            if let Some(l) = lod.and_then(|k| lods.get_mut(k)) {
                *l = true;
            }
        };
        match view {
            SlotView::Main => mark(self.main.map(|(k, _)| k)),
            SlotView::Cascade(i) => mark(self.cascades.get(i).copied().flatten()),
            SlotView::Other => {
                mark(self.main.map(|(k, _)| k));
                self.cascades.iter().for_each(|&c| mark(c));
            }
        }
        lods
    }

    fn main_draws(&self, lod: usize, set: usize) -> bool {
        self.main.is_some_and(|(k, simple)| k == lod && (simple || set != SIMPLE_SET))
    }

    fn cascade_draws(&self, i: usize, lod: usize, set: usize) -> bool {
        set != SIMPLE_SET && self.cascades.get(i).copied().flatten() == Some(lod)
    }

    fn main_carrier(&self, lod: usize, set: usize, own: usize) -> Option<usize> {
        if !self.main_draws(lod, set) {
            return None;
        }
        let simple = self.main.is_some_and(|(_, simple)| simple);
        Some(match self.carriers {
            Some(carriers) if simple => carriers[SIMPLE_SET],
            _ => own,
        })
    }

    fn cascade_carrier(&self, i: usize, lod: usize, set: usize, own: usize) -> Option<usize> {
        self.cascade_draws(i, lod, set).then(|| self.main_carrier(lod, set, own).unwrap_or(own))
    }

    fn shown_carrier(&self, lod: usize, set: usize, own: usize) -> Option<usize> {
        let shown = self.main_draws(lod, set) || (0..CASCADES).any(|c| self.cascade_draws(c, lod, set));
        shown.then(|| self.main_carrier(lod, set, own).unwrap_or(own))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TerrainPart {
    unit: usize,
    lod: usize,
    set: usize,
}

impl TerrainPart {
    pub(super) fn new(unit: usize, lod: usize, set: usize) -> Self {
        TerrainPart { unit, lod, set }
    }
}

#[derive(Clone, Copy)]
pub(super) enum SlotView {
    Main,
    Cascade(usize),
    Other,
}

#[derive(Resource, Default)]
pub struct TerrainLods {
    pub(super) units: Vec<TerrainUnit>,
    records: Option<Vec<usize>>,
    camera: Option<Entity>,
    priorities: Vec<f32>,
    order: Vec<(f32, usize)>,
    streamed: Vec<bool>,
    logged: (Counts, f32),
}

impl TerrainLods {
    pub(super) fn push(&mut self, unit: TerrainUnit) {
        self.units.push(unit);
        self.records = None;
    }

    pub(super) fn len(&self) -> usize {
        self.units.len()
    }

    pub(super) fn camera(&self) -> Option<Entity> {
        self.camera
    }

    pub(super) fn candidates(&self, view: SlotView) -> impl Iterator<Item = usize> + '_ {
        self.units.iter().flat_map(move |unit| {
            let lods = unit.lods_in(view);
            unit.parts.iter().zip(lods).filter(|&(_, drawn)| drawn).flat_map(|(parts, _)| parts.iter().map(|&(i, _)| i))
        })
    }

    pub(super) fn carrier(&self, part: &TerrainPart, view: SlotView, own: usize) -> Option<usize> {
        let unit = self.units.get(part.unit)?;
        match view {
            SlotView::Main => unit.main_carrier(part.lod, part.set, own),
            SlotView::Cascade(i) => unit.cascade_carrier(i, part.lod, part.set, own),
            SlotView::Other => unit.shown_carrier(part.lod, part.set, own),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counts {
    main: [usize; LOD_COUNT + 1],
    simple: usize,
    far: usize,
    triangles: (usize, usize),
    streamed: usize,
    draws: (usize, usize),
    cascades: [usize; CASCADES],
}

pub(super) fn register(app: &mut App) {
    app.add_systems(
        PostUpdate,
        update_terrain_lods
            .after(TransformSystems::Propagate)
            .after(bevy::camera::CameraUpdateSystems)
            .after(crate::render::shadows::fit_cascades)
            .before(VisibilitySystems::VisibilityPropagate),
    );
    crate::shell::level::scoped::<TerrainLods>(app);
}

fn record_order(units: &[TerrainUnit]) -> Vec<usize> {
    let mut records: Vec<usize> = (0..units.len()).filter(|&i| units[i].record.1 != usize::MAX).collect();
    records.sort_by_key(|&i| units[i].record);
    records
}

fn update_terrain_lods(
    lods: Option<ResMut<TerrainLods>>,
    cameras: Query<(Entity, &Camera, &GlobalTransform), With<Camera3d>>,
    suns: Query<&CascadeFit, With<GameSun>>,
    ride: Option<Res<crate::ride::Ride>>,
    distance: Option<Res<super::ViewDistance>>,
    time: Res<Time>,
) {
    let Some(mut lods) = lods else { return };
    let Some((camera, projection, view)) = cameras.iter().find(|(_, c, _)| c.is_active) else { return };
    let Some(tan) = metric_tan(projection) else { return };
    let (eye, forward) = (view.translation(), view.forward().as_vec3());
    let reach = reach(distance.map_or(CUT_OFF, |d| d.meters()));
    let cut_off = cut_off_depth(reach.slots, tan);
    let far_cut_off = reach.far.map(|m| cut_off_depth(m, tan));
    let views = suns
        .iter()
        .find(|f| f.camera == Some(camera))
        .map(|f| f.boxes.map(|(min, max)| CascadeView::of_box(f.light, min, max)));
    let start = ride.as_deref().and_then(crate::ride::Ride::stream_from_start);
    let position = stream_position(start, eye);
    let TerrainLods { units, records, camera: chosen_for, priorities, order, streamed, logged } = &mut *lods;
    *chosen_for = Some(camera);
    let records = records.get_or_insert_with(|| record_order(units));
    priorities.clear();
    priorities.extend(units.iter().map(|u| stream_priority(u.stream_centre, u.stream_reach, position)));
    stream_selection(priorities, records, reach.stream, order, streamed);
    let mut counts = Counts { streamed: order.len().min(STREAM_SLOTS), ..default() };
    for (unit, &is_streamed) in units.iter_mut().zip(streamed.iter()) {
        let metric = size_metric(unit.centre, unit.radius, eye, tan);
        let near_depth = (unit.centre - eye).dot(forward) - unit.radius;
        // PORT(choice): a streamed mesh is drawn from the frame it is chosen and an unstreamed one hidden at once
        // (every mesh of the drop is kept in memory); the console loaded a chosen mesh from the disc and built its
        // render slot over the following frames (0x829807B0 states 1 → 2 → 3) and released it in two steps (3 → 4 → 5).
        let slot = if is_streamed { slot_state(metric, near_depth, cut_off) } else { None };
        let far = match (slot, far_cut_off) {
            (None, Some(far)) => far_state(metric, near_depth, far),
            _ => None,
        };
        let main = slot.or(far);
        let cascades = match &views {
            Some(views) if is_streamed => {
                cascade_lods(&cascade_slots(views, unit.centre, unit.half, unit.radius), cut_off)
            }
            _ => [None; CASCADES],
        };
        counts.main[main.map_or(LOD_COUNT, |(k, _)| k)] += 1;
        counts.simple += usize::from(main.is_some_and(|(_, simple)| simple));
        counts.far += usize::from(far.is_some());
        counts.triangles.0 += main.map_or(0, |(k, _)| unit.triangles[k]);
        counts.triangles.1 += unit.triangles[0];
        counts.draws.0 +=
            main.map_or(0, |(k, simple)| unit.parts[k].iter().filter(|&&(_, set)| simple || set != SIMPLE_SET).count());
        counts.draws.1 += unit.parts[0].iter().filter(|&&(_, set)| set != SIMPLE_SET).count();
        for (n, lod) in counts.cascades.iter_mut().zip(cascades) {
            *n += usize::from(lod.is_some());
        }
        (unit.main, unit.cascades) = (main, cascades);
    }
    let now = time.elapsed_secs();
    if counts != logged.0 && now - logged.1 >= 1.0 {
        info!(
            "terrain LODs: {} streamed of {} (from the {}, {:.0} m); view 0: {} at LOD 0, {} at LOD 1, {} at LOD 2 ({} \
             simple, {} far detail), {} none (cut-off {cut_off:.0} m{}); {} of {} LOD-0 triangles, {} of {} LOD-0 draws \
             (before the views' culls); cascades {} / {} / {} slots",
            counts.streamed,
            units.len(),
            if start.is_some() { "ride's start" } else { "camera" },
            reach.stream,
            counts.main[0],
            counts.main[1],
            counts.main[2],
            counts.simple,
            counts.far,
            counts.main[LOD_COUNT],
            far_cut_off.map_or(String::new(), |f| format!(", far detail {f:.0} m")),
            counts.triangles.0,
            counts.triangles.1,
            counts.draws.0,
            counts.draws.1,
            counts.cascades[0],
            counts.cascades[1],
            counts.cascades[2],
        );
        *logged = (counts, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_by_factor() {
        assert_eq!(choose_lod(5.0), Some(0));
        assert_eq!(choose_lod(0.8), Some(0));
        assert_eq!(choose_lod(0.79), Some(1));
        assert_eq!(choose_lod(0.4), Some(1));
        assert_eq!(choose_lod(0.39), Some(2));
        assert_eq!(choose_lod(-1.0), Some(2));
        assert_eq!(choose_lod(f32::NAN), None);
    }

    fn sorted_records(keys: &[f32]) -> Vec<usize> {
        let mut v: Vec<(f32, usize)> = keys.iter().copied().zip(0..).collect();
        game_sort(&mut v);
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0), "not sorted");
        v.into_iter().map(|(_, r)| r).collect()
    }

    #[test]
    fn equal_priorities_end_in_the_games_order() {
        assert_eq!(sorted_records(&[5.0; 10]), (0..10).collect::<Vec<_>>());
        let keys: Vec<f32> = (0..40).map(|i| ((i * 7) % 5) as f32).collect();
        assert_eq!(
            sorted_records(&keys),
            [
                35, 30, 25, 20, 15, 10, 0, 5, 38, 8, 33, 13, 28, 23, 18, 3, 36, 31, 1, 26, 21, 16, 11, 6, 19, 24, 4,
                29, 14, 34, 9, 39, 22, 17, 2, 27, 12, 32, 7, 37
            ]
        );
        let keys: Vec<f32> = (0..200).map(|i| ((i * 37) % 10) as f32).collect();
        assert_eq!(
            sorted_records(&keys),
            [
                190, 180, 170, 160, 150, 140, 130, 120, 110, 100, 90, 80, 70, 60, 50, 40, 30, 20, 0, 10, 133, 123, 53,
                43, 143, 113, 153, 103, 3, 163, 33, 13, 63, 173, 93, 183, 23, 83, 193, 73, 96, 66, 106, 56, 86, 116,
                76, 126, 6, 46, 136, 146, 156, 36, 166, 16, 176, 26, 186, 196, 129, 119, 139, 149, 109, 159, 9, 99,
                169, 179, 89, 189, 79, 199, 59, 49, 39, 29, 19, 69, 102, 132, 62, 22, 122, 192, 182, 92, 142, 42, 172,
                52, 2, 12, 82, 152, 32, 72, 162, 112, 25, 125, 115, 185, 175, 135, 45, 5, 145, 15, 155, 35, 165, 55,
                75, 85, 195, 105, 65, 95, 18, 198, 78, 158, 168, 38, 148, 88, 68, 138, 98, 28, 8, 48, 118, 108, 188,
                128, 178, 58, 111, 161, 71, 31, 11, 81, 151, 121, 51, 171, 41, 181, 1, 91, 141, 101, 191, 21, 131, 61,
                194, 174, 24, 184, 74, 84, 94, 64, 104, 114, 54, 124, 14, 134, 44, 144, 4, 154, 34, 164, 137, 47, 177,
                127, 27, 147, 7, 117, 57, 187, 107, 157, 97, 167, 37, 17, 67, 87, 197, 77
            ]
        );
        let base = [
            1, 2, 3, 61, 53, 5, 51, 7, 57, 9, 49, 11, 64, 13, 47, 15, 55, 17, 45, 19, 59, 21, 43, 62, 60, 58, 56, 54,
            52, 50, 48, 46, 0, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 44, 41, 42, 39, 40, 37, 38, 35, 36, 33, 34, 31, 32,
            29, 30, 27, 28, 25, 26, 23, 24,
        ];
        let keys: Vec<f32> = base.iter().map(|k| (k / 2) as f32).collect();
        assert_eq!(
            sorted_records(&keys),
            [
                32, 0, 2, 1, 33, 5, 34, 7, 35, 9, 36, 11, 37, 13, 38, 15, 39, 17, 40, 19, 41, 21, 62, 42, 63, 60, 58,
                61, 59, 56, 57, 54, 55, 52, 53, 50, 51, 48, 46, 49, 44, 47, 45, 22, 18, 43, 31, 14, 10, 30, 6, 29, 4,
                28, 16, 27, 8, 26, 25, 20, 3, 24, 23, 12
            ]
        );
        let keys: Vec<f32> = (0..300).map(|i| ((i * 7919) % 300) as f32).collect();
        let order = sorted_records(&keys);
        assert!(order.windows(2).all(|w| keys[w[0]] < keys[w[1]]));
    }

    #[test]
    fn streaming_measures_from_the_start_until_gameplay() {
        let (start, eye) = (Vec3::new(604.3, 659.1, -227.4), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(stream_position(Some(start), eye), start);
        assert_eq!(stream_position(None, eye), eye);
    }

    #[test]
    fn streaming_keeps_the_nearest_180_under_the_distance_in_record_order() {
        let centre = Vec3::new(100.0, 0.0, 0.0);
        assert_eq!(stream_priority(centre, 30.0, Vec3::ZERO), 70.0);
        assert_eq!(stream_priority(centre, 130.0, Vec3::ZERO), -30.0);
        let (mut order, mut streamed) = (Vec::new(), Vec::new());
        stream_selection(&[599.9, 600.0, f32::NAN, -5.0], &[0, 1, 2, 3], STREAM_DISTANCE, &mut order, &mut streamed);
        assert_eq!(streamed, [true, false, false, true]);
        stream_selection(&[299.9, 300.0, 10.0], &[0, 1, 2], 300.0, &mut order, &mut streamed);
        assert_eq!(streamed, [true, false, true]);
        let priorities: Vec<f32> = (0..200).rev().map(|i| i as f32).collect();
        let records: Vec<usize> = (0..200).collect();
        stream_selection(&priorities, &records, STREAM_DISTANCE, &mut order, &mut streamed);
        assert_eq!(streamed.iter().filter(|&&s| s).count(), STREAM_SLOTS);
        assert!(streamed[20..].iter().all(|&s| s) && streamed[..20].iter().all(|&s| !s));
        let equal = vec![1.0; 200];
        stream_selection(&equal, &records, STREAM_DISTANCE, &mut order, &mut streamed);
        let first: Vec<usize> = (0..200).filter(|&i| streamed[i]).collect();
        let reversed: Vec<usize> = (0..200).rev().collect();
        stream_selection(&equal, &reversed, STREAM_DISTANCE, &mut order, &mut streamed);
        let second: Vec<usize> = (0..200).filter(|&i| streamed[i]).collect();
        assert_ne!(first, second);
        stream_selection(&equal, &records[1..], STREAM_DISTANCE, &mut order, &mut streamed);
        assert!(!streamed[0]);
    }

    #[test]
    fn cut_off_and_simple_techniques() {
        let tan_min = 1.0 / crate::camera::GAME_ASPECT;
        let cut = cut_off_depth(CUT_OFF, tan_min);
        assert!((cut - 600.0 * std::f32::consts::SQRT_2).abs() < 1e-3, "{cut}");
        assert_eq!(slot_state(5.0, 10.0, cut), Some((0, false)));
        assert_eq!(slot_state(5.0, 125.0, cut), Some((0, false)));
        assert_eq!(slot_state(0.5, 125.5, cut), Some((1, true)));
        assert_eq!(slot_state(0.1, cut - 1.0, cut), Some((2, true)));
        assert_eq!(slot_state(0.1, cut, cut), None);
        assert_eq!(slot_state(f32::NAN, 10.0, cut), None);
        assert_eq!(slot_state(VIEW_MIN_METRIC, 10.0, cut), None);
        assert_eq!(slot_state(0.0021, 10.0, cut), Some((2, false)));
        assert_eq!(slot_state(5.0, f32::NAN, cut), Some((0, true)));
    }

    #[test]
    fn the_view_distance_at_600_m_is_the_original() {
        let original = reach(600.0);
        assert_eq!(
            original,
            Reach { slots: CUT_OFF, stream: STREAM_DISTANCE, far: None, proxy_push: 600.0, proxy_clip: 480.0 }
        );
        assert_eq!(original.proxy_push / PROXY_BAND, 5.0);
        assert_eq!(
            reach(300.0),
            Reach { slots: 300.0, stream: 300.0, far: None, proxy_push: 300.0, proxy_clip: 180.0 }
        );
        let far = reach(2000.0);
        assert_eq!((far.slots, far.far, far.proxy_clip), (CUT_OFF, Some(2000.0), 480.0));
        assert!((far.stream - 2000.0).abs() < 1e-3, "{}", far.stream);
        assert_eq!(far_state(0.01, 1500.0, 2000.0), Some((2, true)));
        assert_eq!(far_state(0.01, 2000.0, 2000.0), None);
        assert_eq!(far_state(VIEW_MIN_METRIC, 100.0, 2000.0), None);
        assert_eq!(far_state(f32::NAN, 100.0, 2000.0), None);
    }

    fn cascade(min: [f32; 3], max: [f32; 3]) -> CascadeView {
        CascadeView::of_box(Mat3::IDENTITY, Vec3::from_array(min), Vec3::from_array(max))
    }

    #[test]
    fn a_cascade_tests_the_slots_box_against_its_box() {
        let c = cascade([-10.0, -10.0, -20.0], [10.0, 10.0, -10.0]);
        assert_eq!((c.min.z, c.max.z), (10.0, 20.0));
        let half = Vec3::splat(1.0);
        assert_eq!(c.containment(Vec3::new(0.0, 0.0, 15.0), half), Containment::Inside);
        assert_eq!(c.containment(Vec3::new(9.5, 0.0, 15.0), half), Containment::Intersecting);
        assert_eq!(c.containment(Vec3::new(11.5, 0.0, 15.0), half), Containment::Outside);
        assert_eq!(c.containment(Vec3::new(0.0, 0.0, 9.5), half), Containment::Intersecting);
        assert_eq!(c.containment(Vec3::new(0.0, 0.0, 21.5), half), Containment::Outside);
        let light = Mat3::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let r = CascadeView { light, ..c };
        let corner_reach = std::f32::consts::SQRT_2;
        let at = |x: f32| light.inverse() * Vec3::new(x, 0.0, 15.0);
        assert_eq!(r.containment(at(10.0 - corner_reach - 0.01), half), Containment::Inside);
        assert_eq!(r.containment(at(10.0 + corner_reach + 0.01), half), Containment::Outside);
        assert_eq!(cascade([0.0, 0.0, 0.0], [40.0, 20.0, 1.0]).metric(5.0), 0.5);
        assert_eq!(c.near_depth(Vec3::new(3.0, 4.0, -50.0), 2.0), 48.0);
    }

    #[test]
    fn a_cascade_skips_slots_inside_an_earlier_one() {
        use Containment::*;
        let cut = 800.0;
        let slot = |c, metric| (c, metric, 0.0);
        assert_eq!(
            cascade_lods(&[slot(Inside, 5.0), slot(Inside, 0.5), slot(Inside, 0.1)], cut),
            [Some(0), None, None]
        );
        assert_eq!(
            cascade_lods(&[slot(Intersecting, 5.0), slot(Inside, 0.5), slot(Inside, 0.1)], cut),
            [Some(0), Some(1), None]
        );
        assert_eq!(
            cascade_lods(&[slot(Outside, 5.0), slot(Intersecting, 0.5), slot(Inside, 0.1)], cut),
            [None, Some(1), Some(2)]
        );
        assert_eq!(cascade_lods(&[(Inside, 5.0, cut), slot(Outside, 0.5), slot(Outside, 0.1)], cut), [None; 3]);
        let views = [cascade([-10.0, -10.0, -20.0], [10.0, 10.0, -10.0]); CASCADES];
        let tiny = cascade_slots(&views, Vec3::new(0.0, 0.0, 15.0), Vec3::splat(0.1), 0.1);
        assert!(tiny.iter().all(|s| s.0 == Outside));
        let big = cascade_slots(&views, Vec3::new(0.0, 0.0, 15.0), Vec3::splat(1.0), 1.0);
        assert!(big.iter().all(|s| s.0 == Inside));
    }
}
