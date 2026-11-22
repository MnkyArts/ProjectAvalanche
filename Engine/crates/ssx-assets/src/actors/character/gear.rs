use super::*;

pub enum SlotParts {
    Equipped(Vec<CharacterPart>, Vec<GearLightNode>),
    Cleared,
    Default,
}

pub type EquippedSlots = Vec<(String, SlotParts)>;

pub fn equipped_parts(
    big_root: &Path,
    track: Option<&str>,
    name: &str,
    loadout: &Loadout,
    rig: &SceneGraph,
) -> Result<(EquippedSlots, Vec<String>)> {
    let equipment = equipment_refs(big_root, name, loadout)?;
    let mut problems = Vec::new();
    let mut walked: Vec<(String, Option<Outfit>)> = Vec::new();
    crate::vault_db::with_level_db(big_root, track, |db| {
        let after: Vec<(String, SlotAfterLoadout)> = GEAR_SLOTS
            .iter()
            .map(|g| (g.field.to_lowercase(), slot_after_loadout(g, equipment.refs.get(&g.item).copied())))
            .collect();
        let mut copies = Outfit::new(String::new());
        if let Some(id) = equipment.board_id {
            let roots: Vec<(u32, u32)> = after
                .iter()
                .filter_map(|(_, a)| if let SlotAfterLoadout::Equipped(c, k) = *a { Some((c, k)) } else { None })
                .collect();
            override_board_id(db, &roots, id, &mut copies)?;
            problems.append(&mut copies.left_out);
        }
        for (slot, a) in after {
            match a {
                SlotAfterLoadout::Equipped(c, k) => {
                    let mut out = Outfit::new(String::new());
                    out.overrides = copies.overrides.clone();
                    collect(db, &slot, c, k, &mut out)?;
                    walked.push((slot, Some(out)));
                }
                SlotAfterLoadout::Cleared => walked.push((slot, None)),
                SlotAfterLoadout::Default => {}
            }
        }
        Ok(())
    })?;
    let mut out = Vec::new();
    for g in &GEAR_SLOTS {
        let slot = g.field.to_lowercase();
        let Some((_, w)) = walked.iter_mut().find(|(s, _)| *s == slot) else {
            out.push((slot, SlotParts::Default));
            continue;
        };
        let Some(o) = w.take() else {
            out.push((slot, SlotParts::Cleared));
            continue;
        };
        let mut loaded = Vec::new();
        for m in o.meshes {
            match load_mesh_files(big_root, &m) {
                Ok((geom, crsf, mesh, file)) => loaded.push((m, geom, crsf, mesh, file)),
                Err(e) => problems.push(format!("{} ({}): {e:#}", m.mesh_name, m.slot)),
            }
        }
        let (parts, _) = disc_parts(rig, loaded, &mut problems)?;
        let mut lights = o.light_nodes;
        if equipment.lamp_accessory && slot == "headlamp" {
            scale_head_lamp(&mut lights, loadout.head_lamp_stats);
        }
        out.push((slot, SlotParts::Equipped(parts, lights)));
    }
    Ok((out, problems))
}

pub(super) fn scale_head_lamp(lights: &mut [GearLightNode], stats: [Option<u8>; 2]) {
    let (cone, length) = head_lamp_scale(stats);
    for g in lights.iter_mut().filter(|g| g.slot == "headlamp") {
        if let Some(crate::track_lights::SceneLight {
            shape: crate::track_lights::LightShape::Spot { cone_angle, penumbra_angle, outer_length, inner_length, .. },
            ..
        }) = g.node.light.as_mut()
        {
            (*cone_angle, *penumbra_angle) = (*cone_angle * cone, *penumbra_angle * cone);
            (*outer_length, *inner_length) = (*outer_length * length, *inner_length * length);
        }
    }
}

pub fn mesh_part(
    big_root: &Path,
    track: Option<&str>,
    mesh_name: &str,
    slot: &str,
    rig: &SceneGraph,
) -> Result<CharacterPart> {
    let m = crate::vault_db::with_level_db(big_root, track, |db| {
        mesh_ref(db, crate::vault_db::FieldOverrides::none(), slot, fnv1(mesh_name))
    })?;
    let (geom, crsf, mesh, file) = load_mesh_files(big_root, &m)?;
    let mut problems = Vec::new();
    let (mut parts, _) = disc_parts(rig, vec![(m, geom, crsf, mesh, file)], &mut problems)?;
    match parts.pop() {
        Some(p) => Ok(p),
        None => bail!("{mesh_name}: {}", problems.join("; ")),
    }
}
