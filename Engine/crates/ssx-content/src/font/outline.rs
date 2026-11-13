use std::cell::Cell;
use std::collections::BTreeMap;

use skrifa::FontRef;
use skrifa::raw::TableProvider;
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::raw::ps::cff::index::Index;
use skrifa::raw::ps::cs::{self, CharstringContext, CharstringKind, CommandSink};
use skrifa::raw::ps::error::Error as CsError;
use skrifa::raw::tables::glyf::{Anchor, Glyf, Glyph};
use skrifa::raw::tables::loca::Loca;
use skrifa::raw::types::{Fixed, GlyphId};
use ssx_formats::gfx::shape::{Shape, ShapeRecord};

use super::Limits;

const MAX_DEPTH: usize = 32;
// PORT(choice): a CFF font's cubic curves become quadratic pieces within 2 units of the 20 480-unit EM (0.003 px at 26
// px, 0.1 px at 1 000 px), at most 8 halvings (256 pieces); the player's glyphs hold quadratics only (as DefineFont3's)
// and the player draws quadratics; a package font is the author's, not the disc's.
const CUBIC_TOLERANCE: f64 = 2.0;
const MAX_CUBIC_SPLITS: u32 = 8;

type Pt = (f64, f64, bool);

enum Kind<'a> {
    Glyf { loca: Loca<'a>, glyf: Glyf<'a>, costs: BTreeMap<u32, Result<(u32, u32), String>> },
    Cff { cff: CffFontRef<'a> },
}

pub struct Outlines<'a> {
    kind: Kind<'a>,
    glyphs: u32,
    limits: Limits,
    pub last: Cost,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub points: u64,
    pub visits: u32,
    pub subr_calls: u32,
    pub charstring_bytes: u64,
    pub records: usize,
}

impl<'a> Outlines<'a> {
    pub fn new(font: &FontRef<'a>, glyphs: u32, limits: &Limits) -> Result<Self, String> {
        let kind = if let (Ok(loca), Ok(glyf)) = (font.loca(None), font.glyf()) {
            Kind::Glyf { loca, glyf, costs: BTreeMap::new() }
        } else if let Ok(cff) = font.cff() {
            let cff = CffFontRef::new_cff(cff.offset_data().as_bytes(), 0, None).map_err(|e| format!("CFF: {e}"))?;
            Kind::Cff { cff }
        } else if let Ok(cff2) = font.cff2() {
            let cff = CffFontRef::new_cff2(cff2.offset_data().as_bytes(), None).map_err(|e| format!("CFF2: {e}"))?;
            Kind::Cff { cff }
        } else {
            return Err("no outlines (glyf + loca, CFF or CFF2)".into());
        };
        Ok(Outlines { kind, glyphs, limits: *limits, last: Cost::default() })
    }

    pub fn kind(&self) -> &'static str {
        match &self.kind {
            Kind::Glyf { .. } => "glyf",
            Kind::Cff { cff } if cff.version() == 2 => "CFF2",
            Kind::Cff { .. } => "CFF",
        }
    }

    pub fn shape(&mut self, g: GlyphId, scale: f64) -> Result<Shape, String> {
        self.last = Cost::default();
        let contours = match &mut self.kind {
            Kind::Glyf { loca, glyf, costs } => {
                let (points, visits) = cost(loca, glyf, costs, g.to_u32(), self.glyphs, &self.limits, &mut Vec::new())?;
                (self.last.points, self.last.visits) = (u64::from(points), visits);
                if points > self.limits.max_glyph_points {
                    return Err(format!(
                        "{points} points with its components (at most {})",
                        self.limits.max_glyph_points
                    ));
                }
                collect(loca, glyf, g.to_u32(), 0)?.into_iter().map(quadratic_contour).collect()
            }
            Kind::Cff { cff } => cff_contours(cff, g, &self.limits, &mut self.last)?,
        };
        let shape = records(&contours, scale, &self.limits)?;
        self.last.records = shape.records.len();
        Ok(shape)
    }
}

fn cost(
    loca: &Loca,
    glyf: &Glyf,
    memo: &mut BTreeMap<u32, Result<(u32, u32), String>>,
    g: u32,
    glyphs: u32,
    limits: &Limits,
    stack: &mut Vec<u32>,
) -> Result<(u32, u32), String> {
    if let Some(c) = memo.get(&g) {
        return c.clone();
    }
    if g >= glyphs {
        return Err(format!("a component names glyph {g} of {glyphs}"));
    }
    if stack.contains(&g) {
        return Err(format!("a cycle of composite glyphs through glyph {g}"));
    }
    if stack.len() >= MAX_DEPTH {
        return Err(format!("composite glyphs nested deeper than {MAX_DEPTH}"));
    }
    let glyph = loca.get_glyf(GlyphId::new(g), glyf).map_err(|e| format!("glyph {g}: {e}"))?;
    let c = match glyph {
        None => Ok((0, 1)),
        Some(Glyph::Simple(s)) => Ok((u32::try_from(s.num_points()).unwrap_or(u32::MAX), 1)),
        Some(Glyph::Composite(c)) => {
            stack.push(g);
            let (mut points, mut visits) = (0u32, 1u32);
            let mut r = Ok(());
            for comp in c.components() {
                match cost(loca, glyf, memo, comp.glyph.to_u32(), glyphs, limits, stack) {
                    Ok((p, v)) => {
                        points = points.saturating_add(p);
                        visits = visits.saturating_add(v);
                    }
                    Err(e) => {
                        r = Err(e);
                        break;
                    }
                }
                if visits > limits.max_component_visits || points > limits.max_glyph_points {
                    r = Err(format!(
                        "a composite glyph of more than {} components or {} points",
                        limits.max_component_visits, limits.max_glyph_points
                    ));
                    break;
                }
            }
            stack.pop();
            r.map(|()| (points, visits))
        }
    };
    memo.insert(g, c.clone());
    c
}

fn collect(loca: &Loca, glyf: &Glyf, g: u32, depth: usize) -> Result<Vec<Vec<Pt>>, String> {
    if depth > MAX_DEPTH {
        return Err(format!("composite glyphs nested deeper than {MAX_DEPTH}"));
    }
    match loca.get_glyf(GlyphId::new(g), glyf).map_err(|e| format!("glyph {g}: {e}"))? {
        None => Ok(Vec::new()),
        Some(Glyph::Simple(s)) => {
            let points: Vec<_> = s.points().collect();
            let (mut out, mut start) = (Vec::new(), 0usize);
            for end in s.end_pts_of_contours() {
                let end = usize::from(end.get());
                if end < start || end >= points.len() {
                    return Err(format!("glyph {g}: a contour ends at point {end} of {}", points.len()));
                }
                out.push(points[start..=end].iter().map(|p| (f64::from(p.x), f64::from(p.y), p.on_curve)).collect());
                start = end + 1;
            }
            Ok(out)
        }
        Some(Glyph::Composite(c)) => {
            use skrifa::raw::tables::glyf::CompositeGlyphFlags as F;
            let mut out: Vec<Vec<Pt>> = Vec::new();
            for comp in c.components() {
                let t = comp.transform;
                let m = [t.xx.to_f32(), t.yx.to_f32(), t.xy.to_f32(), t.yy.to_f32()].map(f64::from);
                let apply = |x: f64, y: f64| (m[0] * x + m[2] * y, m[1] * x + m[3] * y);
                let mut sub = collect(loca, glyf, comp.glyph.to_u32(), depth + 1)?;
                for p in sub.iter_mut().flatten() {
                    (p.0, p.1) = apply(p.0, p.1);
                }
                let (dx, dy) = match comp.anchor {
                    Anchor::Offset { x, y } => {
                        let (x, y) = (f64::from(x), f64::from(y));
                        let scaled = comp.flags.contains(F::SCALED_COMPONENT_OFFSET)
                            && !comp.flags.contains(F::UNSCALED_COMPONENT_OFFSET);
                        if scaled { apply(x, y) } else { (x, y) }
                    }
                    Anchor::Point { base, component } => {
                        let b = out.iter().flatten().nth(usize::from(base));
                        let own = sub.iter().flatten().nth(usize::from(component));
                        let (Some(b), Some(o)) = (b, own) else {
                            return Err(format!("glyph {g}: an anchor point out of range"));
                        };
                        (b.0 - o.0, b.1 - o.1)
                    }
                };
                for contour in sub {
                    out.push(contour.into_iter().map(|(x, y, on)| (x + dx, y + dy, on)).collect());
                }
            }
            Ok(out)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    Line((f64, f64)),
    Quad((f64, f64), (f64, f64)),
    Cubic((f64, f64), (f64, f64), (f64, f64)),
}

fn quadratic_contour(points: Vec<Pt>) -> ((f64, f64), Vec<Seg>) {
    let n = points.len();
    if n == 0 {
        return ((0.0, 0.0), Vec::new());
    }
    let mid = |a: &Pt, b: &Pt| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let first_on = points.iter().position(|p| p.2);
    let (start, rot) = match first_on {
        Some(i) => ((points[i].0, points[i].1), i),
        None => (mid(&points[0], &points[n.min(2) - 1]), 0),
    };
    let mut segs = Vec::new();
    let mut control: Option<(f64, f64)> = None;
    for k in 1..=n {
        let p = &points[(rot + k) % n];
        match (p.2, control) {
            (true, None) => segs.push(Seg::Line((p.0, p.1))),
            (true, Some(c)) => {
                segs.push(Seg::Quad(c, (p.0, p.1)));
                control = None;
            }
            (false, None) => control = Some((p.0, p.1)),
            (false, Some(c)) => {
                let m = ((c.0 + p.0) / 2.0, (c.1 + p.1) / 2.0);
                segs.push(Seg::Quad(c, m));
                control = Some((p.0, p.1));
            }
        }
    }
    if let Some(c) = control {
        segs.push(Seg::Quad(c, start));
    }
    (start, segs)
}

struct Counted<'a> {
    inner: (&'a [u8], &'a Index<'a>, &'a Index<'a>, &'a Index<'a>),
    global: &'a Index<'a>,
    local: &'a Index<'a>,
    calls: Cell<u32>,
    bytes: Cell<u64>,
    limits: Limits,
}

impl Counted<'_> {
    fn take<'b>(&self, data: Result<&'b [u8], CsError>) -> Result<&'b [u8], CsError> {
        let data = data?;
        let calls = self.calls.get().saturating_add(1);
        let bytes = self.bytes.get().saturating_add(data.len() as u64);
        self.calls.set(calls);
        self.bytes.set(bytes);
        if calls > self.limits.max_subr_calls || bytes > self.limits.max_charstring_bytes {
            return Err(CsError::CharstringNestingDepthLimitExceeded);
        }
        Ok(data)
    }

    fn over(&self) -> bool {
        self.calls.get() > self.limits.max_subr_calls || self.bytes.get() > self.limits.max_charstring_bytes
    }
}

impl CharstringContext for Counted<'_> {
    fn kind(&self) -> CharstringKind {
        CharstringKind::Type2
    }

    fn seac_components(&self, base_code: i32, accent_code: i32) -> Result<[&[u8]; 2], CsError> {
        let [a, b] = self.inner.seac_components(base_code, accent_code)?;
        Ok([self.take(Ok(a))?, self.take(Ok(b))?])
    }

    fn global_subr(&self, index: i32) -> Result<&[u8], CsError> {
        let i = index
            .checked_add(self.global.subr_bias())
            .filter(|&i| i >= 0)
            .ok_or(CsError::InvalidCharstringOperator(29))?;
        self.take(self.global.get(i as usize))
    }

    fn subr(&self, index: i32) -> Result<&[u8], CsError> {
        let i = index
            .checked_add(self.local.subr_bias())
            .filter(|&i| i >= 0)
            .ok_or(CsError::InvalidCharstringOperator(10))?;
        self.take(self.local.get(i as usize))
    }
}

#[derive(Default)]
struct Sink {
    contours: Vec<((f64, f64), Vec<Seg>)>,
    open: bool,
    points: u64,
}

fn fx(v: Fixed) -> f64 {
    f64::from(v.to_f32())
}

impl CommandSink for Sink {
    fn move_to(&mut self, x: Fixed, y: Fixed) {
        self.contours.push(((fx(x), fx(y)), Vec::new()));
        self.open = true;
        self.points += 1;
    }

    fn line_to(&mut self, x: Fixed, y: Fixed) {
        if let Some(c) = self.contours.last_mut() {
            c.1.push(Seg::Line((fx(x), fx(y))));
        }
        self.points += 1;
    }

    fn curve_to(&mut self, cx0: Fixed, cy0: Fixed, cx1: Fixed, cy1: Fixed, x: Fixed, y: Fixed) {
        if let Some(c) = self.contours.last_mut() {
            c.1.push(Seg::Cubic((fx(cx0), fx(cy0)), (fx(cx1), fx(cy1)), (fx(x), fx(y))));
        }
        self.points += 3;
    }

    fn close(&mut self) {
        self.open = false;
    }
}

type Contour = ((f64, f64), Vec<Seg>);

fn cff_contours(cff: &CffFontRef, g: GlyphId, limits: &Limits, cost: &mut Cost) -> Result<Vec<Contour>, String> {
    let fd = cff.subfont_index(g).unwrap_or(0);
    let subfont = cff.subfont(fd, &[]).map_err(|e| format!("CFF: {e}"))?;
    let local = if subfont.subrs_offset() != 0 {
        let data =
            cff.data().get(subfont.subrs_offset() as usize..).ok_or("CFF: the local subroutines past the table")?;
        Index::new(data, cff.version() == 2).map_err(|e| format!("CFF: {e}"))?
    } else {
        Index::Empty
    };
    let charstrings = cff.charstrings();
    let data = charstrings.get(g.to_u32() as usize).map_err(|e| format!("CFF: {e}"))?;
    let ctx = Counted {
        inner: (cff.data(), charstrings, cff.global_subrs(), &local),
        global: cff.global_subrs(),
        local: &local,
        calls: Cell::new(0),
        bytes: Cell::new(data.len() as u64),
        limits: *limits,
    };
    let blend = cff
        .var_store()
        .and_then(|s| skrifa::raw::ps::cff::blend::BlendState::new(s.clone(), &[], subfont.vs_index()).ok());
    let mut sink = Sink::default();
    let mut transformed = cs::TransformSink::new(&mut sink, cff.transform(&subfont, None));
    let r = cs::evaluate(&ctx, blend, data, &mut transformed);
    (cost.subr_calls, cost.charstring_bytes, cost.points) = (ctx.calls.get(), ctx.bytes.get(), sink.points);
    if ctx.over() {
        return Err(format!(
            "its charstring calls more than {} subroutines or {} bytes",
            limits.max_subr_calls, limits.max_charstring_bytes
        ));
    }
    r.map_err(|e| format!("CFF: {e}"))?;
    if sink.points > u64::from(limits.max_glyph_points) {
        return Err(format!("{} points (at most {})", sink.points, limits.max_glyph_points));
    }
    Ok(sink.contours)
}

fn cubic_to_quads(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64), depth: u32, out: &mut Vec<Seg>) {
    let d = ((p3.0 - 3.0 * c2.0 + 3.0 * c1.0 - p0.0).powi(2) + (p3.1 - 3.0 * c2.1 + 3.0 * c1.1 - p0.1).powi(2)).sqrt();
    if d * 3f64.sqrt() / 36.0 <= CUBIC_TOLERANCE || depth >= MAX_CUBIC_SPLITS {
        let q = ((3.0 * (c1.0 + c2.0) - p0.0 - p3.0) / 4.0, (3.0 * (c1.1 + c2.1) - p0.1 - p3.1) / 4.0);
        out.push(Seg::Quad(q, p3));
        return;
    }
    let m = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (a, b, c) = (m(p0, c1), m(c1, c2), m(c2, p3));
    let (ab, bc) = (m(a, b), m(b, c));
    let mid = m(ab, bc);
    cubic_to_quads(p0, a, ab, mid, depth + 1, out);
    cubic_to_quads(mid, bc, c, p3, depth + 1, out);
}

fn records(contours: &[((f64, f64), Vec<Seg>)], scale: f64, limits: &Limits) -> Result<Shape, String> {
    let to = |p: (f64, f64)| -> Result<(i32, i32), String> {
        let (x, y) = ((p.0 * scale).round(), (-p.1 * scale).round());
        if !(x.is_finite() && y.is_finite()) || x.abs() > 1e7 || y.abs() > 1e7 {
            return Err("a point outside ±500 EM".into());
        }
        Ok((x as i32, y as i32))
    };
    let mut out = Vec::new();
    let mut first = true;
    for (start, segs) in contours {
        if segs.is_empty() {
            continue;
        }
        let mut cur = to(*start)?;
        out.push(ShapeRecord::StyleChange {
            move_to: Some(cur),
            fill0: first.then_some(0),
            fill1: first.then_some(1),
            line: None,
            new_styles: None,
        });
        first = false;
        let mut pieces = Vec::new();
        let mut at = *start;
        for s in segs {
            match *s {
                Seg::Cubic(c1, c2, p) => {
                    let sc = |q: (f64, f64)| (q.0 * scale, q.1 * scale);
                    let mut q = Vec::new();
                    cubic_to_quads(sc(at), sc(c1), sc(c2), sc(p), 0, &mut q);
                    pieces.extend(q.into_iter().map(|s| match s {
                        Seg::Quad(c, e) => Seg::Quad((c.0 / scale, c.1 / scale), (e.0 / scale, e.1 / scale)),
                        other => other,
                    }));
                    at = p;
                }
                Seg::Line(p) | Seg::Quad(_, p) => {
                    pieces.push(*s);
                    at = p;
                }
            }
        }
        if (at.0 - start.0).abs() > 1e-9 || (at.1 - start.1).abs() > 1e-9 {
            pieces.push(Seg::Line(*start));
        }
        for s in pieces {
            match s {
                Seg::Line(p) => {
                    let e = to(p)?;
                    if e != cur {
                        out.push(ShapeRecord::Straight { dx: e.0 - cur.0, dy: e.1 - cur.1 });
                        cur = e;
                    }
                }
                Seg::Quad(c, p) => {
                    let (c, e) = (to(c)?, to(p)?);
                    out.push(ShapeRecord::Curved {
                        control_dx: c.0 - cur.0,
                        control_dy: c.1 - cur.1,
                        anchor_dx: e.0 - c.0,
                        anchor_dy: e.1 - c.1,
                    });
                    cur = e;
                }
                Seg::Cubic(..) => {}
            }
            if out.len() > limits.max_glyph_points as usize * 4 {
                return Err(format!("more than {} records in one glyph", limits.max_glyph_points as usize * 4));
            }
        }
    }
    Ok(Shape { styles: Default::default(), records: out })
}

pub(super) fn bounds(shape: &Shape) -> ssx_formats::gfx::types::Rect {
    let (mut x, mut y) = (0i64, 0i64);
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut see = |x: i64, y: i64| {
        b = [b[0].min(x as f64), b[1].min(y as f64), b[2].max(x as f64), b[3].max(y as f64)];
    };
    for r in &shape.records {
        match *r {
            ShapeRecord::StyleChange { move_to: Some((mx, my)), .. } => {
                (x, y) = (i64::from(mx), i64::from(my));
                see(x, y);
            }
            ShapeRecord::StyleChange { .. } => {}
            ShapeRecord::Straight { dx, dy } => {
                (x, y) = (x + i64::from(dx), y + i64::from(dy));
                see(x, y);
            }
            ShapeRecord::Curved { control_dx, control_dy, anchor_dx, anchor_dy } => {
                see(x + i64::from(control_dx), y + i64::from(control_dy));
                (x, y) = (x + i64::from(control_dx + anchor_dx), y + i64::from(control_dy + anchor_dy));
                see(x, y);
            }
        }
    }
    if b[0] > b[2] { super::rect_of([0.0; 4]) } else { super::rect_of(b) }
}
