use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ssx_formats::subd::{SubdFile, SubdMesh};

#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub attributes: Vec<[f32; 8]>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn append(&mut self, other: &TriMesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.attributes.extend_from_slice(&other.attributes);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let first = *self.positions.first()?;
        Some(self.positions.iter().fold((first, first), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        }))
    }
}

pub struct TerrainPiece {
    pub path: PathBuf,
    pub subd: SubdFile,
}

pub fn load_track(dir: &Path) -> Result<Vec<TerrainPiece>> {
    load_track_files(dir, None)
}

pub fn track_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_mesh_subd.subd")))
        .collect();
    paths.sort();
    Ok(paths)
}

pub fn track_file_names(dir: &Path) -> Result<Vec<String>> {
    Ok(track_files(dir)?.iter().map(|p| file_name(p)).collect())
}

pub fn select_drop_files<'a, S: AsRef<str>>(all: &'a [S], drop: Option<&[String]>) -> Vec<&'a str> {
    match drop {
        Some(drop) => {
            all.iter().map(AsRef::as_ref).filter(|n| drop.iter().any(|f| f.eq_ignore_ascii_case(n))).collect()
        }
        None => all.iter().map(AsRef::as_ref).collect(),
    }
}

pub fn load_track_files(dir: &Path, drop: Option<&[String]>) -> Result<Vec<TerrainPiece>> {
    let paths = track_files(dir)?;
    let names: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
    let chosen = select_drop_files(&names, drop);
    paths
        .into_iter()
        .zip(&names)
        .filter(|(_, n)| chosen.contains(&n.as_str()))
        .map(|(path, _)| {
            let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let subd = SubdFile::parse(&data).with_context(|| format!("parsing {}", path.display()))?;
            Ok(TerrainPiece { path, subd })
        })
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn cage_trimesh(mesh: &SubdMesh) -> TriMesh {
    let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    let mut indices = Vec::with_capacity(mesh.faces.len() * 6);

    for face in mesh.faces.iter().filter(|f| !is_hidden_face(f.unk_a)) {
        let loop_: Vec<u32> = mesh.face_vertices(face).map(u32::from).collect();
        for k in 1..loop_.len().saturating_sub(1) {
            let tri = [loop_[0], loop_[k], loop_[k + 1]];
            let n = cross(
                sub(positions[tri[1] as usize], positions[tri[0] as usize]),
                sub(positions[tri[2] as usize], positions[tri[0] as usize]),
            );
            for &i in &tri {
                let acc = &mut normals[i as usize];
                *acc = [acc[0] + n[0], acc[1] + n[1], acc[2] + n[2]];
            }
            indices.extend_from_slice(&tri);
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        *n = if len > 0.0 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 1.0, 0.0] };
    }
    let attributes = mesh
        .vertices
        .iter()
        .map(|v| {
            let mut a = v.attributes;
            crate::terrain_vertex::normalise_layer_weights(&mut a);
            a
        })
        .collect();
    TriMesh { positions, normals, attributes, indices }
}

pub fn file_trimesh(subd: &SubdFile) -> TriMesh {
    let mut out = TriMesh::default();
    for mesh in &subd.meshes {
        out.append(&cage_trimesh(mesh));
    }
    out
}

pub fn is_hidden_face(unk_a: u8) -> bool {
    unk_a & 1 != 0
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[cfg(test)]
mod tests {
    use super::select_drop_files;

    const ALL: [&str; 3] = ["a_mesh_subd.subd", "B_mesh_subd.subd", "c_mesh_subd.subd"];

    fn drop(files: &[&str]) -> Vec<String> {
        files.iter().map(|f| (*f).to_owned()).collect()
    }

    #[test]
    fn matches_case_insensitively() {
        let d = drop(&["A_MESH_SUBD.SUBD", "b_mesh_subd.subd"]);
        assert_eq!(select_drop_files(&ALL, Some(&d)), ["a_mesh_subd.subd", "B_mesh_subd.subd"]);
    }

    #[test]
    fn selects_only_the_drop_subset() {
        let d = drop(&["c_mesh_subd.subd", "missing_mesh_subd.subd"]);
        assert_eq!(select_drop_files(&ALL, Some(&d)), ["c_mesh_subd.subd"]);
    }

    #[test]
    fn a_drop_that_names_nothing_draws_nothing() {
        assert_eq!(select_drop_files(&ALL, Some(&[])), Vec::<&str>::new());
        let d = drop(&["other_mesh_subd.subd"]);
        assert_eq!(select_drop_files(&ALL, Some(&d)), Vec::<&str>::new());
    }

    #[test]
    fn no_drop_keeps_every_file() {
        assert_eq!(select_drop_files(&ALL, None), ALL);
    }
}
