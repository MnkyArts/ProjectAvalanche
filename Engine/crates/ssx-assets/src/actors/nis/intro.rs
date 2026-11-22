use super::*;

pub const TUTORIAL_END_BINDING: &str = "nis_binding_tutorial_end_test";

pub fn load_sequence_nis(big_root: &Path, binding: &str, character: &str) -> Result<IntroNis> {
    let bind = Binding::load(big_root, binding, character)?;
    let db = &bind.db;
    let walk = bind.walk(HashMap::new())?;
    let b = db.by_name(binding).context("binding")?;
    let seq = db.get(b.guid("ControllerAsset").context("binding without controller")?).context("controller")?;
    anyhow::ensure!(seq.is("SequenceContainerAsset"), "{binding}: {} is no sequence container", seq.class());
    let scene = walk.sequence_scene(&seq)?;
    let controller = seq.name().to_string();
    let coi = for_character_of_interest(b.value("CharacterOfInterestName").and_then(|v| v.as_str()), &bind.character);
    let requires_wingsuit = b.bool("RequiresWingsuit").unwrap_or(false);
    let entry_blend = bind.entry_blend()?;
    let Binding { bundles, rigs: [rider, heli, camera], .. } = bind;
    Ok(IntroNis {
        binding: binding.to_string(),
        controller,
        ready: scene.clone(),
        jump: scene,
        transition_blend: Blend::cut(),
        entry_blend,
        branch_out: None,
        bundles,
        rider,
        heli,
        camera,
        cabin: Vec::new(),
        requires_wingsuit,
        character_of_interest: coi,
        ready_loop: false,
        jump_start: 0.0,
    })
}

pub const PILOT_BANK: &str = "PTR_proxybank_nis_intro_pilot";

pub(super) struct Binding {
    pub(super) db: AssetDb,
    pub(super) bundles: Vec<String>,
    pub(super) rigs: [Rig; 3],
    banks: HashMap<String, Guid>,
    per_actor: HashMap<i64, Guid>,
    roles: Vec<i64>,
    character: String,
}

impl Binding {
    pub(super) fn entry_blend(&self) -> Result<Blend> {
        let mut banks = self.banks.clone();
        if let Some(&bank) = self.per_actor.get(&ROLE_CHARACTER1) {
            banks.insert(PER_ACTOR_BANK.to_string(), bank);
        }
        crate::anim_graph::Graph::transition_blend(&self.db, GAMEPLAY_TO_NIS, banks)
    }

    pub(super) fn load(big_root: &Path, binding: &str, character: &str) -> Result<Self> {
        Self::load_with(big_root, binding, character, &[])
    }

    pub(super) fn load_with(big_root: &Path, binding: &str, character: &str, extra: &[&str]) -> Result<Self> {
        Self::load_full(big_root, binding, character, None, extra)
    }

    fn load_full(big_root: &Path, binding: &str, character: &str, heli: Option<&str>, extra: &[&str]) -> Result<Self> {
        let mut bundles = intro_bundles(big_root, binding, character);
        for (i, e) in extra.iter().enumerate() {
            bundles.insert(1 + i, e.to_string());
        }
        let load = |b: &[String]| AssetDb::load(big_root, &b.iter().map(String::as_str).collect::<Vec<_>>());
        let mut db = load(&bundles)?;
        let bank = |db: &AssetDb, pointer: &str| {
            let b = db.by_name(binding)?;
            let desc = b.guid("ProxyBankDescriptor").and_then(|g| db.get(g));
            desc.map(|d| d.guids("Banks")).unwrap_or_default().into_iter().filter_map(|g| db.get(g)).find_map(|e| {
                let ptr = e.guid("NisBankPointerAsset").and_then(|g| db.get(g))?;
                let fmt = e.value("FormatString")?.as_str()?;
                (ptr.name() == pointer).then(|| fmt.replace("%p", character).replace("%h", heli.unwrap_or("")))
            })
        };
        db.by_name(binding).with_context(|| format!("no NisBindingAsset {binding}"))?;
        let player_bank = bank(&db, "PTR_proxybank_nis_player");
        let pilot_bank = heli.and_then(|_| bank(&db, PILOT_BANK));
        let mut reload = false;
        for p in [&player_bank, &pilot_bank].into_iter().flatten() {
            if crate::rig_anim::has_bundle(big_root, &format!("{p}.cba")) {
                bundles.insert(1, format!("{p}.cba"));
                reload = true;
            }
        }
        if reload {
            db = load(&bundles)?;
        }
        let all = &db.streams.iter().find(|(n, _)| n == RIG_BUNDLE).context("no anim_all.cba")?.1;
        let rigs = [Rig::load(all, RIDER_RIG)?, Rig::load(all, HELI_RIG)?, Rig::load(all, CAMERA_RIG)?];
        let b = db.by_name(binding).context("binding")?;
        let mut banks = HashMap::new();
        if let Some(g) = b.guid("AllActorProxyBank") {
            banks.insert("PTR_proxybank_nis_all_actors".to_string(), g);
        }
        if let Some(g) = player_bank.as_deref().and_then(|p| db.by_name(p)).map(|a| a.guid) {
            banks.insert("PTR_proxybank_nis_player".to_string(), g);
        }
        if let Some(g) = pilot_bank.as_deref().and_then(|p| db.by_name(p)).map(|a| a.guid) {
            banks.insert(PILOT_BANK.to_string(), g);
        }
        let (mut per_actor, mut roles) = (HashMap::new(), Vec::new());
        for item in b.guids("Bindings").into_iter().filter_map(|g| db.get(g)) {
            let Some(role) = enum_value(&db, &item, "RoleId") else { continue };
            roles.push(role);
            if let Some(bank) = item.guid("ProxyBank") {
                per_actor.insert(role, bank);
            }
        }
        roles.sort_unstable();
        Ok(Self { db, bundles, rigs, banks, per_actor, roles, character: character.to_string() })
    }

    pub(super) fn walk(&self, enums: HashMap<String, i64>) -> Result<Walk<'_>> {
        let all = &self.db.streams.iter().find(|(n, _)| n == RIG_BUNDLE).context("no anim_all.cba")?.1;
        Ok(Walk {
            db: &self.db,
            all,
            banks: self.banks.clone(),
            per_actor: self.per_actor.clone(),
            rigs: [&self.rigs[0], &self.rigs[1], &self.rigs[2]],
            roles: self.roles.clone(),
            enums,
            reverse_camera: false,
        })
    }
}

pub fn load_intro(big_root: &Path, binding: &str, character: &str, snow: bool) -> Result<IntroNis> {
    intro_of_binding(Binding::load(big_root, binding, character)?, binding, snow, IntroPath::Short, false)
}

pub fn load_drop_intro(
    big_root: &Path,
    binding: &str,
    character: &str,
    heli: Option<&str>,
    snow: bool,
    path: IntroPath,
    reverse_camera: bool,
) -> Result<IntroNis> {
    match (heli, path) {
        (Some(h), IntroPath::Long | IntroPath::Medium) => intro_of_binding(
            Binding::load_full(big_root, binding, character, Some(h), &[])?,
            binding,
            snow,
            path,
            reverse_camera,
        ),
        _ => intro_of_binding(
            Binding::load(big_root, binding, character)?,
            binding,
            snow,
            IntroPath::Short,
            reverse_camera,
        ),
    }
}

pub fn load_intro_path(
    big_root: &Path,
    binding: &str,
    character: &str,
    heli: &str,
    snow: bool,
    path: IntroPath,
) -> Result<IntroNis> {
    intro_of_binding(Binding::load_full(big_root, binding, character, Some(heli), &[])?, binding, snow, path, false)
}

fn intro_of_binding(
    bind: Binding,
    binding: &str,
    snow: bool,
    path: IntroPath,
    reverse_camera: bool,
) -> Result<IntroNis> {
    let db = &bind.db;
    let walk = Walk { reverse_camera, ..bind.walk(HashMap::new())? };
    let b = db.by_name(binding).context("binding")?;
    let sf = db.get(b.guid("ControllerAsset").context("binding without controller")?).context("state flow")?;
    let coi = for_character_of_interest(b.value("CharacterOfInterestName").and_then(|v| v.as_str()), &bind.character);
    let (cabin, end) = cabin_chain(&walk, &sf, path, snow, coi)?;
    let go_of = |ready: &Asset| {
        sf.guids("StateFlowTransitionAssets")
            .into_iter()
            .filter_map(|g| db.get(g))
            .find(|t| {
                same(t.guid("From"), ready.guid)
                    && walk.names(t, "ConditionsRequiredTrue").iter().any(|s| s == "BLN_NisTriggerTransition")
            })
            .with_context(|| format!("{binding}: no go transition from {}", ready.name()))
            .and_then(|t| Ok((t.guid("To"), walk.transition_blend(&t, None)?)))
    };
    let (ready, jump, blend, ready_loop, jump_start) = if end.ready_loop {
        if end.start != 0.0 {
            bail!("{binding}: the ready loop {} entered at tick {}", end.node.name(), end.start);
        }
        let (to, blend) = go_of(&end.node)?;
        let jump = db.get(to.context("transition without target")?).context("go node")?;
        (end.node, jump, blend, true, 0.0)
    } else {
        let nodes: Vec<Asset> = sf.guids("StateFlowNodeAssets").into_iter().filter_map(|g| db.get(g)).collect();
        let ready = nodes
            .into_iter()
            .find(|n| walk.names(n, "ActiveSignals").iter().any(|s| s == "BLN_IsNisReadyLoop"))
            .with_context(|| format!("{binding}: no ready-loop node"))?;
        (ready, end.node, end.blend.clone(), false, end.start)
    };
    let (ready, jump) = (walk.scene(&ready)?, walk.scene(&jump)?);
    if jump_start > 0.0 && camera_layered(&jump) {
        bail!(
            "{binding}: the last scene {} has camera layers and is entered at tick {jump_start} (not ported)",
            jump.node
        );
    }
    let controller = sf.name().to_string();
    let requires_wingsuit = b.bool("RequiresWingsuit").unwrap_or(false);
    let entry_blend = bind.entry_blend()?;
    let Binding { bundles, rigs: [rider, heli, camera], .. } = bind;
    Ok(IntroNis {
        binding: binding.to_string(),
        controller,
        ready,
        jump,
        transition_blend: blend,
        entry_blend,
        branch_out: None,
        bundles,
        rider,
        heli,
        camera,
        cabin,
        requires_wingsuit,
        character_of_interest: coi,
        ready_loop,
        jump_start,
    })
}

fn camera_layered(sc: &NisScene) -> bool {
    sc.actor(ROLE_CAMERA1).is_some_and(|a| !a.layers.is_empty())
}

pub fn for_character_of_interest(name: Option<&str>, character: &str) -> bool {
    name.unwrap_or("") == character
}

struct ChainEnd<'a> {
    node: Asset<'a>,
    ready_loop: bool,
    start: f32,
    blend: Blend,
}

fn cabin_chain<'a>(
    walk: &Walk<'a>,
    sf: &Asset<'a>,
    path: IntroPath,
    snow: bool,
    coi: bool,
) -> Result<(Vec<CabinScene>, ChainEnd<'a>)> {
    let db = walk.db;
    let holds_with = |name: &str, skip: bool| match name {
        "BLN_NisShort" => path == IntroPath::Short,
        "BLN_NisMedium" => path == IntroPath::Medium,
        "BLN_NisSnowStart" => snow,
        "BLN_IsNisForCharacterOfInterest" => coi,
        "BLN_NisTriggerSkip" => skip,
        _ => false,
    };
    let holds_if = |a: &Asset, t: &str, f: &str, skip: bool| {
        walk.names(a, t).iter().all(|n| holds_with(n, skip)) && walk.names(a, f).iter().all(|n| !holds_with(n, skip))
    };
    let conditions_hold = |a: &Asset, t: &str, f: &str| holds_if(a, t, f, false);
    let passes = |t: &Asset, skip: bool| -> Option<Asset<'a>> {
        let to = t.guid("To").and_then(|g| db.get(g))?;
        (walk.name(t, "BranchWindowTypeAsset").is_none()
            && holds_if(t, "ConditionsRequiredTrue", "ConditionsRequiredFalse", skip)
            && holds_if(&to, "EntryConditionsRequiredTrue", "EntryConditionsRequiredFalse", skip))
        .then_some(to)
    };
    let skip_of = |node: &Asset<'a>| -> Result<Option<(Asset<'a>, Blend, bool)>> {
        let Some((t, mut to)) = node
            .guids("Transitions")
            .into_iter()
            .filter_map(|g| db.get(g))
            .find_map(|t| passes(&t, true).filter(|_| passes(&t, false).is_none()).map(|to| (t, to)))
        else {
            return Ok(None);
        };
        let mut blend = walk.transition_blend(&t, None)?;
        let mut branch_in = walk.name(&t, "BranchInWindowTypeAsset").as_deref() == Some("NIS_In");
        for _ in 0..8 {
            if !to.bool("IsConduit").unwrap_or(false) {
                return Ok(Some((to, blend, branch_in)));
            }
            let (t2, next) = to
                .guids("Transitions")
                .into_iter()
                .filter_map(|g| db.get(g))
                .find_map(|t| passes(&t, true).map(|n| (t, n)))
                .with_context(|| format!("{}: no transition holds on the skip", to.name()))?;
            branch_in = walk.name(&t2, "BranchInWindowTypeAsset").as_deref() == Some("NIS_In");
            blend = walk.transition_blend(&t2, None)?;
            to = next;
        }
        bail!("{}: the skip passes more than 8 conduits", node.name())
    };
    let mut node = db.get(sf.guid("StartNode").context("no start node")?).context("start node")?;
    let (mut out, mut start, mut blend_in) = (Vec::new(), 0.0, Blend::cut());
    let mut nodes: Vec<Asset<'a>> = Vec::new();
    let resolve = |mut out: Vec<CabinScene>, nodes: &[Asset<'a>], end: ChainEnd<'a>| -> Result<_> {
        for (i, n) in nodes.iter().enumerate() {
            let Some((to, blend, branch_in)) = skip_of(n)? else { continue };
            let (target, start) = if to.guid == end.node.guid {
                (if end.ready_loop { SkipTo::ReadyLoop } else { SkipTo::Jump }, end.start)
            } else if let Some(k) = nodes.iter().position(|m| m.guid == to.guid) {
                (SkipTo::Cabin(k), out[k].start)
            } else {
                bail!("{}: the skip leads to {}, not on the way", n.name(), to.name());
            };
            let entry = match branch_in {
                true => walk.scene(&to)?.actors.iter().filter_map(|a| a.tags.branch_in).reduce(f32::min).unwrap_or(0.0),
                false => 0.0,
            };
            if entry != start {
                bail!("{}: the skip enters {} at tick {entry}, the way at {start}", n.name(), to.name());
            }
            out[i].skip = Some((target, blend));
        }
        Ok((out, end))
    };
    for _ in 0..24 {
        let conduit = node.bool("IsConduit").unwrap_or(false);
        if !conduit && node.guids("Transitions").is_empty() {
            return resolve(out, &nodes, ChainEnd { node, ready_loop: false, start, blend: blend_in });
        }
        let scene = if conduit { None } else { Some(walk.scene(&node)?) };
        let window_time = |kind: &str| -> Option<f32> {
            let sc = scene.as_ref()?;
            sc.actors
                .iter()
                .filter_map(|a| if kind == "NIS_Out" { a.tags.branch_out } else { a.tags.early_out })
                .reduce(f32::min)
        };
        let mut best: Option<(f32, Asset, String)> = None;
        for t in node.guids("Transitions").into_iter().filter_map(|g| db.get(g)) {
            let Some(to) = t.guid("To").and_then(|g| db.get(g)) else { continue };
            if !conditions_hold(&t, "ConditionsRequiredTrue", "ConditionsRequiredFalse")
                || !conditions_hold(&to, "EntryConditionsRequiredTrue", "EntryConditionsRequiredFalse")
            {
                continue;
            }
            let kind = walk.name(&t, "BranchWindowTypeAsset").unwrap_or_default();
            let time = if kind.is_empty() { Some(0.0) } else { window_time(&kind) };
            if let Some(time) = time
                && best.as_ref().is_none_or(|b| time < b.0)
            {
                best = Some((time, t, kind));
                if time <= 0.0 {
                    break;
                }
            }
        }
        let (end, t, window) = best.with_context(|| format!("{}: no transition holds", node.name()))?;
        let next = db.get(t.guid("To").context("transition without target")?).context("target node")?;
        let blend = walk.transition_blend(&t, scene.as_ref())?;
        if let Some(sc) = scene {
            if start > 0.0 && camera_layered(&sc) {
                bail!("{}: a scene with camera layers entered at tick {start} (not ported)", node.name());
            }
            let tuning_offset = sc.node_tags.tuning_offset;
            out.push(CabinScene { tuning_offset, scene: sc, start, end, window, blend: blend.clone(), skip: None });
            nodes.push(node);
        }
        start = 0.0;
        if walk.name(&t, "BranchInWindowTypeAsset").as_deref() == Some("NIS_In") {
            let sc = walk.scene(&next)?;
            start = sc.actors.iter().filter_map(|a| a.tags.branch_in).reduce(f32::min).unwrap_or(0.0);
        }
        if walk.names(&next, "ActiveSignals").iter().any(|s| s == "BLN_IsNisReadyLoop") {
            return resolve(out, &nodes, ChainEnd { node: next, ready_loop: true, start, blend });
        }
        (node, blend_in) = (next, blend);
    }
    bail!("{}: no ready loop or last scene within 24 nodes", sf.name())
}
