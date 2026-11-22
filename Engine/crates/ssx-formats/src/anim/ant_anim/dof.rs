use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelToDof {
    pub name: String,
    pub storage_type: u32,
    pub indices: Vec<u16>,
}

impl ChannelToDof {
    pub const CLASS: &'static str = "ChannelToDofAsset";

    pub fn parse(schema: &Schema, obj: &Object) -> Result<Self> {
        let fl = Fields::new(schema, obj);
        let (storage_type, data) = (fl.u32("StorageType")?, fl.bytes("IndexData")?);
        let indices = match storage_type {
            1 => data.iter().map(|&b| b.into()).collect(),
            2 if data.len() % 2 == 0 => data.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect(),
            _ => return invalid(Self::CLASS, format!("storage type {storage_type} with {} bytes", data.len())),
        };
        Ok(ChannelToDof { name: obj.name(schema).unwrap_or_default().to_string(), storage_type, indices })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DofType {
    Quaternion,
    Translation,
    Scale,
    Vector3,
    Float,
    Other(u32),
}

impl DofType {
    pub fn from_id(id: u32) -> Self {
        match id {
            14 => DofType::Quaternion,
            0x7A2E_5497 => DofType::Translation,
            0x7A2E_53C6 => DofType::Scale,
            12 => DofType::Vector3,
            10 => DofType::Float,
            x => DofType::Other(x),
        }
    }

    pub fn accepts(self, kind: usize) -> bool {
        matches!(
            (kind, self),
            (0, DofType::Quaternion)
                | (1, DofType::Translation | DofType::Scale | DofType::Vector3)
                | (2, DofType::Float)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dof {
    pub layout: String,
    pub name: String,
    pub ty: DofType,
}

impl Dof {
    pub fn joint(&self) -> Option<(&str, char)> {
        let (joint, suffix) = self.name.rsplit_once('.')?;
        matches!(suffix, "q" | "t" | "s").then(|| (joint, suffix.chars().next().unwrap_or('q')))
    }
}

pub const DELTA_TRAJ_DOFS: [(&str, DofType); 8] = [
    ("deltaRotation", DofType::Quaternion),
    ("angularVelocity", DofType::Quaternion),
    ("angularAcceleration", DofType::Quaternion),
    ("deltaTranslation", DofType::Translation),
    ("velocity", DofType::Translation),
    ("acceleration", DofType::Translation),
    ("lastTranslation", DofType::Translation),
    ("lastRotation", DofType::Quaternion),
];

pub fn dof_list(stream: &Stream, rig: &str, hierarchy: &str) -> Result<Vec<Dof>> {
    let schema = &stream.schema;
    let by_guid = stream.guid_index();
    let find = |g: Guid| by_guid.get(&g).map(|&i| &stream.assets[i].object);
    let guids = |o: &Object, f: &str| -> Vec<Guid> {
        o.get(schema, f)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_guid).collect())
            .unwrap_or_default()
    };
    let rig_obj = stream
        .assets_of_class("RigAsset")
        .map(|a| &a.object)
        .find(|o| o.name(schema) == Some(rig))
        .ok_or_else(|| Error::invalid(format!("no RigAsset {rig}")))?;
    let mut queue: Vec<Guid> = guids(rig_obj, "DofSetLists");
    let mut seen = HashSet::new();
    while let Some(g) = queue.first().copied() {
        queue.remove(0);
        let Some(h) = find(g) else { continue };
        if !seen.insert(g) {
            continue;
        }
        if h.name(schema) != Some(hierarchy) {
            queue.extend(guids(h, "Children"));
            continue;
        }
        let mut out = Vec::new();
        for l in guids(h, "LayoutAssets") {
            let lo = find(l).ok_or_else(|| Error::invalid(format!("layout {l} not in this stream")))?;
            let layout = lo.name(schema).unwrap_or_default().to_string();
            if lo.type_desc(schema).name == "DeltaTrajLayoutAsset" {
                out.extend(DELTA_TRAJ_DOFS.iter().map(|&(n, ty)| Dof {
                    layout: layout.clone(),
                    name: n.to_string(),
                    ty,
                }));
                continue;
            }
            for slot in lo.get(schema, "Slots").and_then(Value::as_array).unwrap_or_default() {
                let s = slot.as_struct().ok_or_else(|| Error::invalid("LayoutEntry"))?;
                let name = s.own(schema, "Name").and_then(Value::as_str).unwrap_or_default().to_string();
                let ty = s.own(schema, "Type").and_then(Value::as_i64).unwrap_or(0) as u32;
                out.push(Dof { layout: layout.clone(), name, ty: DofType::from_id(ty) });
            }
        }
        return Ok(out);
    }
    invalid("RigAsset", format!("{rig} has no layout hierarchy {hierarchy}"))
}

pub fn rig_hierarchies(stream: &Stream, rig: &str) -> Result<HashSet<Guid>> {
    let schema = &stream.schema;
    let by_guid = stream.guid_index();
    let guids = |o: &Object, f: &str| -> Vec<Guid> {
        o.get(schema, f)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_guid).collect())
            .unwrap_or_default()
    };
    let rig_obj = stream
        .assets_of_class("RigAsset")
        .map(|a| &a.object)
        .find(|o| o.name(schema) == Some(rig))
        .ok_or_else(|| Error::invalid(format!("no RigAsset {rig}")))?;
    let mut queue = guids(rig_obj, "DofSetLists");
    let mut out = HashSet::new();
    while let Some(g) = queue.pop() {
        if !out.insert(g) {
            continue;
        }
        if let Some(&i) = by_guid.get(&g) {
            queue.extend(guids(&stream.assets[i].object, "Children"));
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skeleton {
    pub name: String,
    pub joints: Vec<SkeletonJoint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkeletonJoint {
    pub name: String,
    pub parent: Option<usize>,
    pub unk_joint_type: i64,
}

impl Skeleton {
    pub const CLASS: &'static str = "SkeletonAsset";

    pub fn parse(schema: &Schema, obj: &Object) -> Result<Self> {
        let fl = Fields::new(schema, obj);
        let mut joints = Vec::new();
        for (i, j) in fl.array("Joints")?.iter().enumerate() {
            let s = j.as_struct().ok_or_else(|| fl.bad("Joints"))?;
            let get = |f: &str| s.own(schema, f).ok_or_else(|| fl.bad(f));
            let name = get("JointName")?.as_str().ok_or_else(|| fl.bad("JointName"))?.to_string();
            let parent = get("ParentIndex")?.as_i64().ok_or_else(|| fl.bad("ParentIndex"))?;
            let parent = match parent {
                -1 => None,
                p if (0..i as i64).contains(&p) => Some(p as usize),
                p => return invalid(Self::CLASS, format!("joint {i} ({name}) has parent {p}")),
            };
            let unk_joint_type = get("JointType")?.as_i64().ok_or_else(|| fl.bad("JointType"))?;
            joints.push(SkeletonJoint { name, parent, unk_joint_type });
        }
        Ok(Skeleton { name: obj.name(schema).unwrap_or_default().to_string(), joints })
    }

    pub fn of_rig(stream: &Stream, rig: &str) -> Result<Self> {
        let schema = &stream.schema;
        let rig_obj = stream
            .assets_of_class("RigAsset")
            .map(|a| &a.object)
            .find(|o| o.name(schema) == Some(rig))
            .ok_or_else(|| Error::invalid(format!("no RigAsset {rig}")))?;
        let g = rig_obj
            .get(schema, "Skeleton")
            .and_then(Value::as_guid)
            .ok_or_else(|| Error::invalid("RigAsset: Skeleton"))?;
        let i = *stream
            .guid_index()
            .get(&g)
            .ok_or_else(|| Error::invalid(format!("{rig}: skeleton {g} not in this stream")))?;
        Self::parse(schema, &stream.assets[i].object)
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }
}
