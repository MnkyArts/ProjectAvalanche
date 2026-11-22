use crate::reader::Reader;
use crate::{Error, Result};

pub const MESH_VERSION: u32 = 0xEA00_0006;

const PREAMBLE: ([u16; 2], [u32; 7]) = ([1, 1], [2, 2, 2, 2, 12, 4, 2]);

#[derive(Clone, Debug)]
pub struct SubdFile {
    pub nodes: Vec<SubdNode>,
    pub leaf_mesh: Vec<u16>,
    pub meshes: Vec<SubdMesh>,
}

#[derive(Clone, Copy, Debug)]
pub struct SubdNode {
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
    pub index: u16,
    pub leaf_count: u16,
    pub children: [u16; 2],
}

impl SubdNode {
    pub fn is_leaf(&self) -> bool {
        self.leaf_count == 1
    }
}

#[derive(Clone, Debug)]
pub struct SubdMesh {
    pub version: u32,
    pub unk_header: [u8; 3],
    pub vertices: Vec<SubdVertex>,
    pub edges: Vec<[u16; 2]>,
    pub faces: Vec<SubdFace>,
    pub unk_face_flags: Vec<u8>,
    pub unk_tail: [u16; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct SubdVertex {
    pub position: [f32; 3],
    pub attributes: [f32; 8],
}

#[derive(Clone, Debug)]
pub struct SubdFace {
    pub unk_a: u8,
    pub reversed_mask: u8,
    pub edges: Vec<u16>,
}

impl SubdMesh {
    pub fn face_vertices<'a>(&'a self, face: &'a SubdFace) -> impl Iterator<Item = u16> + 'a {
        face.edges.iter().enumerate().map(move |(i, &e)| {
            let [a, b] = self.edges[e as usize];
            if face.reversed_mask >> i & 1 == 1 { b } else { a }
        })
    }
}

impl SubdFile {
    pub fn mesh_of_leaf(&self, node: &SubdNode) -> Option<&SubdMesh> {
        let mesh = *self.leaf_mesh.get(node.index as usize)?;
        self.meshes.get(mesh as usize)
    }

    pub fn meshes_in_tree_order(&self) -> Result<Vec<usize>> {
        let Some(root) = self.nodes.first() else { return Ok(Vec::new()) };
        let mut out = Vec::with_capacity(self.meshes.len());
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let node = self.nodes.get(i).ok_or_else(|| Error::invalid(format!("tree child {i} past the nodes")))?;
            if std::mem::replace(&mut seen[i], true) {
                return Err(Error::invalid(format!("the tree reaches node {i} twice")));
            }
            let [first, second] = node.children.map(usize::from);
            if first == 0 && second == 0 {
                let slot = usize::from(root.index) + usize::from(node.index);
                let mesh = self
                    .leaf_mesh
                    .get(slot)
                    .ok_or_else(|| Error::invalid(format!("leaf slot {slot} past the table")))?;
                out.push(usize::from(*mesh));
                continue;
            }
            if second != 0 {
                stack.push(second);
            }
            if first != 0 {
                stack.push(first);
            }
        }
        Ok(out)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let mut preamble = ([r.u16()?, r.u16()?], [0u32; 7]);
        for v in &mut preamble.1 {
            *v = r.u32()?;
        }
        if preamble != PREAMBLE {
            return Err(Error::invalid(format!("unexpected .subd preamble {preamble:?}")));
        }

        let node_count = r.u16()?;
        let node_count_2 = r.u16()?;
        let leaf_count = r.u16()?;
        let node_size = r.u16()?;
        let node_count_3 = r.u16()?;
        if node_count != node_count_2 || node_count != node_count_3 || node_size != 30 {
            return Err(Error::invalid(format!(
                "unexpected .subd tree header ({node_count}, {node_count_2}, {leaf_count}, {node_size}, {node_count_3})"
            )));
        }

        let leaf_mesh = (0..leaf_count).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
        let nodes = (0..node_count)
            .map(|_| {
                Ok(SubdNode {
                    center: r.vec3()?,
                    half_extents: r.vec3()?,
                    index: r.u16()?,
                    leaf_count: r.u16()?,
                    children: [r.u16()?, r.u16()?],
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mesh_count = r.u16()?;
        if mesh_count != leaf_count {
            return Err(Error::invalid(format!("{mesh_count} meshes but {leaf_count} leaves")));
        }
        let meshes = (0..mesh_count).map(|_| parse_mesh(&mut r)).collect::<Result<Vec<_>>>()?;

        if r.remaining() != 0 {
            return Err(Error::invalid(format!("{} trailing bytes after .subd meshes", r.remaining())));
        }
        if let Some(&bad) = leaf_mesh.iter().find(|&&m| m >= mesh_count) {
            return Err(Error::invalid(format!("leaf mesh index {bad} out of range ({mesh_count} meshes)")));
        }
        Ok(Self { nodes, leaf_mesh, meshes })
    }
}

fn parse_mesh(r: &mut Reader) -> Result<SubdMesh> {
    let version_pos = r.pos();
    let version = r.u32()?;
    if version != MESH_VERSION {
        return Err(Error::invalid(format!("unexpected .subd mesh version {version:#x} at {version_pos:#x}")));
    }
    let unk_header = r.array::<3>()?;
    let face_count = r.u16()? as usize;
    let edge_count = r.u16()? as usize;
    let vertex_count = r.u16()? as usize;

    let mut vertices = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        let position = r.vec3()?;
        let mut attributes = [0.0; 8];
        for w in &mut attributes {
            *w = r.f32()?;
        }
        vertices.push(SubdVertex { position, attributes });
    }

    let mut edges = Vec::with_capacity(edge_count);
    for _ in 0..edge_count {
        let edge = [r.u16()?, r.u16()?];
        if edge.iter().any(|&v| v as usize >= vertex_count) {
            return Err(Error::invalid(format!("edge {edge:?} out of range ({vertex_count} vertices)")));
        }
        edges.push(edge);
    }

    let mut faces = Vec::with_capacity(face_count);
    for _ in 0..face_count {
        let unk_a = r.u8()?;
        let n = r.u8()? as usize;
        let reversed_mask = r.u8()?;
        let face_edges = (0..n).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
        if face_edges.iter().any(|&e| e as usize >= edge_count) {
            return Err(Error::invalid(format!("face edge out of range ({edge_count} edges)")));
        }
        faces.push(SubdFace { unk_a, reversed_mask, edges: face_edges });
    }

    let flag_count = r.u32()? as usize;
    if flag_count != face_count {
        return Err(Error::invalid(format!("face flag count {flag_count} != face count {face_count}")));
    }
    let unk_face_flags = r.bytes(flag_count)?.to_vec();
    let unk_tail = [r.u16()?, r.u16()?];

    Ok(SubdMesh { version, unk_header, vertices, edges, faces, unk_face_flags, unk_tail })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(index: u16, children: [u16; 2]) -> SubdNode {
        SubdNode { center: [0.0; 3], half_extents: [0.0; 3], index, leaf_count: 1, children }
    }

    fn file(nodes: Vec<SubdNode>, leaf_mesh: Vec<u16>) -> SubdFile {
        SubdFile { nodes, leaf_mesh, meshes: Vec::new() }
    }

    #[test]
    fn tree_order_is_pre_order_first_child_first() {
        let f = file(
            vec![node(0, [1, 2]), node(9, [3, 4]), node(1, [0, 0]), node(2, [0, 0]), node(0, [0, 0])],
            vec![7, 5, 6],
        );
        assert_eq!(f.meshes_in_tree_order().unwrap(), [6, 7, 5]);
        let f = file(vec![node(0, [0, 1]), node(1, [0, 0])], vec![3, 4]);
        assert_eq!(f.meshes_in_tree_order().unwrap(), [4]);
        let f = file(vec![node(1, [1, 2]), node(0, [0, 0]), node(1, [0, 0])], vec![9, 8, 7]);
        assert_eq!(f.meshes_in_tree_order().unwrap(), [8, 7]);
        assert!(file(Vec::new(), Vec::new()).meshes_in_tree_order().unwrap().is_empty());
    }

    #[test]
    fn tree_order_rejects_loops_and_bad_indices() {
        assert!(file(vec![node(0, [1, 0]), node(0, [1, 0])], vec![0]).meshes_in_tree_order().is_err());
        assert!(file(vec![node(0, [5, 0])], vec![0]).meshes_in_tree_order().is_err());
        assert!(file(vec![node(0, [1, 0]), node(3, [0, 0])], vec![0]).meshes_in_tree_order().is_err());
    }
}
