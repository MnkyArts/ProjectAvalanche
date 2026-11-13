mod boundary;
mod build;
mod encode;
pub mod heli;
pub mod template;
mod toml;
mod volume;

#[cfg(test)]
mod tests;

pub use boundary::{BoundaryRaster, MAX_BOUNDARY_SPANS, OUT_OF_BOUNDS, raster as boundary_raster};
pub use build::{build, read_map};
pub use encode::sim_hash;
pub use toml::{Intro, MAX_TIME_LIMIT_S, MAX_TITLE_CHARS, MapToml, Mode, RawMap, RawMapTable, from_table, parse_toml};
pub use volume::{Volume, VolumeShape};

use crate::id::ContentId;
use crate::model::CollisionTriangle;

pub const MAP_FILE: &str = "map.toml";

pub const MAX_SPAWNS: usize = 10;
pub const MAX_CHECKPOINTS: usize = 50;
pub const MAX_VOLUMES: usize = 2048;
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_MARKERS: usize = 1024;
pub const MAX_ORDER: u32 = 256;
pub const HELI_PATH_POINTS: usize = 2;
pub const HELI_LOCATORS: [&str; HELI_PATH_POINTS] = ["nis_intro_locator", "nis_outro_locator"];
pub const MAX_ENTITIES: usize = 2048;
pub const MAX_BODIES: usize = 256;
pub const MAX_CAMERAS: usize = 64;
pub const MAX_DLC_COLLISION_TRIANGLES: usize = 1_875_056;
pub const MAX_COLLISION_TRIANGLES: usize = 1_824_266;
pub const MAX_EXTRAS_BYTES: usize = 4096;
pub const MAX_COORDINATE: f32 = 131_072.0;
pub const BOUNDARY_CELL: f32 = 10.0;
pub const MAX_BOUNDARY_CELLS: usize = 1 << 18;
pub const MAX_AO: f32 = 4.0;

pub fn scene_limits() -> crate::model::Limits {
    crate::model::Limits { max_coordinate: MAX_COORDINATE, ..crate::model::Limits::default() }
}

pub const TERRAIN_SURFACES: [(&str, [f32; 8]); 8] = [
    ("powder", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0]),
    ("windswept", [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0]),
    ("crusty", [0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0]),
    ("wet", [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0]),
    ("groomed", [0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]),
    ("crud", [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0]),
    ("rock", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
    ("ice", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]),
];

pub const RAIL_GROUP: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    pub position: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMesh {
    pub node: String,
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub weights: Vec<[f32; 8]>,
    pub material: Option<ContentId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollisionMesh {
    pub node: String,
    pub triangles: Vec<CollisionTriangle>,
    pub hidden: bool,
    pub material: Option<ContentId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropLook {
    Mesh { vertices: usize, triangles: usize },
    Object(ContentId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub node: String,
    pub look: PropLook,
    pub placement: Placement,
    pub ao: f32,
    pub gltf_node: usize,
    pub material: Option<ContentId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub node: String,
    pub slot: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeliPoint {
    pub node: String,
    pub order: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarkerPlacement {
    pub node: String,
    pub marker: ContentId,
    pub placement: Placement,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraDef {
    pub node: String,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub yfov: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryArea {
    pub triangles: Vec<[[f32; 2]; 3]>,
    pub raster: BoundaryRaster,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapDefinition {
    pub terrain: Vec<TerrainMesh>,
    pub collision: Vec<CollisionMesh>,
    pub rails: Vec<CollisionMesh>,
    pub props: Vec<Prop>,
    pub spawns: Vec<Spawn>,
    pub checkpoints: Vec<Volume>,
    pub finish: Option<Volume>,
    pub triggers: Vec<Volume>,
    pub deaths: Vec<Volume>,
    pub boundary: Option<BoundaryArea>,
    pub markers: Vec<MarkerPlacement>,
    pub heli_path: Vec<HeliPoint>,
    pub cameras: Vec<CameraDef>,
    pub notes: Vec<String>,
}

impl MapDefinition {
    pub fn collision_triangles(&self) -> usize {
        self.terrain.iter().map(|t| t.triangles.len()).sum::<usize>()
            + self.collision.iter().chain(&self.rails).map(|c| c.triangles.len()).sum::<usize>()
    }

    pub fn ids(&self) -> Vec<ContentId> {
        let objects = self.props.iter().filter_map(|p| match &p.look {
            PropLook::Object(id) => Some(id.clone()),
            PropLook::Mesh { .. } => None,
        });
        let materials = self.terrain.iter().filter_map(|t| t.material.clone());
        let materials = materials.chain(self.props.iter().filter_map(|p| p.material.clone()));
        let materials = materials.chain(self.rails.iter().filter_map(|r| r.material.clone()));
        objects.chain(self.markers.iter().map(|m| m.marker.clone())).chain(materials).collect()
    }

    pub fn volume_count(&self) -> usize {
        self.checkpoints.len() + self.triggers.len() + self.deaths.len() + usize::from(self.finish.is_some())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub toml: MapToml,
    pub definition: MapDefinition,
    pub props: Option<PropMeshes>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropMeshes {
    pub model: std::sync::Arc<crate::model::Model>,
    pub ranges: Vec<std::ops::Range<usize>>,
    pub cost: crate::model::ModelCost,
}

pub const DEFAULT_TERRAIN_MATERIAL: &str = "ssx:material/terrain_default_mat";
pub const DEFAULT_RAIL_MATERIAL: &str = "ssx:material/terrain_rail_red_mat";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "{}",
    ssx_core::text::Text::new(format_args!(
        "{}: {}{}",
        ssx_core::text::shown_max(.file, ssx_core::text::PATH_CHARS),
        ssx_core::text::shown_max(.at, ssx_core::text::PATH_CHARS),
        .message
    ))
)]
pub struct MapError {
    pub file: String,
    pub at: String,
    pub message: String,
}

impl MapError {
    pub fn new(file: &str, at: impl Into<String>, message: impl Into<String>) -> MapError {
        let at = at.into();
        let at = if at.is_empty() { at } else { format!("{at}: ") };
        MapError { file: file.to_string(), at, message: message.into() }
    }
}
