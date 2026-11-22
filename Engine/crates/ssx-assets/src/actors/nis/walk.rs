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

struct TrackAnim<'a> {
    asset: Asset<'a>,
    start_time: f32,
    start_in_time: f32,
    end_in_time: f32,
    scale: f32,
    flags: i64,
    holds: (f32, f32),
    blend: Option<Blend>,
}

impl TrackAnim<'_> {
    fn clock(&self) -> Result<ElementClock> {
        let scale = if self.scale == 0.0 { 1.0 } else { self.scale };
        anyhow::ensure!(scale.is_finite() && scale > 0.0, "Scale {} (not ported)", self.scale);
        Ok(ElementClock {
            begin: self.start_time + self.start_in_time,
            start_in: self.start_in_time,
            scale,
            end: self.start_time + self.end_in_time,
        })
    }
}

fn track_animations<'a>(db: &'a AssetDb, a: &Asset<'a>) -> Result<Vec<TrackAnim<'a>>> {
    let track = a.array("Tracks").first().context("no track")?;
    let anims = field(a.schema, track, "Anims").and_then(Value::as_array).unwrap_or_default();
    let mut out = Vec::with_capacity(anims.len());
    for (k, an) in anims.iter().enumerate() {
        let fv = |n: &str| {
            field(a.schema, an, n).and_then(|v| v.as_f32().or_else(|| v.as_i64().map(|x| x as f32))).unwrap_or(0.0)
        };
        let g = field(a.schema, an, "Asset").and_then(Value::as_guid).with_context(|| format!("animation {k}"))?;
        let blend = match field(a.schema, an, "BlendAsset").and_then(Value::as_guid).filter(|g| !g.is_null()) {
            Some(g) => {
                let b = db.get(g).with_context(|| format!("animation {k}: blend {g} not loaded"))?;
                Some(Blend::of_asset(&b).with_context(|| format!("animation {k}: {} is no blend", b.name()))?)
            }
            None => None,
        };
        out.push(TrackAnim {
            asset: db.get(g).with_context(|| format!("animation {k}: controller not loaded"))?,
            start_time: fv("StartTime"),
            start_in_time: fv("StartInTime"),
            end_in_time: fv("EndInTime"),
            scale: fv("Scale"),
            flags: field(a.schema, an, "Flags").and_then(Value::as_i64).unwrap_or(0),
            holds: (fv("StartHoldTime"), fv("EndHoldTime")),
            blend,
        });
    }
    anyhow::ensure!(!out.is_empty(), "no animation");
    Ok(out)
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
            Some(PER_ACTOR_BANK) => self.per_actor.get(&role).copied(),
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

    pub(super) fn sequence(
        &self,
        g: Guid,
        looping: Option<LoopFields>,
        depth: usize,
        layers: &mut Vec<(Asset<'a>, usize)>,
    ) -> Result<(Asset<'a>, Option<LoopFields>)> {
        let a = self.db.get(g).with_context(|| format!("controller {g} not loaded"))?;
        anyhow::ensure!(depth < 16, "{}: too deep", a.name());
        match a.class() {
            "SequenceContainerAsset" => Ok((a, looping)),
            "LayersControllerAsset" => {
                let first = a.guids("ControllerAssets").first().copied().context("no layer 0")?;
                layers.extend(Self::upper_layers(&a));
                self.sequence(first, looping, depth + 1, layers)
            }
            "LoopingControllerAsset" => {
                anyhow::ensure!(looping.is_none(), "{}: a loop inside a loop (not ported)", a.name());
                let s = a.guid("SubjectControllerAsset").context("Looping without subject")?;
                self.sequence(s, Some(LoopFields::of_asset(&a)), depth + 1, layers)
            }
            "ProxyControllerAsset" => {
                self.sequence(self.proxy_subject(&a, ROLE_CHARACTER1)?, looping, depth + 1, layers)
            }
            c => bail!("{}: {c} is not resolved to a sequence", a.name()),
        }
    }

    fn actor_as(&self, a: &Asset<'a>, role: Option<i64>) -> Result<NisActor> {
        let role = enum_value(self.db, a, "TrackSelectionEnumValue")
            .or(role)
            .with_context(|| format!("{}: no track selection", a.name()))?;
        let anims = track_animations(self.db, a)?;
        let first = &anims[0];
        let clock = first.clock().with_context(|| format!("{}: track animation 0", a.name()))?;
        anyhow::ensure!(
            clock.begin == 0.0 && first.start_time <= 0.0 && first.flags == 0 && first.holds == (0.0, 0.0),
            "{}: track animation 0 begins at {} (StartTime {}, flags {:#x}, holds {:?}; not ported)",
            a.name(),
            clock.begin,
            first.start_time,
            first.flags,
            first.holds
        );
        let mut actor = self.descend(a.name(), role, &first.asset)?;
        let nested_loop = actor.root.as_ref().is_some_and(|r| r.looping.is_some());
        anyhow::ensure!(
            clock.start_in == 0.0 && clock.scale == 1.0 || !nested_loop && actor.pieces.is_empty(),
            "{}: a nested sequence's loop or later animations under a track animation with its own clock (not ported)",
            a.name()
        );
        for l in &mut actor.layers {
            l.clock = LayerClock::Element(match l.clock {
                LayerClock::Node => clock,
                LayerClock::Element(inner) => clock.then(&inner),
            });
        }
        anyhow::ensure!(
            actor.layers.is_empty() || actor.choices.is_empty(),
            "{}: layers under a random chooser's element (not ported)",
            a.name()
        );
        actor.clock = clock.then(&actor.clock);
        actor.length = a.f32("Length").unwrap_or(0.0);
        for (k, pair) in anims.windows(2).enumerate() {
            let (before, t) = (&pair[0], &pair[1]);
            let piece = self.piece(a.name(), role, k + 1, t)?;
            let end = before.start_time + before.end_in_time;
            anyhow::ensure!(
                piece.clock.begin == end && end > actor.pieces.last().map_or(0.0, |p| p.clock.begin),
                "{}: track animation {} begins at {} (the one before ends at {end}; not ported)",
                a.name(),
                k + 1,
                piece.clock.begin
            );
            actor.pieces.push(piece);
        }
        self.tags(a, self.rig(role), &mut actor.tags);
        Ok(actor)
    }

    pub(super) fn actor(&self, a: &Asset<'a>) -> Result<NisActor> {
        self.actor_as(a, None)
    }

    fn piece(&self, name: &str, role: i64, k: usize, t: &TrackAnim<'a>) -> Result<NisPiece> {
        let blend_time = t.blend.as_ref().map_or(0.0, |b| b.blend_time);
        anyhow::ensure!(
            blend_time <= 0.0,
            "{name}: track animation {k} blends in over {blend_time} ticks (not ported)"
        );
        anyhow::ensure!(t.flags == 0, "{name}: track animation {k} has flags {:#x} (not ported)", t.flags);
        anyhow::ensure!(t.holds == (0.0, 0.0), "{name}: track animation {k} holds {:?} (not ported)", t.holds);
        let clock = t.clock().with_context(|| format!("{name}: track animation {k}"))?;
        let inner = self.descend(name, role, &t.asset)?;
        anyhow::ensure!(
            inner.choices.is_empty()
                && inner.sequences.is_empty()
                && inner.pieces.is_empty()
                && inner.clock == ElementClock::SEQUENCE,
            "{name}: track animation {k} is a chooser or a sequence (not ported)"
        );
        anyhow::ensure!(inner.layers.is_empty(), "{name}: track animation {k} has layers (not ported)");
        Ok(NisPiece { clock, clip: inner.clip, offset: inner.offset })
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
        let (mut toc_root, mut looping) = (None, None);
        let mut found: Vec<(Asset<'a>, usize)> = Vec::new();
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
                    found.extend(Self::upper_layers(&ctrl));
                    ctrl = self.db.get(first).context("layer 0")?;
                }
                "LoopingControllerAsset" => {
                    anyhow::ensure!(looping.is_none(), "{name}: {}: a loop inside a loop (not ported)", ctrl.name());
                    looping = Some((LoopFields::of_asset(&ctrl), toc_root.is_some()));
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
                    inner.root = Some(NisRoot {
                        controller: toc_root.unwrap_or_else(|| ctrl.name().to_string()),
                        looping: looping.map(|l| l.0),
                        loop_in_root: looping.is_some_and(|l| l.1),
                    });
                    inner.layers.extend(self.layers_for(role, &found, LayerClock::Node)?);
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
                            anyhow::ensure!(item.layers.is_empty(), "{name}: a random item with layers (not ported)");
                            choices.push((item.clip, e.f32("Weight").unwrap_or(1.0), item.clock));
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
                        root: toc_root.map(|controller| NisRoot {
                            controller,
                            looping: looping.map(|l| l.0),
                            loop_in_root: looping.is_some_and(|l| l.1),
                        }),
                        tags: Tags::default(),
                        clock: ElementClock::SEQUENCE,
                        pieces: Vec::new(),
                        layers: self.layers_for(role, &found, LayerClock::Node)?,
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
        let root = actors.iter().find(|a| a.role == ROLE_CHARACTER1).and_then(|a| a.root.clone());
        let sequence = root.as_ref().map_or_else(|| chooser.name().to_string(), |r| r.controller.clone());
        let own = |a: &NisActor| a.root.as_ref().and_then(|r| r.looping);
        let fields = root.as_ref().and_then(|r| r.looping);
        anyhow::ensure!(
            actors.iter().all(|a| own(a) == fields),
            "{}: the roles' loops differ (not ported)",
            chooser.name()
        );
        let in_root = root.as_ref().is_some_and(|r| r.loop_in_root);
        let looping =
            fields.map(|f| NisLoop::of_scene(f, in_root, &actors)).transpose().context(chooser.name().to_string())?;
        let mut node_tags = Tags::default();
        self.tags(node, self.rigs[0], &mut node_tags);
        let active_signals = self.names(node, "ActiveSignals");
        Ok(NisScene {
            node: node.name().to_string(),
            sequence,
            looping,
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
            let mut layers = Vec::new();
            let (seq, _) = self.sequence(proxy, None, 0, &mut layers)?;
            anyhow::ensure!(layers.is_empty(), "{}: a pick with layers (not ported)", chooser.name());
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
                    first.choices = vec![(item.clip, weight, item.clock)];
                    first.sequences = vec![(seq.name().to_string(), item.length)];
                    out = Some(first);
                }
                Some(o) => {
                    o.choices.push((item.clip, weight, item.clock));
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
        let mut layers = Vec::new();
        let (seq, fields) =
            self.sequence(subject, None, 0, &mut layers).with_context(|| format!("node {}", node.name()))?;
        let mut actors = seq
            .guids("ActorAssets")
            .into_iter()
            .filter_map(|g| self.db.get(g))
            .map(|a| self.actor(&a))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("sequence {}", seq.name()))?;
        for a in &mut actors {
            let own =
                self.layers_for(a.role, &layers, LayerClock::Node).with_context(|| format!("node {}", node.name()))?;
            a.layers.extend(own);
        }
        let looping =
            fields.map(|f| NisLoop::of_scene(f, false, &actors)).transpose().with_context(|| seq.name().to_string())?;
        let mut node_tags = Tags::default();
        self.tags(node, self.rigs[0], &mut node_tags);
        Ok(NisScene {
            node: node.name().to_string(),
            sequence: seq.name().to_string(),
            looping,
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
            looping: None,
            actors,
            node_tags: Tags::default(),
            problems: Vec::new(),
            active_signals: Vec::new(),
            heli_movement: None,
        })
    }
}
