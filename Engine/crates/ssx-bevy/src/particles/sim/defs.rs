use std::sync::Arc;

use ssx_formats::lynx::{Action, Effect, Param};

use super::Params;
use super::draw::RenderConsts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    EmitBox,
    EmitSphere,
    EmitSpheroid,
    Evolve,
    Accelerate,
    Drag,
    SpeedLimit,
    KillAge,
    Weather,
    Turbulence,
    PeriodicBox,
    CopyToParameter,
    Shadow,
    Ao,
    Render,
    Other,
}

impl Kind {
    pub(super) fn instancing_flags(self) -> (bool, bool) {
        match self {
            Kind::Render => (true, false),
            Kind::EmitBox | Kind::EmitSphere | Kind::EmitSpheroid => (false, true),
            Kind::Ao | Kind::Shadow => (true, true),
            _ => (false, false),
        }
    }

    fn of(class: &str) -> Kind {
        match class {
            "ParticleActionEmitBox" => Kind::EmitBox,
            "ParticleActionEmitSphere" => Kind::EmitSphere,
            "ParticleActionWeather" => Kind::Weather,
            "ParticleActionTurbulence" => Kind::Turbulence,
            "ParticleActionPeriodicBox" => Kind::PeriodicBox,
            "ParticleActionCopyToParameter" => Kind::CopyToParameter,
            "ParticleActionEmitSpheroid" => Kind::EmitSpheroid,
            "ParticleActionEvolve" => Kind::Evolve,
            "ParticleActionAccelerate" => Kind::Accelerate,
            "ParticleActionDrag" => Kind::Drag,
            "ParticleActionSpeedLimit" => Kind::SpeedLimit,
            "ParticleActionKillAge" => Kind::KillAge,
            "ParticleActionShadow" => Kind::Shadow,
            "ParticleActionAo" => Kind::Ao,
            "ParticleActionRender" => Kind::Render,
            _ => Kind::Other,
        }
    }
}

pub struct ActionDef {
    pub kind: Kind,
    pub class: String,
    pub instance_name: Option<String>,
    pub params: Params,
    pub render: Option<Arc<RenderConsts>>,
}

impl ActionDef {
    pub fn texture(&self) -> Option<&str> {
        if self.kind != Kind::Render {
            return None;
        }
        super::View::new(&self.params, &super::Weights::NONE).text("sTexture")
    }
}

pub struct GroupDef {
    pub name: String,
    pub params: Params,
    pub actions: Vec<ActionDef>,
}

pub struct EffectDef {
    pub name: String,
    pub params: Params,
    pub groups: Vec<GroupDef>,
}

fn state_number(id: &str) -> u8 {
    id.trim().parse::<u32>().map_or(0, |n| n.min(15) as u8)
}

impl EffectDef {
    pub fn compile(e: &Effect) -> Arc<EffectDef> {
        let groups = e
            .groups
            .iter()
            .map(|g| GroupDef {
                name: g.name.clone(),
                params: Params::new("ParticleGroup", &g.params, &[]),
                actions: g.actions.iter().map(compile_action).collect(),
            })
            .collect();
        Arc::new(EffectDef { name: e.name.clone(), params: Params::new("ParticleEffect", &e.params, &[]), groups })
    }
}

pub(super) fn compile_action(a: &Action) -> ActionDef {
    let states: Vec<(u8, &[Param])> = a.states.iter().map(|s| (state_number(&s.id), s.params.as_slice())).collect();
    let kind = Kind::of(&a.class_name);
    let params = Params::new(&a.class_name, &a.params, &states);
    let render = (kind == Kind::Render).then(|| Arc::new(RenderConsts::of(&params)));
    ActionDef {
        kind,
        class: a.class_name.clone(),
        instance_name: a.instance_name.clone().or_else(|| a.name.clone()),
        params,
        render,
    }
}
