use super::*;

fn num(a: &Asset, f: &str) -> f32 {
    a.value(f).and_then(|v| v.as_f32().or_else(|| v.as_i64().map(|x| x as f32))).unwrap_or(0.0)
}

pub(super) fn window(a: &Asset) -> Window {
    Window {
        time: num(a, "Time"),
        duration: num(a, "Duration"),
        always_active: a.bool("AlwaysActive").unwrap_or(false),
    }
}

pub(super) fn vec3_of(a: &Asset, f: &str) -> [f32; 3] {
    match a.value(f) {
        Some(Value::Vector3(v)) => *v,
        Some(Value::Vector4(v)) => [v[0], v[1], v[2]],
        _ => [0.0; 3],
    }
}

pub(super) fn quat_of(a: &Asset, f: &str) -> [f32; 4] {
    match a.value(f) {
        Some(Value::Quaternion(v) | Value::Vector4(v)) => *v,
        _ => [0.0, 0.0, 0.0, 1.0],
    }
}

fn signal_value(a: &Asset, reverse_camera: bool) -> Option<bool> {
    match (a.class(), a.name()) {
        ("BoolScriptExpressionAsset", "BLNX_IsNisReverseCamera") => Some(reverse_camera),
        ("BoolAsset", _) => a.bool("Default"),
        _ => None,
    }
}

pub(super) struct Walk<'a> {
    pub(super) db: &'a AssetDb,
    pub(super) all: &'a Stream,
    pub(super) banks: HashMap<String, Guid>,
    pub(super) per_actor: HashMap<i64, Guid>,
    pub(super) rigs: [&'a Rig; 3],
    pub(super) roles: Vec<i64>,
    pub(super) enums: HashMap<String, i64>,
    pub(super) reverse_camera: bool,
}

pub(super) fn enum_value(db: &AssetDb, a: &Asset, f: &str) -> Option<i64> {
    a.guid(f).and_then(|g| db.get(g)).and_then(|v| v.int("Value"))
}

fn first_animation<'a>(db: &'a AssetDb, a: &Asset<'a>) -> Result<Asset<'a>> {
    let track = a.array("Tracks").first().context("no track")?;
    let anim = field(a.schema, track, "Anims").and_then(Value::as_array).and_then(|v| v.first());
    let g = anim.and_then(|x| field(a.schema, x, "Asset")).and_then(Value::as_guid).context("no animation")?;
    db.get(g).context("animation controller not loaded")
}

impl<'a> Walk<'a> {
    pub(super) fn names(&self, a: &Asset, f: &str) -> Vec<String> {
        a.guids(f).into_iter().filter_map(|g| self.db.get(g)).map(|x| x.name().to_string()).collect()
    }

    pub(super) fn name(&self, a: &Asset, f: &str) -> Option<String> {
        a.guid(f).and_then(|g| self.db.get(g)).map(|x| x.name().to_string())
    }

    pub(super) fn transition_blend(&self, t: &Asset, left: Option<&NisScene>) -> Result<Blend> {
        if let Some(b) = t.guid("OverrideBlend").and_then(|g| self.db.get(g)) {
            return Blend::of_asset(&b).with_context(|| format!("{}: {} is no blend", t.name(), b.name()));
        }
        if let (Some(kind), Some(sc)) = (self.name(t, "BranchWindowTypeAsset"), left)
            && sc.actors.iter().any(|a| a.tags.window_blends.contains(&kind))
        {
            bail!("{}: a {kind} window with its own blend (not ported)", t.name());
        }
        Ok(Blend::default_transition())
    }

    fn proxy_subject(&self, a: &Asset, role: i64) -> Result<Guid> {
        let ptr = a.guid("BankPointerAsset").and_then(|p| self.db.get(p)).map(|p| p.name().to_string());
        let mut next = match ptr.as_deref() {
            Some("PTR_proxybank_nis_per_actor") => self.per_actor.get(&role).copied(),
            Some(p) => self.banks.get(p).copied(),
            None => None,
        };
        for _ in 0..16 {
            let Some(b) = next.and_then(|b| self.db.get(b)) else { break };
            for pair in b.guids("AssetListItems").into_iter().filter_map(|p| self.db.get(p)) {
                if same(pair.guid("ProxyController"), a.guid) {
                    return pair.guid("SubjectController").context("pair without subject");
                }
            }
            next = b.guid("ParentProxyBankAsset");
        }
        a.guid("DefaultControllerAsset").with_context(|| format!("{}: unresolved", a.name()))
    }

    pub(super) fn sequence(&self, g: Guid, loop_end: Option<f32>, depth: usize) -> Result<(Asset<'a>, Option<f32>)> {
        let a = self.db.get(g).with_context(|| format!("controller {g} not loaded"))?;
        anyhow::ensure!(depth < 16, "{}: too deep", a.name());
        match a.class() {
            "SequenceContainerAsset" => Ok((a, loop_end)),
            "LayersControllerAsset" => {
                let first = a.guids("ControllerAssets").first().copied().context("no layer 0")?;
                self.sequence(first, loop_end, depth + 1)
            }
            "LoopingControllerAsset" => {
                let s = a.guid("SubjectControllerAsset").context("Looping without subject")?;
                self.sequence(s, Some(a.f32("LoopEndTime").unwrap_or(0.0)), depth + 1)
            }
            "ProxyControllerAsset" => self.sequence(self.proxy_subject(&a, ROLE_CHARACTER1)?, loop_end, depth + 1),
            c => bail!("{}: {c} is not resolved to a sequence", a.name()),
        }
    }

    fn actor_as(&self, a: &Asset<'a>, role: Option<i64>) -> Result<NisActor> {
        let role = enum_value(self.db, a, "TrackSelectionEnumValue")
            .or(role)
            .with_context(|| format!("{}: no track selection", a.name()))?;
        let ctrl = first_animation(self.db, a)?;
        let mut actor = self.descend(a.name(), role, &ctrl)?;
        actor.length = a.f32("Length").unwrap_or(0.0);
        self.tags(a, self.rig(role), &mut actor.tags);
        Ok(actor)
    }

    pub(super) fn actor(&self, a: &Asset<'a>) -> Result<NisActor> {
        self.actor_as(a, None)
    }

    pub(super) fn rig(&self, role: i64) -> &'a Rig {
        match role {
            ROLE_HELICOPTER1 => self.rigs[1],
            ROLE_CAMERA1 => self.rigs[2],
            _ => self.rigs[0],
        }
    }

    fn descend(&self, name: &str, role: i64, ctrl: &Asset<'a>) -> Result<NisActor> {
        let rig = self.rig(role);
        let mut ctrl = *ctrl;
        let (mut offset, mut choices, mut seed, mut chooser) = (None, Vec::new(), None, None);
        let (mut toc_root, mut loop_end) = (None, None);
        for _ in 0..24 {
            match ctrl.class() {
                "TrajectoryOffsetControllerAsset" => {
                    let n = |f: &str| ctrl.guid(f).and_then(|g| self.db.get(g)).map(|x| x.name().to_string());
                    offset = Some((
                        n("TranslationOffsetAsset").unwrap_or_default(),
                        n("RotationOffsetAsset").unwrap_or_default(),
                    ));
                    let subject = self
                        .db
                        .get(ctrl.guid("SubjectControllerAsset").context("TOC without subject")?)
                        .context("TOC subject")?;
                    if subject.class() == "LayersControllerAsset" {
                        toc_root = Some(ctrl.name().to_string());
                    }
                    ctrl = subject;
                }
                "LayersControllerAsset" => {
                    let first = ctrl.guids("ControllerAssets").first().copied().context("no layer 0")?;
                    ctrl = self.db.get(first).context("layer 0")?;
                }
                "LoopingControllerAsset" => {
                    loop_end = loop_end.or(ctrl.f32("LoopEndTime"));
                    ctrl = self
                        .db
                        .get(ctrl.guid("SubjectControllerAsset").context("loop without subject")?)
                        .context("loop subject")?;
                }
                "ProxyControllerAsset" => {
                    ctrl = self.db.get(self.proxy_subject(&ctrl, role)?).context("proxy subject not loaded")?;
                }
                "SequenceContainerAsset" => {
                    let actors: Vec<Asset> =
                        ctrl.guids("ActorAssets").into_iter().filter_map(|g| self.db.get(g)).collect();
                    let a = match ctrl.guid("TrackSelectionGameStateEnum") {
                        Some(_) => actors
                            .iter()
                            .find(|a| enum_value(self.db, a, "TrackSelectionEnumValue") == Some(role))
                            .with_context(|| format!("{}: no actor of role {role}", ctrl.name()))?,
                        None => actors.first().context("sequence without actor")?,
                    };
                    let mut inner = self.actor_as(a, Some(role))?;
                    inner.name = name.to_string();
                    inner.role = role;
                    if inner.offset.is_none() {
                        inner.offset = offset;
                    }
                    inner.root = Some((toc_root.unwrap_or_else(|| ctrl.name().to_string()), loop_end));
                    return Ok(inner);
                }
                "ChooserControllerAsset" => {
                    let policy =
                        ctrl.guid("PolicyAsset").and_then(|g| self.db.get(g)).context("chooser without policy")?;
                    let entries: Vec<Asset> =
                        ctrl.guids("EntryAssetList").into_iter().filter_map(|g| self.db.get(g)).collect();
                    let target = |e: &Asset| e.guid("ControllerAsset").and_then(|g| self.db.get(g));
                    if policy.is("RandomChooserPolicyAsset") {
                        seed = policy
                            .guid("RandomValueAsset")
                            .and_then(|g| self.db.get(g))
                            .and_then(|r| r.int("Seed"))
                            .map(|s| s as u32);
                        for e in &entries {
                            let c = target(e).context("chooser entry")?;
                            let item = self.descend(name, role, &c)?;
                            choices.push((item.clip, e.f32("Weight").unwrap_or(1.0)));
                        }
                        anyhow::ensure!(!choices.is_empty(), "{name}: empty chooser");
                        chooser = Some(self.chooser_of(&ctrl));
                        ctrl = target(&entries[0]).context("chooser item")?;
                    } else if policy.is("SignalChooserPolicyAsset") {
                        let holds = |e: &Asset, f: &str, want: bool| {
                            e.guids(f)
                                .into_iter()
                                .filter_map(|g| self.db.get(g))
                                .all(|sig| signal_value(&sig, self.reverse_camera) == Some(want))
                        };
                        let e = entries
                            .iter()
                            .find(|e| holds(e, "TrueSignals", true) && holds(e, "FalseSignals", false))
                            .with_context(|| format!("{}: no signal entry holds", ctrl.name()))?;
                        ctrl = target(e).context("signal chooser entry")?;
                    } else if policy.is("EnumChooserPolicyAsset") {
                        let state = self.name(&policy, "GameStateAsset").unwrap_or_default();
                        let value = match state.as_str() {
                            "GSENUM_NisActorRoleId" => role,
                            s => *self.enums.get(s).with_context(|| format!("{}: no value for {s}", ctrl.name()))?,
                        };
                        let e = entries
                            .iter()
                            .find(|e| enum_value(self.db, e, "EnumerationValueAsset") == Some(value))
                            .or_else(|| entries.iter().find(|e| e.guid("EnumerationValueAsset").is_none()))
                            .with_context(|| format!("{}: no entry for {state} = {value}", ctrl.name()))?;
                        ctrl = target(e).context("enum chooser entry")?;
                    } else {
                        bail!("{name}: {} chooser", policy.class());
                    }
                }
                "ClipControllerAsset" => {
                    let clip = NisClip::load(self.db, self.all, &ctrl, &rig.name)?;
                    return Ok(NisActor {
                        name: name.to_string(),
                        role,
                        length: clip.num_ticks,
                        clip,
                        offset,
                        choices,
                        seed,
                        chooser,
                        sequences: Vec::new(),
                        root: toc_root.map(|r| (r, loop_end)),
                        tags: Tags::default(),
                    });
                }
                c => bail!("{name}: {} is a {c}, no clip", ctrl.name()),
            }
        }
        bail!("{name}: too deep")
    }

    fn role_scene(&self, node: &Asset<'a>, chooser: &Asset<'a>) -> Result<NisScene> {
        let entries: Vec<(Option<i64>, Asset)> = chooser
            .guids("EntryAssetList")
            .into_iter()
            .filter_map(|g| self.db.get(g))
            .filter_map(|e| {
                Some((enum_value(self.db, &e, "EnumerationValueAsset"), self.db.get(e.guid("ControllerAsset")?)?))
            })
            .collect();
        let (mut actors, mut problems) = (Vec::new(), Vec::new());
        let mut heli_movement = None;
        for &role in &self.roles {
            let own = entries.iter().find(|(r, _)| *r == Some(role));
            let Some((value, ctrl)) = own.or_else(|| entries.iter().find(|(r, _)| r.is_none())) else { continue };
            let random = value.is_none()
                && role == ROLE_CHARACTER1
                && ctrl.class() == "ChooserControllerAsset"
                && ctrl
                    .guid("PolicyAsset")
                    .and_then(|g| self.db.get(g))
                    .is_some_and(|p| p.is("RandomChooserPolicyAsset"));
            if role == ROLE_HELICOPTER1 && ctrl.is("HeliMovementControllerAsset") {
                heli_movement = Some(ctrl.name().to_string());
            }
            let r = if random { self.snow_character(ctrl) } else { self.descend(ctrl.name(), role, ctrl) };
            match r {
                Ok(a) => actors.push(a),
                Err(e) => problems.push(format!("role {role} ({}): {e:#}", ctrl.name())),
            }
        }
        anyhow::ensure!(
            actors.iter().any(|a| a.role == ROLE_CHARACTER1),
            "{}: no Character1 clip ({})",
            chooser.name(),
            problems.join("; ")
        );
        let (sequence, loop_end) = actors
            .iter()
            .find(|a| a.role == ROLE_CHARACTER1)
            .and_then(|a| a.root.clone())
            .unwrap_or_else(|| (chooser.name().to_string(), None));
        let mut node_tags = Tags::default();
        self.tags(node, self.rigs[0], &mut node_tags);
        let active_signals = self.names(node, "ActiveSignals");
        Ok(NisScene {
            node: node.name().to_string(),
            sequence,
            loop_end,
            actors,
            node_tags,
            problems,
            active_signals,
            heli_movement,
        })
    }

    fn snow_character(&self, chooser: &Asset<'a>) -> Result<NisActor> {
        anyhow::ensure!(chooser.class() == "ChooserControllerAsset", "{}: no chooser", chooser.name());
        let policy = chooser.guid("PolicyAsset").and_then(|g| self.db.get(g)).context("chooser without policy")?;
        anyhow::ensure!(policy.is("RandomChooserPolicyAsset"), "{}: {} chooser", chooser.name(), policy.class());
        let seed =
            policy.guid("RandomValueAsset").and_then(|g| self.db.get(g)).and_then(|r| r.int("Seed")).map(|s| s as u32);
        let mut out: Option<NisActor> = None;
        for e in chooser.guids("EntryAssetList").into_iter().filter_map(|g| self.db.get(g)) {
            let proxy = e.guid("ControllerAsset").context("entry without controller")?;
            let (seq, _) = self.sequence(proxy, None, 0)?;
            let actor = seq
                .guids("ActorAssets")
                .into_iter()
                .filter_map(|g| self.db.get(g))
                .find(|a| {
                    a.guid("TrackSelectionEnumValue").and_then(|g| self.db.get(g)).and_then(|v| v.int("Value"))
                        == Some(ROLE_CHARACTER1)
                })
                .with_context(|| format!("{}: no Character1 actor", seq.name()))?;
            let item = self.actor(&actor)?;
            let weight = e.f32("Weight").unwrap_or(1.0);
            match &mut out {
                None => {
                    let mut first = item.clone();
                    first.name = chooser.name().to_string();
                    first.seed = seed;
                    first.chooser = Some(self.chooser_of(chooser));
                    first.choices = vec![(item.clip, weight)];
                    first.sequences = vec![(seq.name().to_string(), item.length)];
                    out = Some(first);
                }
                Some(o) => {
                    o.choices.push((item.clip, weight));
                    o.sequences.push((seq.name().to_string(), item.length));
                }
            }
        }
        out.with_context(|| format!("{}: empty chooser", chooser.name()))
    }

    fn chooser_of(&self, c: &Asset) -> NisChooser {
        NisChooser {
            looping: c.bool("Looping").unwrap_or(false),
            choose_on_loop: c.bool("ChooseOnLoop").unwrap_or(false),
            blend: c.guid("BlendTag").and_then(|g| self.db.get(g)).and_then(|b| Blend::of_asset(&b)),
        }
    }

    pub(super) fn tags(&self, a: &Asset<'a>, rig: &Rig, actor: &mut Tags) {
        let Some(set) = a.guid("TagCollectionSet").and_then(|g| self.db.get(g)) else { return };
        let mut seen = Vec::new();
        for c in set.guids("TagCollectionKeys").into_iter().filter_map(|g| self.db.get(g)) {
            let keys = c.guids("TagKeys").into_iter().chain(c.guids("MonitoredTagKeys"));
            for t in keys.filter_map(|g| self.db.get(g)) {
                if seen.contains(&t.guid) {
                    continue;
                }
                seen.push(t.guid);
                let value_name = || t.guid("ValueAsset").and_then(|g| self.db.get(g)).map(|v| v.name().to_string());
                match t.class() {
                    "BoolGameStateTag" if value_name().as_deref() == Some("BLN_NisRequestStop") => {
                        actor.stop = Some(window(&t));
                    }
                    "BoolGameStateTag" if value_name().as_deref() == Some("BLN_NisOutroActive") => {
                        actor.outro_active = Some(window(&t));
                    }
                    "JointToJointAttachmentDriverTag" | "JointToJointAttachmentTag" => {
                        let target = self.name(&t, "TargetAnimatableIdAsset").unwrap_or_default();
                        let target_rig = match target.as_str() {
                            "ID_NisTargetPlayer" => self.rigs[0],
                            _ => self.rigs[1],
                        };
                        let joint = |f: &str, r: &Rig| {
                            t.guid(f).and_then(|g| self.db.get(g)).and_then(|id| template_joint(self.db, r, &id))
                        };
                        let att = Attachment {
                            window: window(&t),
                            source_joint: joint("SourceJoint", rig).unwrap_or_default(),
                            target_joint: joint("TargetJoint", target_rig).unwrap_or_default(),
                            target,
                            offset_t: vec3_of(&t, "OffsetTranslation"),
                            offset_q: quat_of(&t, "OffsetRotation"),
                            blend_in: t.f32("BlendInTicks").unwrap_or(0.0),
                        };
                        if t.class() == "JointToJointAttachmentDriverTag" {
                            actor.drivers.push(att);
                        } else {
                            actor.attachments.push(att);
                        }
                    }
                    "BranchOutPointTag" => {
                        let kind = self.name(&t, "BranchTypeAsset").unwrap_or_default();
                        match kind.as_str() {
                            "NIS_Out" => actor.branch_out = Some(num(&t, "Time")),
                            "NIS_EarlyOut" => actor.early_out = Some(num(&t, "Time")),
                            _ => {}
                        }
                        if t.guid("BlendAsset").is_some_and(|g| self.db.get(g).is_some()) {
                            actor.window_blends.push(kind);
                        }
                    }
                    "BranchInWindowTag" if self.name(&t, "BranchTypeAsset").as_deref() == Some("NIS_In") => {
                        actor.branch_in = Some(num(&t, "Time"));
                    }
                    c if c.ends_with("GameStateTag") && value_name().as_deref() == Some("VEC_NisTuningOffset") => {
                        actor.tuning_offset = Some(vec3_of(&t, "Value"));
                    }
                    c @ ("BoolGameStateTag" | "FloatGameStateTag")
                        if value_name().is_some_and(|n| GAME_STATE_TAGS.contains(&n.as_str())) =>
                    {
                        let float = c == "FloatGameStateTag";
                        let value = match float {
                            true => num(&t, "Value"),
                            false => f32::from(u8::from(t.bool("Value").unwrap_or(false))),
                        };
                        let var = t.guid("ValueAsset").and_then(|g| self.db.get(g));
                        let default = match float {
                            true => var.and_then(|v| v.f32("Default")).unwrap_or(0.0),
                            false => f32::from(u8::from(var.and_then(|v| v.bool("Default")).unwrap_or(false))),
                        };
                        actor.game_state.push(GameStateTag {
                            state: value_name().unwrap_or_default(),
                            window: window(&t),
                            float,
                            value,
                            mode: t.int("Mode").unwrap_or(0) as u32,
                            reset_on_leave: t.bool("ResetOnLeave").unwrap_or(false),
                            default,
                            spline: t.array("Spline").iter().filter_map(Value::as_f32).collect(),
                        });
                    }
                    "ScriptTag" => actor.scripts.push(ScriptCall {
                        window: window(&t),
                        script: self.name(&t, "ScriptAsset").unwrap_or_default(),
                        update: t.int("UpdateFunctionOrdinal").unwrap_or(-1),
                        enter: t.int("EnterFunctionOrdinal").unwrap_or(-1),
                        exit: t.int("ExitFunctionOrdinal").unwrap_or(-1),
                    }),
                    _ => {}
                }
            }
        }
    }

    pub(super) fn scene(&self, node: &Asset<'a>) -> Result<NisScene> {
        let subject = node.guid("SubjectController").with_context(|| format!("{}: no subject", node.name()))?;
        if let Some(c) = self.db.get(subject).filter(|c| c.class() == "ChooserControllerAsset")
            && c.guid("PolicyAsset").and_then(|g| self.db.get(g)).is_some_and(|p| p.is("EnumChooserPolicyAsset"))
        {
            return self.role_scene(node, &c).with_context(|| format!("node {}", node.name()));
        }
        let (seq, loop_end) = self.sequence(subject, None, 0).with_context(|| format!("node {}", node.name()))?;
        let actors = seq
            .guids("ActorAssets")
            .into_iter()
            .filter_map(|g| self.db.get(g))
            .map(|a| self.actor(&a))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("sequence {}", seq.name()))?;
        let mut node_tags = Tags::default();
        self.tags(node, self.rigs[0], &mut node_tags);
        Ok(NisScene {
            node: node.name().to_string(),
            sequence: seq.name().to_string(),
            loop_end,
            actors,
            node_tags,
            problems: Vec::new(),
            active_signals: self.names(node, "ActiveSignals"),
            heli_movement: None,
        })
    }
}

impl<'a> Walk<'a> {
    pub(super) fn sequence_scene(&self, seq: &Asset<'a>) -> Result<NisScene> {
        let actors = seq
            .guids("ActorAssets")
            .into_iter()
            .filter_map(|g| self.db.get(g))
            .map(|a| self.actor(&a))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("sequence {}", seq.name()))?;
        Ok(NisScene {
            node: seq.name().to_string(),
            sequence: seq.name().to_string(),
            loop_end: None,
            actors,
            node_tags: Tags::default(),
            problems: Vec::new(),
            active_signals: Vec::new(),
            heli_movement: None,
        })
    }
}
