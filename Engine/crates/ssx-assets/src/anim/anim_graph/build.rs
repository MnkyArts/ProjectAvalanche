use super::*;

pub(super) struct Builder<'a> {
    pub(super) db: &'a AssetDb,
    pub(super) g: Graph,
    pub(super) overrides: HashMap<String, Guid>,
}

fn f(a: &Asset, name: &str) -> f32 {
    a.f32(name).unwrap_or(0.0)
}

impl Builder<'_> {
    pub(super) fn signal(&mut self, guid: Option<Guid>) -> Option<SigId> {
        let guid = guid?;
        if let Some(&id) = self.g.sig_ids.get(&guid) {
            return Some(id);
        }
        let a = self.db.get(guid)?;
        let kind = if a.is("BoolAsset") {
            SignalKind::Bool(a.bool("Default").unwrap_or(false))
        } else if a.is("FloatAsset") {
            SignalKind::Float(f(&a, "Default"))
        } else if a.is("IntegerAsset") {
            SignalKind::Int(a.int("Default").unwrap_or(0))
        } else if a.is("GameStateEnumerationAsset") {
            SignalKind::Enum(a.int("Default").unwrap_or(0))
        } else if a.is("ScriptExpressionBaseAsset") {
            let sa = a.guid("ScriptAsset").and_then(|g| self.db.get(g));
            let script = sa.map(|s| s.name().to_string()).unwrap_or_default();
            let ordinal = a.int("ReadFunctionOrdinal").unwrap_or(-1);
            let symbols = sa.map(|s| s.array("SymbolTable")).unwrap_or_default();
            let function = usize::try_from(ordinal)
                .ok()
                .and_then(|o| symbols.get(o))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            SignalKind::Script { script, ordinal, float: a.is("FloatScriptExpressionAsset"), function }
        } else if a.is("Vector3Asset") || a.is("QuaternionAsset") || a.is("Vector4Asset") {
            SignalKind::Vector
        } else {
            SignalKind::Other(a.class().to_string())
        };
        let id = self.g.signals.len();
        let trigger = a.is("BoolAsset") && a.bool("DeactivateAfterTransition").unwrap_or(false);
        self.g.signals.push(Signal { name: a.name().to_string(), kind, trigger });
        self.g.sig_ids.insert(guid, id);
        Some(id)
    }

    pub(super) fn signals(&mut self, a: &Asset, field: &str) -> Vec<SigId> {
        a.guids(field).into_iter().filter_map(|g| self.signal(Some(g))).collect()
    }

    fn enum_conds(&mut self, a: &Asset, field: &str) -> Vec<EnumCond> {
        let mut out = Vec::new();
        for g in a.guids(field) {
            let Some(p) = self.db.get(g) else { continue };
            let value = p.guid("Enumerator").and_then(|e| self.db.get(e)).and_then(|e| e.int("Value")).unwrap_or(-1);
            if let Some(signal) = self.signal(p.guid("GameStateEnumeration")) {
                out.push(EnumCond { signal, value });
            }
        }
        out
    }

    pub(super) fn blend(&mut self, guid: Option<Guid>) -> Option<BlendId> {
        let guid = guid?;
        if let Some(&id) = self.g.blend_ids.get(&guid) {
            return Some(id);
        }
        let a = self.db.get(guid)?;
        if a.class() == "ProxyBlendAsset" {
            return Some(self.proxy_blend(&a));
        }
        if !a.is("IBlendAsset") {
            return None;
        }
        let samples = a.array("Samples").iter().filter_map(Value::as_f32).collect();
        let dof_curves = a.guid("WeightCurves").and_then(|g| self.dof_curves(g));
        let weight = self.signal(a.guid("BlendWeight"));
        let b = Blend {
            class: a.class().to_string(),
            blend_time: f(&a, "BlendTime"),
            launch_angle: f(&a, "LaunchAngle"),
            curve: a.int("CurveType").or_else(|| a.int("Curve")).unwrap_or(-1),
            sync_phase: a.bool("ShouldSyncPhase").unwrap_or(false),
            weight,
            samples,
            dof_curves,
        };
        let id = self.g.blends.len();
        self.g.blends.push(b);
        self.g.blend_ids.insert(guid, id);
        Some(id)
    }

    fn proxy_blend(&mut self, a: &Asset<'_>) -> BlendId {
        let mut cur = Some(a.guid);
        for _ in 0..8 {
            let Some(p) = cur.and_then(|g| self.db.get(g)) else { break };
            if p.class() != "ProxyBlendAsset" {
                if let Some(id) = self.blend(Some(p.guid)) {
                    self.g.blend_ids.insert(a.guid, id);
                    return id;
                }
                break;
            }
            let bank = self.name_of(p.guid("BankPointerAsset"));
            let paired = bank
                .as_ref()
                .and_then(|b| self.proxy_subject(b, p.guid))
                .or_else(|| p.guid("DefaultProxyBankAsset").and_then(|bank| self.bank_subject(Some(bank), p.guid)));
            cur = paired.or_else(|| p.guid("DefaultBlendAsset"));
        }
        let id = self.g.blends.len();
        self.g.blends.push(Blend {
            class: "ProxyBlendAsset (unresolved)".into(),
            blend_time: 0.0,
            launch_angle: 0.0,
            curve: -1,
            sync_phase: false,
            weight: None,
            samples: Vec::new(),
            dof_curves: None,
        });
        self.g.blend_ids.insert(a.guid, id);
        id
    }

    pub(super) fn dof_curves(&self, g: Guid) -> Option<DofCurves> {
        let a = self.db.get(g)?;
        let layouts: Vec<String> = a
            .guid("DofSetList")
            .and_then(|h| self.db.get(h))
            .map(|h| h.guids("LayoutAssets").into_iter().map(|l| self.name_of(Some(l)).unwrap_or_default()).collect())
            .unwrap_or_default();
        let sets = a
            .array("DofSetCurves")
            .iter()
            .zip(layouts)
            .map(|(v, layout)| {
                let bytes = |f: &str| field(a.schema, v, f).and_then(Value::as_bytes).unwrap_or_default().to_vec();
                let raw = bytes("CurveBytes");
                DofSetCurve {
                    layout,
                    for_each: field(a.schema, v, "ForEachDof").and_then(Value::as_bool).unwrap_or(false),
                    curves: raw.as_chunks::<4>().0.to_vec(),
                    inherited: bytes("InheritedMaskBytes"),
                }
            })
            .collect();
        Some(DofCurves { sets })
    }

    fn name_of(&self, guid: Option<Guid>) -> Option<String> {
        guid.and_then(|g| self.db.get(g)).map(|a| a.name().to_string())
    }

    pub(super) fn ctrl(&mut self, guid: Option<Guid>) -> Option<CtrlId> {
        let guid = guid?;
        if let Some(&id) = self.g.ctrl_ids.get(&guid) {
            return Some(id);
        }
        let a = self.db.get(guid)?;
        let id = self.g.ctrls.len();
        self.g.ctrls.push((a.name().to_string(), Ctrl::Other(a.class().to_string())));
        self.g.ctrl_ids.insert(guid, id);
        let c = self.build(&a);
        self.g.ctrls[id].1 = c;
        Some(id)
    }
}

impl<'a> Builder<'a> {
    fn build(&mut self, a: &Asset<'a>) -> Ctrl {
        let sub = |s: &mut Self, f: &str| s.ctrl(a.guid(f));
        match a.class() {
            "StateFlowControllerAsset" => Ctrl::StateFlow(self.state_flow(a)),
            "ModularControllerAsset" => {
                let items = a.guids("Items").into_iter().filter_map(|g| self.db.get(g)).collect::<Vec<_>>();
                Ctrl::Modular(
                    items
                        .iter()
                        .map(|i| (self.ctrl(i.guid("ControllerAsset")), i.bool("Enabled").unwrap_or(true)))
                        .collect(),
                )
            }
            "ChooserControllerAsset" => self.chooser(a),
            "ClipControllerAsset" => Ctrl::Clip {
                anim: self
                    .name_of(a.guid("Anim"))
                    .map(|n| n.strip_suffix(" Anim").unwrap_or(&n).to_string())
                    .unwrap_or_default(),
                anim_guid: a.guid("Anim"),
                target: self.name_of(a.guid("Target")).unwrap_or_default(),
                num_ticks: f(a, "NumTicks"),
                fps: f(a, "FPS"),
                fps_scale: f(a, "FPSScale"),
                delta_trajectory: a.int("DeltaTrajectory").unwrap_or(0) as u8,
                distance: f(a, "Distance"),
            },
            "SequenceContainerAsset" => match a.guids("ActorAssets").first().and_then(|g| self.db.get(*g)) {
                Some(actor) => self.sequence(&actor),
                None => Ctrl::Other("SequenceContainerAsset (no actor)".into()),
            },
            "ActorControllerAsset" => self.sequence(a),
            "LoopingControllerAsset" => Ctrl::Looping {
                subject: sub(self, "SubjectControllerAsset"),
                loop_control: a.bool("IsLoopControl").unwrap_or(false),
                start: f(a, "StartTime"),
                loop_start: f(a, "LoopStartTime"),
                loop_end: f(a, "LoopEndTime"),
            },
            "SignalBlendArrayControllerAsset" => {
                let arr = a.guid("BlendArray").and_then(|g| self.db.get(g));
                let mut items = Vec::new();
                let (mut range, mut snap) = ([0.0; 2], 0);
                if let Some(arr) = arr {
                    range = [f(&arr, "Min"), f(&arr, "Max")];
                    snap = arr.int("SnapToAnim").unwrap_or(0);
                    for it in arr.array("Items") {
                        let fv = |n: &str| field(arr.schema, it, n).and_then(Value::as_f32).unwrap_or(0.0);
                        let clip = field(arr.schema, it, "Clip").and_then(Value::as_guid).filter(|g| !g.is_null());
                        items.push(BlendItem { controller: self.ctrl(clip), x: fv("X"), end_time: fv("EndTime") });
                    }
                }
                Ctrl::BlendArray {
                    signal: self.signal(a.guid("BlendSignal")),
                    clamp: [f(a, "ClampMin"), f(a, "ClampMax")],
                    damping: [f(a, "PositiveDamping"), f(a, "NegativeDamping")],
                    mirrored: a.bool("Mirrored").unwrap_or(false),
                    master: sub(self, "MasterController"),
                    master_length: f(a, "MasterControllerLength"),
                    items,
                    range,
                    snap,
                }
            }
            "SignalPhaseControllerAsset" => Ctrl::SignalPhase {
                subject: sub(self, "SubjectControllerAsset"),
                phase: self.signal(a.guid("PhaseAsset")),
                range: [f(a, "MinPhase"), f(a, "MaxPhase")],
                curve: self.name_of(a.guid("PhaseCurve")),
            },
            "ReferencePoseBlendControllerAsset" => Ctrl::ReferencePoseBlend {
                subject: sub(self, "SubjectControllerAsset"),
                reference: sub(self, "ReferenceControllerAsset"),
                frame: f(a, "ReferencePoseFrame"),
                weight: self.signal(a.guid("BlendWeightAsset")),
            },
            "OffsetPoseControllerAsset" => Ctrl::OffsetPose {
                subject: sub(self, "SubjectControllerAsset"),
                reference: sub(self, "ReferenceControllerAsset"),
                frame: f(a, "ReferenceFrame"),
            },
            "PlaybackRateControllerAsset" => Ctrl::PlaybackRate {
                subject: sub(self, "Subject"),
                signal: self.signal(a.guid("TimeScaleGameState")),
                scale: f(a, "TimeScale"),
                range: [f(a, "MinScale"), f(a, "MaxScale")],
            },
            "LayersControllerAsset" => self.layers(a),
            "AutoMirrorControllerAsset" => Ctrl::AutoMirror {
                subject: sub(self, "SubjectControllerAsset"),
                signal: self.signal(a.guid("MirrorSignal")),
                mirror_list: {
                    let items = a
                        .guid("MirrorAssetList")
                        .and_then(|g| self.db.get(g))
                        .map(|l| l.guids("AssetListItems"))
                        .unwrap_or_default();
                    items.into_iter().filter_map(|g| self.signal(Some(g))).collect()
                },
                duration: f(a, "BlendDuration"),
                freeze: a.bool("FreezeSlaveOnBlend").unwrap_or(false),
                continuous: a.bool("ContinuousUpdating").unwrap_or(false),
            },
            "MirrorController" => {
                Ctrl::Mirror { subject: sub(self, "ControllerAsset"), phase_offset: f(a, "PhaseOffset") }
            }
            "ProxyControllerAsset" => {
                let bank = self.name_of(a.guid("BankPointerAsset"));
                let subject = bank.as_ref().and_then(|b| self.proxy_subject(b, a.guid));
                Ctrl::Proxy { default: sub(self, "DefaultControllerAsset"), bank, resolved: self.ctrl(subject) }
            }
            "TrajMaskControllerAsset" => Ctrl::TrajMask { subject: sub(self, "SubjectControllerAsset") },
            "ContextDatabaseChooserWrapperAsset" => self.context_db(a),
            "SignalBlendSpaceControllerAsset" => self.blend_space(a),
            c if a.is("WrapperControllerAsset") => {
                Ctrl::Physics { class: c.to_string(), subject: sub(self, "SubjectControllerAsset") }
            }
            c => Ctrl::Other(c.to_string()),
        }
    }

    fn blend_space(&mut self, a: &Asset<'a>) -> Ctrl {
        let flags = ["IsWrapX", "IsWrapY", "AbsoluteIncDecX", "AbsoluteIncDecY"];
        let Some(space) = a.guid("SubjectControllerAsset").and_then(|g| self.db.get(g)).filter(|s| s.is("BlendSpace"))
        else {
            return Ctrl::Other("SignalBlendSpaceControllerAsset (no BlendSpace)".into());
        };
        if flags.iter().any(|f| a.bool(f).unwrap_or(false)) {
            return Ctrl::Other("SignalBlendSpaceControllerAsset (wrap / absolute inc-dec)".into());
        }
        let axis = |c: &str| ["MaxDec", "MaxInc", "MaxAcc"].map(|k| f(a, &format!("Signal{c}{k}")));
        let (ax, ay) = (axis("X"), axis("Y"));
        let axes = [
            [ax[0], ax[1], ax[2], f(a, "RegionXMin"), f(a, "RegionXMax")],
            [ay[0], ay[1], ay[2], f(a, "RegionYMin"), f(a, "RegionYMax")],
        ];
        let sch = space.schema;
        let Some(diagram) = space.value("VoronoiDiagram") else {
            return Ctrl::Other("BlendSpace (no VoronoiDiagram)".into());
        };
        let list = |v: &'a Value, name: &str| field(sch, v, name).and_then(Value::as_array).unwrap_or_default();
        let vec2 = |v: Option<&Value>| -> [f32; 2] {
            let a = v.and_then(Value::as_array).unwrap_or_default();
            [a.first().and_then(Value::as_f32).unwrap_or(0.0), a.get(1).and_then(Value::as_f32).unwrap_or(0.0)]
        };
        let mut items = Vec::new();
        let mut snap = Vec::new();
        for it in list(diagram, "Items") {
            let c = field(sch, it, "BlendAsset").and_then(Value::as_guid).filter(|g| !g.is_null());
            let pos = vec2(field(sch, it, "Position"));
            let c = self.ctrl(c);
            items.push((pos, c));
            snap.push(matches!(field(sch, it, "Snap"), Some(Value::Bool(true))));
        }
        let inverse_area = list(diagram, "Triangles")
            .iter()
            .map(|t| field(sch, t, "InverseArea").and_then(Value::as_f32).unwrap_or(0.0))
            .collect();
        if space.int("ClampMode").unwrap_or(0) != 0 || space.int("BlendMode").unwrap_or(0) != 0 {
            return Ctrl::Other("BlendSpace (ClampMode / BlendMode ≠ 0)".into());
        }
        let snap_tolerance = space.f32("SnapTolerance").unwrap_or(0.0);
        let triangles = list(diagram, "Triangles")
            .iter()
            .map(|t| {
                let v = field(sch, t, "V").and_then(Value::as_array).unwrap_or_default();
                [0, 1, 2].map(|k| v.get(k).and_then(Value::as_i64).unwrap_or(0).max(0) as usize)
            })
            .collect();
        let hull = list(diagram, "Edges")
            .iter()
            .map(|e| {
                let t = field(sch, e, "TriangleIndex").and_then(Value::as_i64).unwrap_or(0).max(0) as usize;
                (vec2(field(sch, e, "PosBegin")), vec2(field(sch, e, "PosEnd")), t)
            })
            .collect();
        let (x, y) = (self.signal(a.guid("SignalAssetX")), self.signal(a.guid("SignalAssetY")));
        Ctrl::BlendSpace { x, y, axes, items, triangles, inverse_area, hull, snap, snap_tolerance }
    }

    fn context_db(&mut self, a: &Asset<'a>) -> Ctrl {
        let Some(db) = a.guid("ContextDatabaseAsset").and_then(|g| self.db.get(g)) else {
            return Ctrl::Other("ContextDatabaseChooserWrapperAsset (no database)".into());
        };
        let mut fields = Vec::new();
        for f in db.array("FieldDescriptionAssets") {
            let sig = field(db.schema, f, "GameStateDriverAsset0")
                .and_then(Value::as_guid)
                .and_then(|g| self.signal(Some(g)));
            let kind = match field(db.schema, f, "FieldType").and_then(Value::as_i64) {
                Some(0) => ContextField::Bool,
                Some(1) => ContextField::Enum,
                Some(2) => ContextField::Bitmap {
                    shift: field(db.schema, f, "AutoShiftEnumsForBitmap").and_then(Value::as_bool).unwrap_or(false),
                },
                t => return Ctrl::Other(format!("ContextDatabaseAsset (field type {t:?})")),
            };
            fields.push((sig, kind));
        }
        let mut entries = Vec::new();
        for e in db.array("EntryAssets") {
            let ctrl = field(db.schema, e, "Asset").and_then(Value::as_guid).filter(|g| !g.is_null());
            let cells = field(db.schema, e, "Cells")
                .and_then(Value::as_array)
                .unwrap_or_default()
                .iter()
                .map(|c| field(db.schema, c, "Int0").and_then(Value::as_i64).unwrap_or(0))
                .collect();
            let ctrl = self.ctrl(ctrl);
            entries.push((ctrl, cells));
        }
        Ctrl::ContextDb { fields, entries }
    }

    fn proxy_subject(&self, bank: &str, proxy: Guid) -> Option<Guid> {
        let pointer = self.overrides.get(bank).copied().or_else(|| self.db.by_name(bank).map(|a| a.guid))?;
        self.bank_subject(self.db.get(pointer)?.guid("SubjectBankAsset"), proxy)
    }

    fn bank_subject(&self, mut next: Option<Guid>, proxy: Guid) -> Option<Guid> {
        for _ in 0..16 {
            let b = self.db.get(next?)?;
            for pair in b.guids("AssetListItems").into_iter().filter_map(|g| self.db.get(g)) {
                if pair.guid("ProxyController") == Some(proxy) {
                    return pair.guid("SubjectController");
                }
            }
            next = b.guid("ParentProxyBankAsset");
        }
        None
    }

    pub(super) fn tags(&self, a: &Asset) -> Vec<String> {
        let mut out = Vec::new();
        let Some(set) = a.guid("TagCollectionSet").and_then(|g| self.db.get(g)) else { return out };
        for c in set.guids("TagCollectionKeys").into_iter().filter_map(|g| self.db.get(g)) {
            for t in c.guids("TagKeys").into_iter().chain(c.guids("MonitoredTagKeys")).filter_map(|g| self.db.get(g)) {
                out.push(format!("{}:{}", t.class(), t.name()));
            }
        }
        out
    }

    fn state_flow(&mut self, a: &Asset<'a>) -> StateFlow {
        let node_guids = a.guids("StateFlowNodeAssets");
        let trans_guids = a.guids("StateFlowTransitionAssets");
        let node_index: HashMap<Guid, usize> = node_guids.iter().enumerate().map(|(i, g)| (*g, i)).collect();
        let trans_index: HashMap<Guid, usize> = trans_guids.iter().enumerate().map(|(i, g)| (*g, i)).collect();
        let mut transitions = Vec::new();
        for g in &trans_guids {
            let Some(t) = self.db.get(*g) else { continue };
            let idx = |f: &str| t.guid(f).and_then(|g| node_index.get(&g).copied()).unwrap_or(usize::MAX);
            let window = self.name_of(t.guid("BranchWindowTypeAsset"));
            let branch_in_window = self.name_of(t.guid("BranchInWindowTypeAsset"));
            let override_blend = self.blend(t.guid("OverrideBlend"));
            let (cond_true, cond_false) =
                (self.signals(&t, "ConditionsRequiredTrue"), self.signals(&t, "ConditionsRequiredFalse"));
            let (enum_true, enum_false) =
                (self.enum_conds(&t, "EnumTrueConditions"), self.enum_conds(&t, "EnumFalseConditions"));
            transitions.push(Transition {
                name: t.name().to_string(),
                from: idx("From"),
                to: idx("To"),
                override_blend,
                window,
                branch_in_window,
                cond_true,
                cond_false,
                enum_true,
                enum_false,
                branch_in_phase: t.int("BranchInPhase").unwrap_or(0) as u8,
                destination_phase: f(&t, "DestinationPhase"),
                branch_in_any: t.bool("BranchInAny").unwrap_or(false),
            });
        }
        let mut nodes = Vec::new();
        for g in &node_guids {
            let Some(n) = self.db.get(*g) else { continue };
            let subject = self.ctrl(n.guid("SubjectController"));
            let tr = n.guids("Transitions").into_iter().chain(n.guids("TransitionLinks"));
            let transitions_of: Vec<usize> = tr.filter_map(|g| trans_index.get(&g).copied()).collect();
            let mut tags = self.tags(&n);
            if let Some(op) = self.name_of(n.guid("StateOpAsset")) {
                tags.push(format!("StateOp:{op}"));
            }
            nodes.push(SfNode {
                name: n.name().to_string(),
                conduit: n.bool("IsConduit").unwrap_or(false),
                transparent: n.bool("IsTransparent").unwrap_or(false),
                subject,
                transitions: transitions_of,
                entry_true: self.signals(&n, "EntryConditionsRequiredTrue"),
                entry_false: self.signals(&n, "EntryConditionsRequiredFalse"),
                entry_enum_true: self.enum_conds(&n, "EnumTrueConditions"),
                entry_enum_false: self.enum_conds(&n, "EnumFalseConditions"),
                active_signals: self.signals(&n, "ActiveSignals"),
                tags,
                start_tick: n
                    .guid("SubjectControllerInitParams")
                    .and_then(|g| self.db.get(g))
                    .and_then(|p| p.f32("StartTick"))
                    .unwrap_or(0.0),
            });
        }
        StateFlow {
            name: a.name().to_string(),
            start: a.guid("StartNode").and_then(|g| node_index.get(&g).copied()).unwrap_or(0),
            mirrored: self.signal(a.guid("MirroredSignal")),
            nodes,
            transitions,
            max_active: a.int("MaxActiveNodes").unwrap_or(5) as u32,
            transition_as_blend_slave: a.bool("TransitionAsBlendSlave").unwrap_or(false),
            identifier: a.int("Identifier").unwrap_or(0) as u32,
        }
    }

    fn chooser(&mut self, a: &Asset<'a>) -> Ctrl {
        let policy = a.guid("PolicyAsset").and_then(|g| self.db.get(g));
        let policy_signal = policy.and_then(|p| self.signal(p.guid("GameStateAsset")));
        let cache_last_choice = policy.and_then(|p| p.bool("CacheLastChoice")).unwrap_or(false);
        if let Some(seed) =
            policy.and_then(|p| p.guid("RandomValueAsset")).and_then(|g| self.db.get(g)).and_then(|r| r.int("Seed"))
        {
            self.g.random_seed.get_or_insert(seed as u32);
        }
        let mut entries = Vec::new();
        for g in a.guids("EntryAssetList") {
            let Some(e) = self.db.get(g) else { continue };
            let controller = self.ctrl(e.guid("ControllerAsset"));
            let selector = match e.class() {
                "EnumChooserEntryAsset" => Selector::Enum(
                    e.guid("EnumerationValueAsset")
                        .and_then(|v| self.db.get(v))
                        .and_then(|v| v.int("Value"))
                        .unwrap_or(-1),
                ),
                "LinearSignalChooserEntryAsset" => {
                    Selector::Linear { signal: self.signal(e.guid("SignalAsset")), threshold: f(&e, "Threshold") }
                }
                "SignalChooserEntryAsset" => Selector::Signals {
                    all_true: self.signals(&e, "TrueSignals"),
                    all_false: self.signals(&e, "FalseSignals"),
                },
                "RandomChooserEntryAsset" => Selector::Random { weight: f(&e, "Weight") },
                c => Selector::Other(c.to_string()),
            };
            entries.push(ChooserEntry { controller, selector });
        }
        Ctrl::Chooser {
            policy: policy.map(|p| p.class().to_string()).unwrap_or_default(),
            policy_signal,
            cache_last_choice,
            entries,
            choose_on_reset: a.bool("ChooseOnReset").unwrap_or(false),
            choose_every_update: a.bool("ChooseEveryUpdate").unwrap_or(false),
            choose_as_blend_slave: a.bool("ChooseAsBlendSlave").unwrap_or(false),
            looping: a.bool("Looping").unwrap_or(false),
            choose_on_loop: a.bool("ChooseOnLoop").unwrap_or(false),
            phase_match_previous: a.bool("PhaseMatchPrevious").unwrap_or(false),
            blend: self.blend(a.guid("BlendTag")),
        }
    }

    fn sequence(&mut self, actor: &Asset<'a>) -> Ctrl {
        let sc = actor.schema;
        let tracks = actor.array("Tracks");
        let target = |t: &Value| {
            field(sc, t, "Target").and_then(Value::as_guid).and_then(|g| self.db.get(g)).map(|x| x.name().to_string())
        };
        let track = tracks.iter().find(|t| target(t).as_deref() == Some("AnimRig")).or(tracks.first());
        let mut anims = Vec::new();
        if let Some(t) = track {
            for an in field(sc, t, "Anims").and_then(Value::as_array).unwrap_or_default() {
                let gv = |n: &str| field(sc, an, n).and_then(Value::as_guid).filter(|g| !g.is_null());
                let fv = |n: &str| {
                    field(sc, an, n).and_then(|v| v.as_f32().or_else(|| v.as_i64().map(|x| x as f32))).unwrap_or(0.0)
                };
                anims.push(SeqAnim {
                    controller: self.ctrl(gv("Asset")),
                    blend: self.blend(gv("BlendAsset")),
                    flags: field(sc, an, "Flags").and_then(Value::as_i64).unwrap_or(0),
                    start_time: fv("StartTime"),
                    start_in_time: fv("StartInTime"),
                    end_in_time: fv("EndInTime"),
                    start_hold_time: fv("StartHoldTime"),
                    end_hold_time: fv("EndHoldTime"),
                    scale: fv("Scale"),
                });
            }
        }
        let mut windows = Vec::new();
        for g in actor.guids("BranchTable") {
            let Some(w) = self.db.get(g) else { continue };
            let bt = w.guid("BranchTypeAsset").and_then(|g| self.db.get(g));
            windows.push(BranchWindow {
                kind: bt.map(|b| b.name().to_string()).unwrap_or_default(),
                filter: bt.and_then(|b| b.int("ControllerFilterIdentifier")).unwrap_or(0) as u32,
                time: f(&w, "Time"),
                length: f(&w, "Length"),
                blend: self.blend(w.guid("BlendAsset")),
                destination_phase: f(&w, "DestinationPhase"),
            });
        }
        let start_tick = actor
            .value("DefaultInitParam")
            .and_then(|v| field(sc, v, "StartTick"))
            .and_then(Value::as_f32)
            .unwrap_or(0.0);
        let audio_tags = self.audio_tags(actor);
        let game_tags = self.game_tags(actor);
        Ctrl::Sequence { anims, length: f(actor, "Length"), windows, start_tick, audio_tags, game_tags }
    }

    fn game_tags(&mut self, a: &Asset) -> Vec<crate::anim_graph::GameTag> {
        let mut out = Vec::new();
        let Some(set) = a.guid("TagCollectionSet").and_then(|g| self.db.get(g)) else { return out };
        for c in set.guids("TagCollectionKeys").into_iter().filter_map(|g| self.db.get(g)) {
            for t in c.guids("TagKeys").into_iter().filter_map(|g| self.db.get(g)) {
                // TODO(guess): `Vector3GameStateTag`s are not applied — 27 on the disc, 4 on the rider actors (each
                // `VEC_carving_bounce_rotational_force` = (−0.125, 0, 0), Mode 1, Time 0, Duration 0, on
                // SEQ_bail_ground_nose2_180 / _nose2_0 / _fwd2_90R / _bck2_90L); the carving controller binds that
                // variable as `BounceRotationalForce` (CarvingControllerAsset field 0x4E, asset +0x150) and reads and
                // writes it as frame +0x160 (0x82CC436C / 0x82CC38FC), but its use in the carving update was not found
                // (the port has no bounce force); `IntGameStateTag`s (0 on the disc) are not applied either; settle:
                // the reads of the carving frame's +0x160 and the Vector3 tag's Enter / Apply / Leave.
                if !t.is("BoolGameStateTag") && !t.is("FloatGameStateTag") {
                    continue;
                }
                let Some(signal) = self.signal(t.guid("ValueAsset")) else { continue };
                let num = |field: &str| t.f32(field).or_else(|| t.int(field).map(|v| v as f32)).unwrap_or(0.0);
                let boolean = t.is("BoolGameStateTag");
                let value = if boolean {
                    f32::from(u8::from(t.bool("Value").unwrap_or(false)))
                } else {
                    t.f32("Value").unwrap_or(0.0)
                };
                out.push(crate::anim_graph::GameTag {
                    name: t
                        .guid("ValueAsset")
                        .and_then(|g| self.db.get(g))
                        .map(|a| a.name().to_string())
                        .unwrap_or_default(),
                    signal,
                    window: crate::tags::TagWindow::new(
                        num("Time"),
                        num("Duration"),
                        t.bool("AlwaysActive").unwrap_or(false),
                    ),
                    value,
                    boolean,
                    mode: t.int("Mode").unwrap_or(0) as u32,
                    reset_on_leave: t.bool("ResetOnLeave").unwrap_or(false),
                    default: self.g.signals.get(signal).map_or(0.0, |s| s.kind.default_value()),
                    spline: t.array("Spline").iter().filter_map(Value::as_f32).collect(),
                });
            }
        }
        out
    }

    fn audio_tags(&self, a: &Asset) -> Vec<crate::anim_graph::AudioTag> {
        let mut out = Vec::new();
        let Some(set) = a.guid("TagCollectionSet").and_then(|g| self.db.get(g)) else { return out };
        for c in set.guids("TagCollectionKeys").into_iter().filter_map(|g| self.db.get(g)) {
            for t in c.guids("TagKeys").into_iter().filter_map(|g| self.db.get(g)) {
                if !t.is("AudioEventTag") {
                    continue;
                }
                let num = |field: &str| t.f32(field).or_else(|| t.int(field).map(|v| v as f32)).unwrap_or(0.0);
                out.push(crate::anim_graph::AudioTag {
                    name: t.value("EventName").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    time: num("Time"),
                    duration: num("Duration"),
                    system: t.int("SystemID").unwrap_or(0),
                    event: t.int("EventID").unwrap_or(0),
                });
            }
        }
        out.sort_by(|a, b| a.time.total_cmp(&b.time));
        out
    }

    pub(super) fn mask(&self, g: Guid) -> Option<Mask> {
        let m = self.db.get(g)?;
        if !m.is("BlendMaskListAsset") {
            return None;
        }
        let layout = |g: Guid| self.db.get(g).map(|l| l.name().to_string()).unwrap_or_default();
        let slots = m
            .guids("MaskKeys")
            .into_iter()
            .filter_map(|k| self.db.get(k))
            .map(|k| {
                (
                    k.guid("DofSetKey").map(layout).unwrap_or_default(),
                    k.array("Values").iter().filter_map(Value::as_f32).collect(),
                )
            })
            .collect();
        let values: Vec<f32> = m.array("DefaultValues").iter().filter_map(Value::as_f32).collect();
        let defaults = m.guids("DefaultDofSetKeys").into_iter().zip(values).map(|(g, v)| (layout(g), v)).collect();
        Some(Mask { name: m.name().to_string(), slots, defaults })
    }

    pub(super) fn mask_array(&mut self, g: Guid) -> Option<MaskArray> {
        let m = self.db.get(g)?;
        if !m.is("BlendMaskArrayControllerAsset") {
            return None;
        }
        let values: Vec<f32> = m.array("Values").iter().filter_map(Value::as_f32).collect();
        let items = m.guids("Masks").into_iter().zip(values).filter_map(|(g, v)| Some((self.mask(g)?, v))).collect();
        Some(MaskArray {
            value: self.signal(m.guid("ValueAsset")),
            range: [f(&m, "MinRange"), f(&m, "MaxRange")],
            max_dec: f(&m, "MaxDec"),
            max_inc: f(&m, "MaxInc"),
            items,
        })
    }

    fn layers(&mut self, a: &Asset<'a>) -> Ctrl {
        let guid_at = |f: &str, i: usize| a.array(f).get(i).and_then(Value::as_guid).filter(|g| !g.is_null());
        let int_at = |f: &str, i: usize| a.array(f).get(i).and_then(Value::as_i64).unwrap_or(0) as u32;
        let n = a.array("ControllerAssets").len();
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            layers.push(Layer {
                controller: self.ctrl(guid_at("ControllerAssets", i)),
                signal: self.signal(guid_at("SignalAssets", i)),
                mask: guid_at("BlendMaskControllers", i).and_then(|g| self.mask(g)),
                mask_array: guid_at("BlendMaskControllers", i).and_then(|g| self.mask_array(g)),
                update_mode: int_at("UpdateModes", i),
                update_rate: int_at("UpdateRates", i),
                blend_mode: int_at("BlendModes", i),
                branch_in_mode: int_at("BranchInModes", i),
            });
        }
        Ctrl::Layers(layers)
    }
}
