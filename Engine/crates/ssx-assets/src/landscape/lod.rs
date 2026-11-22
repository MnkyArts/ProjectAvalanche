use anyhow::{Context, Result, bail};

use crate::subdiv::{GAME_LEVELS, SubdLevel};
use crate::terrain::is_hidden_face;

pub const LOD_COUNT: usize = 3;

const LOD_START: [usize; LOD_COUNT] = [0, 2, 6];

const STITCH_EDGE: [u8; 64] = {
    let mut t = [4u8; 64];
    let set = [(2, 0), (6, 1), (0x12, 1), (0x16, 2), (0x22, 2), (0x26, 3), (0x32, 3), (0x36, 0)];
    let mut i = 0;
    while i < set.len() {
        t[set[i].0] = set[i].1;
        i += 1;
    }
    t
};

const SPLIT_BITS: [u8; 5] = [1, 2, 4, 8, 0];
const MIDPOINT_BITS: [u8; 5] = [0x10, 0x20, 0x40, 0x80, 0];

const SPLIT_STEPS: [usize; 3] = [1, 3, 9];
const MIDPOINT_OFFSET: [usize; LOD_COUNT] = [0, 1, 4];

const BLOCK_PER_CORNER: usize = 16;

pub fn face_group(unk_a: u8) -> u8 {
    if unk_a & 2 != 0 {
        1
    } else if unk_a & 4 != 0 {
        2
    } else {
        0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LodList {
    pub indices: Vec<u32>,
    pub groups: [Option<(usize, usize)>; 3],
}

impl LodList {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

pub fn lod_lists(cage: &SubdLevel, refined: &SubdLevel) -> Result<[LodList; LOD_COUNT]> {
    let expected: usize = cage.faces.iter().map(|f| 4usize.pow(GAME_LEVELS as u32 - 1) * usize::from(f.len)).sum();
    if refined.faces.len() != expected || cage.faces.iter().any(|f| !(3..=4).contains(&f.len)) {
        bail!(
            "refined mesh: {} faces for a cage of {} faces (expected {expected})",
            refined.faces.len(),
            cage.faces.len()
        );
    }
    let mut quads = Vec::with_capacity(refined.faces.len() * 4);
    for (face, corners) in refined.faces.iter().zip(refined.face_loops()) {
        if is_hidden_face(face.unk_a) {
            continue;
        }
        if corners.len() != 4 {
            bail!("refined face with {} corners", corners.len());
        }
        quads.extend(corners);
    }
    let mut lods: [LodList; LOD_COUNT] = Default::default();
    for (lod, out) in lods.iter_mut().enumerate() {
        *out = build_lod(cage, &quads, lod).with_context(|| format!("LOD {lod}"))?;
    }
    Ok(lods)
}

fn build_lod(cage: &SubdLevel, quads: &[u32], lod: usize) -> Result<LodList> {
    let step = 4usize.pow(lod as u32);
    let start = LOD_START[lod];
    let last = lod == LOD_COUNT - 1;
    let mut w = Writer { quads, out: LodList::default() };
    let (mut p, mut in_face, mut block, mut next_face) = (start, 0usize, 0usize, 0usize);
    let mut two = true;
    let (mut flags, mut group) = (0u8, 0u8);
    let mut run: Option<u8> = None;
    let (mut run_start, mut run_triangles) = (0usize, 0usize);
    while p < quads.len() {
        if in_face >= block {
            let face = cage.faces.get(next_face).context("more quads than cage faces")?;
            next_face += 1;
            if is_hidden_face(face.unk_a) {
                break;
            }
            block = usize::from(face.len) * BLOCK_PER_CORNER;
            in_face = 0;
            if last {
                two = face.len == 4;
            }
            flags = face.unk_flags;
            group = face_group(face.unk_a);
        }
        let before = w.out.triangle_count();
        if lod != 0 && flags & 0xF != 0 {
            let s = Stitch { p, in_face, step, start, lod, block, flags, two };
            w.stitch(&s)?;
            let corners = if two { 4 } else { 3 };
            p += corners * step;
            in_face += corners * step;
        } else {
            let [a, b, c] = [w.at(p)?, w.at(p + step)?, w.at(p + 2 * step)?];
            w.triangle(a, b, c);
            in_face += 3 * step;
            if two {
                let d = w.at(p + 3 * step)?;
                w.triangle(a, c, d);
                in_face += step;
            }
            p += if two { 4 } else { 3 } * step;
        }
        if run != Some(group) {
            if let Some(g) = run {
                w.out.groups[usize::from(g)] = Some((run_start, run_triangles));
                run_start += 3 * run_triangles;
                run_triangles = 0;
            }
            run = Some(group);
        }
        run_triangles += w.out.triangle_count() - before;
    }
    if let Some(g) = run {
        w.out.groups[usize::from(g)] = Some((run_start, run_triangles));
    }
    Ok(w.out)
}

struct Stitch {
    p: usize,
    in_face: usize,
    step: usize,
    start: usize,
    lod: usize,
    block: usize,
    flags: u8,
    two: bool,
}

struct Writer<'a> {
    quads: &'a [u32],
    out: LodList,
}

impl Writer<'_> {
    fn at(&self, i: usize) -> Result<u32> {
        self.quads.get(i).copied().with_context(|| format!("index {i} past the {} of LOD 0", self.quads.len()))
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.out.indices.extend_from_slice(&[a, b, c]);
    }

    fn stitch(&mut self, s: &Stitch) -> Result<()> {
        let count = if s.two { 4 } else { 3 };
        let mut corners = [0u32; 4];
        for (i, c) in corners.iter_mut().enumerate().take(count) {
            *c = self.at(s.p + i * s.step)?;
        }
        let centre = self.at(s.p + s.step / 2)?;
        for i in 0..count {
            let slot = s.start + s.in_face + i * s.step;
            let mut edge = *STITCH_EDGE.get(slot).with_context(|| format!("stitch table index {slot}"))?;
            if s.block.is_multiple_of(3) {
                edge %= 3;
            }
            let e = usize::from(edge);
            let (from, to) = (corners[i], corners[(i + 1) % count]);
            if SPLIT_BITS[e] & s.flags != 0 {
                let mut r = i * s.step + s.start;
                let mut v = from;
                for &k in &SPLIT_STEPS[..(1 << s.lod) - 1] {
                    r = (r + k) % s.block;
                    let next = self.at(s.p - s.start + r)?;
                    self.triangle(v, next, centre);
                    v = next;
                }
                self.triangle(v, to, centre);
            } else if MIDPOINT_BITS[e] & s.flags != 0 {
                let mid = self.at(s.p + i * s.step + MIDPOINT_OFFSET[s.lod])?;
                self.triangle(from, mid, centre);
                self.triangle(mid, to, centre);
            } else {
                self.triangle(from, to, centre);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subdiv::{Face, Vertex};

    fn cage(faces: &[(usize, u8, u8)]) -> SubdLevel {
        let mut m = SubdLevel::default();
        for (k, &(n, unk_a, unk_flags)) in faces.iter().enumerate() {
            let v0 = m.vertices.len() as u32;
            let e0 = m.edges.len() as u32;
            let x = 2.0 * k as f64;
            let corners: &[[f64; 3]] = if n == 4 {
                &[[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x + 1.0, 0.0, 1.0], [x, 0.0, 1.0]]
            } else {
                &[[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 0.0, 1.0]]
            };
            for &position in corners {
                m.vertices.push(Vertex { position, attrs: [0.0; 8] });
            }
            let mut edges = [u32::MAX; 4];
            for i in 0..n as u32 {
                m.edges.push([v0 + i, v0 + (i + 1) % n as u32]);
                edges[i as usize] = e0 + i;
            }
            m.faces.push(Face { reversed_mask: 0, len: n as u8, edges, unk_a, unk_flags });
        }
        m
    }

    fn lods(c: &SubdLevel) -> ([LodList; LOD_COUNT], Vec<u32>) {
        let refined = c.clone().refined(GAME_LEVELS);
        let quads = refined.faces.iter().zip(refined.face_loops()).filter(|(f, _)| f.unk_a & 1 == 0);
        let quads = quads.flat_map(|(_, l)| l).collect();
        (lod_lists(c, &refined).expect("lods"), quads)
    }

    fn tris(l: &LodList) -> Vec<[u32; 3]> {
        l.indices.chunks(3).map(|t| [t[0], t[1], t[2]]).collect()
    }

    #[test]
    fn quad_lods_pick_the_coarser_corners() {
        let c = cage(&[(4, 0, 0)]);
        let ([l0, l1, l2], q) = lods(&c);
        assert_eq!(l0.triangle_count(), 32);
        assert_eq!(&tris(&l0)[..2], &[[q[0], q[1], q[2]], [q[0], q[2], q[3]]]);
        assert_eq!(l1.triangle_count(), 8);
        assert_eq!(&tris(&l1)[..2], &[[q[2], q[6], q[10]], [q[2], q[10], q[14]]]);
        assert_eq!(tris(&l2), vec![[22, 23, 24], [22, 24, 21]]);
        assert_eq!([q[6], q[22], q[38], q[54]], [22, 23, 24, 21]);
        for l in [&l0, &l1, &l2] {
            assert_eq!(l.groups, [Some((0, l.triangle_count())), None, None]);
        }
    }

    #[test]
    fn cage_triangle_is_one_triangle_on_the_last_lod() {
        let ([l0, l1, l2], q) = lods(&cage(&[(3, 0, 0)]));
        assert_eq!((l0.triangle_count(), l1.triangle_count()), (24, 6));
        assert_eq!(tris(&l2), vec![[q[6], q[22], q[38]]]);
    }

    #[test]
    fn split_and_midpoint_edges_fan_around_the_centre() {
        let ([_, _, l2], q) = lods(&cage(&[(4, 0, 0x42)]));
        let c = q[14];
        let want = vec![
            [q[6], q[7], c],
            [q[7], q[10], c],
            [q[10], q[19], c],
            [q[19], q[22], c],
            [q[22], q[26], c],
            [q[26], q[38], c],
            [q[38], q[54], c],
            [q[54], q[6], c],
        ];
        assert_eq!(tris(&l2), want);
        assert_eq!([c, q[10], q[26]], [16, 18, 19]);
    }

    #[test]
    fn midpoint_bits_alone_leave_the_quads() {
        let ([_, l1, l2], _) = lods(&cage(&[(4, 0, 0xF0)]));
        assert_eq!((l1.triangle_count(), l2.triangle_count()), (8, 2));
    }

    #[test]
    fn inner_edges_of_a_cage_triangle_count_as_its_edge_one() {
        let ([_, l1, _], _) = lods(&cage(&[(3, 0, 0x02)]));
        assert_eq!(l1.triangle_count(), 7 + 7 + 6);
    }

    #[test]
    fn groups_keep_their_last_run_and_hidden_faces_end_the_list() {
        let ([l0, _, l2], _) = lods(&cage(&[(4, 0, 0), (4, 2, 0), (4, 0, 0)]));
        assert_eq!(l0.groups, [Some((192, 32)), Some((96, 32)), None]);
        assert_eq!(l2.groups, [Some((12, 2)), Some((6, 2)), None]);
        let ([l0, l1, l2], _) = lods(&cage(&[(4, 4, 0), (4, 1, 0), (4, 0, 0)]));
        assert_eq!([l0.triangle_count(), l1.triangle_count(), l2.triangle_count()], [32, 8, 2]);
        assert_eq!(l0.groups, [None, None, Some((0, 32))]);
    }
}
