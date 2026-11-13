use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};
use ssx_lua::ast::{Expr, FuncBody, LValue, Stat, TableItem};

pub const SCRIPTS_DIR: &str = "memresident/data/common/scripts";

// PORT(choice): 32 scripts, a port bound eight times the disc's most (4); tuning: the most scripts a modded event's
// record lists; the original loads no packages.
pub const MAX_SCRIPTS: usize = 32;

// PORT(choice): 128 bytes, a port bound about three times the disc's longest name (44); tuning: the longest name a
// modded event's record carries; the original loads no packages.
pub const MAX_SCRIPT_NAME: usize = 128;

// PORT(choice): 1 MiB, a port bound eight times the disc's largest script (123 307 bytes); tuning: the largest event
// script a modded package carries; the original loads no packages.
pub const MAX_SCRIPT_BYTES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntroVariant {
    Heli,
    Snow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntroChoice {
    pub script: String,
    pub binding: String,
    pub variant: IntroVariant,
}

pub fn script_name(name: &str) -> bool {
    let ok_component = |c: &str| {
        !c.is_empty()
            && c != "."
            && c != ".."
            && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
    };
    (1..=MAX_SCRIPT_NAME).contains(&name.len()) && name.ends_with(".lua") && name.split(['/', '\\']).all(ok_component)
}

pub fn intro_of_scripts(folder: &Path, scripts: &[String]) -> Result<Option<IntroChoice>> {
    if scripts.len() > MAX_SCRIPTS {
        bail!("{} scripts, at most {MAX_SCRIPTS}", scripts.len());
    }
    let mut sources = Vec::with_capacity(scripts.len());
    for name in scripts {
        if !script_name(name) {
            bail!("the script name {:?} is not one the reader joins to a path", name.escape_debug().to_string());
        }
        let path = folder.join(SCRIPTS_DIR).join(name.replace('\\', "/"));
        let src = crate::read::read_file(&path).with_context(|| format!("the event's script {name}"))?;
        if src.len() > MAX_SCRIPT_BYTES {
            bail!("the event's script {name}: {} bytes, at most {MAX_SCRIPT_BYTES}", src.len());
        }
        sources.push((name.clone(), src));
    }
    intro_of_sources(&sources)
}

pub fn intro_of_sources(sources: &[(String, Vec<u8>)]) -> Result<Option<IntroChoice>> {
    let mut snow = false;
    for (name, src) in sources {
        let chunk = ssx_lua::parser::parse(src, name).map_err(|e| anyhow::anyhow!("the script {name}: {}", e.0))?;
        if let Some(binding) = walk(&chunk, &mut snow).with_context(|| format!("the script {name}"))? {
            let variant = if snow { IntroVariant::Snow } else { IntroVariant::Heli };
            return Ok(Some(IntroChoice { script: name.clone(), binding, variant }));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntroStart {
    pub binding: &'static str,
    pub variant: IntroVariant,
    pub rider: Option<&'static str>,
    pub every_path: bool,
    pub stop_ticks: u32,
    pub handover: [f32; 3],
    pub first: [[f32; 3]; START_FIRST],
}

pub const START_FIRST: usize = 4;

// PORT(choice): the hand-over places and the first executions' way, measured from the port's intro (ssx_physics::nis
// over the disc's root motion) and ride, not computed on the server; tuning: the server re-simulating the intro and the
// first executions (the NIS bundles and the collision's snow depth, the data classes optional-verify and
// optional-collision: phase 11's verified runs); the original has no validation.
pub const INTRO_STARTS: &[IntroStart] = &[
    row(
        "nis_binding_intro_dd_alps",
        IntroVariant::Heli,
        75,
        [-0.1695, 6.7167, 3.6687],
        [[0.0055, -0.4084, 0.1531], [0.0109, -0.8428, 0.3058], [0.0163, -1.3029, 0.4579], [0.0217, -1.7885, 0.6095]],
    ),
    rider_row(
        "nis_binding_intro_dd_alps",
        "psymon",
        true,
        40,
        [-0.1357, 4.4760, 4.6298],
        [[0.0040, -0.3909, 0.1173], [0.0080, -0.8076, 0.2342], [0.0120, -1.2502, 0.3509], [0.0160, -1.7185, 0.4673]],
    ),
    row(
        "nis_binding_intro_dd_alsk",
        IntroVariant::Heli,
        40,
        [0.1519, 1.3790, 0.9616],
        [[0.0146, -0.1346, 0.1007], [0.0292, -0.2935, 0.2013], [0.0438, -0.4766, 0.3019], [0.0630, -0.7742, 0.3613]],
    ),
    row(
        "nis_binding_intro_dd_antar",
        IntroVariant::Heli,
        40,
        [0.1572, 1.2508, 0.8511],
        [[0.0147, -0.1323, 0.1008], [0.0292, -0.2889, 0.2015], [0.0475, -0.5207, 0.2781], [0.0624, -0.7297, 0.3674]],
    ),
    row(
        "nis_binding_intro_dd_hima",
        IntroVariant::Heli,
        80,
        [0.0046, 11.1598, 3.1772],
        [
            [-0.0029, -0.1479, 0.1866],
            [-0.0057, -0.3224, 0.3724],
            [-0.0086, -0.5234986, 0.5575],
            [-0.0115, -0.7510, 0.7419],
        ],
    ),
    rider_row(
        "nis_binding_intro_dd_hima",
        "kaori",
        false,
        510,
        [0.0209, 7.8679, 5.1994],
        [[0.0008, -0.2252, 0.1121], [0.0015, -0.4769, 0.2240], [0.0023, -0.7549, 0.3355], [0.0031, -1.0591, 0.4468]],
    ),
    row(
        "nis_binding_intro_dd_kili",
        IntroVariant::Heli,
        75,
        [7.9323, 22.0172, -22.2147],
        [
            [-0.0028, -0.5607, 0.0208],
            [-0.0057, -1.1464, 0.0416],
            [-0.0085, -1.7571, 0.0624],
            [-0.0113, -2.3926, 0.0832],
        ],
    ),
    rider_row(
        "nis_binding_intro_dd_kili",
        "mac",
        false,
        288,
        [7.9350, 22.9431, -22.3031],
        [
            [-0.0011, -0.5012, 0.0216],
            [-0.0023, -1.0278, 0.0432],
            [-0.0034, -1.5797, 0.0649],
            [-0.0046, -2.1569, 0.0865],
        ],
    ),
    row(
        "nis_binding_intro_dd_lhotse",
        IntroVariant::Heli,
        75,
        [0.0326, 22.0172, 2.0260],
        [
            [-0.0028, -0.5607, 0.0209],
            [-0.0056, -1.1464, 0.0417],
            [-0.0084, -1.7571, 0.0626],
            [-0.0113, -2.3926, 0.0835],
        ],
    ),
    rider_row(
        "nis_binding_intro_dd_lhotse",
        "zoe",
        false,
        88,
        [3.9103, 21.9349, 1.9216],
        [
            [-0.0006, -0.2883, 0.0017],
            [-0.0011, -0.6029, 0.0035],
            [-0.0017, -0.9436, 0.0052],
            [-0.0022, -1.3104, 0.0069],
        ],
    ),
    row(
        "nis_binding_intro_dd_newz",
        IntroVariant::Heli,
        80,
        [-0.0192, 11.2100, 3.2284],
        [
            [-0.0029, -0.1480, 0.1866],
            [-0.0058, -0.3225, 0.3724],
            [-0.0086, -0.5235043, 0.5576],
            [-0.0115, -0.7510, 0.7419],
        ],
    ),
    rider_row(
        "nis_binding_intro_dd_newz",
        "tyler",
        false,
        315,
        [-0.0480, 10.9768, 3.7602],
        [[0.0009, -0.3053, 0.2373], [0.0017, -0.6368, 0.4734], [0.0026, -0.9944, 0.7082], [0.0034, -1.3781, 0.9419]],
    ),
    row(
        "nis_binding_intro_dd_pata",
        IntroVariant::Heli,
        80,
        [0.0077, 11.0193, 3.1237],
        [
            [-0.0029, -0.1479, 0.1866],
            [-0.0058, -0.3225, 0.3724],
            [-0.0087, -0.5235596, 0.5576],
            [-0.0115, -0.7511, 0.7420],
        ],
    ),
    rider_row(
        "nis_binding_intro_dd_pata",
        "elise",
        false,
        190,
        [0.8920, -13.1112, -2.3381],
        [[0.0000, -0.0533, 0.0000], [0.0000, -0.1333, 0.0000], [0.0000, -0.2399, 0.0000], [0.0000, -0.3730, 0.0000]],
    ),
    row(
        "nis_binding_intro_dd_roc",
        IntroVariant::Heli,
        40,
        [0.1771, 1.2277, 0.8650],
        [[0.0146, -0.1323, 0.1009], [0.0292, -0.2889, 0.2016], [0.0399, -0.5225, 0.2834], [0.0589, -0.6671, 0.4044]],
    ),
    row(
        "nis_binding_intro_dd_sib",
        IntroVariant::Heli,
        75,
        [-0.1966, 6.8167, 3.5456],
        [[0.0054, -0.4084, 0.1531], [0.0109, -0.8428, 0.3058], [0.0163, -1.3028, 0.4580], [0.0217, -1.7885, 0.6097]],
    ),
    rider_row(
        "nis_binding_intro_dd_sib",
        "alex",
        false,
        419,
        [0.0171, 5.8955, 4.4958],
        [
            [-0.0262, -0.3264, 0.0254],
            [-0.0523, -0.6788, 0.0509],
            [-0.0784, -1.0574, 0.0763],
            [-0.1044, -1.4619, 0.1017],
        ],
    ),
    row(
        "nis_binding_intro_race",
        IntroVariant::Heli,
        80,
        [0.0306, 11.2205, 3.2450],
        [
            [-0.0029, -0.1480, 0.1870],
            [-0.0057, -0.3225, 0.3735],
            [-0.0085, -0.5234985, 0.5594],
            [-0.0113, -0.7510, 0.7448],
        ],
    ),
    row(
        "nis_binding_intro_race",
        IntroVariant::Snow,
        40,
        [0.2272, 1.3006, 0.9224],
        [[0.0146, -0.1906, 0.0937], [0.0292, -0.3006, 0.1997], [0.0481, -0.5003, 0.2817], [0.0401, -0.6664, 0.3894]],
    ),
    row(
        "nis_binding_intro_redbull",
        IntroVariant::Heli,
        80,
        [0.7114, 9.7733, 9.5344],
        [
            [-0.0155, -0.3698, 0.1754],
            [-0.0314, -0.7657, 0.3500],
            [-0.0476, -1.1874, 0.5239],
            [-0.0641, -1.6349, 0.6971],
        ],
    ),
    row(
        "nis_binding_intro_survive",
        IntroVariant::Heli,
        75,
        [0.0177, 22.1799, 2.0649],
        [
            [-0.0035, -0.5606, 0.0207],
            [-0.0074, -1.1463, 0.0414],
            [-0.0117, -1.7570, 0.0620],
            [-0.0163, -2.3925, 0.0824],
        ],
    ),
    row(
        "nis_binding_intro_trick",
        IntroVariant::Heli,
        75,
        [-0.1294, 6.6753, 3.5300],
        [[0.0055, -0.4085, 0.1535], [0.0109, -0.8428, 0.3068], [0.0093, -1.3300, 0.4641], [0.0076, -1.8427, 0.6219]],
    ),
];

const fn row(
    binding: &'static str,
    variant: IntroVariant,
    stop_ticks: u32,
    handover: [f32; 3],
    first: [[f32; 3]; START_FIRST],
) -> IntroStart {
    IntroStart { binding, variant, rider: None, every_path: false, stop_ticks, handover, first }
}

const fn rider_row(
    binding: &'static str,
    rider: &'static str,
    every_path: bool,
    stop_ticks: u32,
    handover: [f32; 3],
    first: [[f32; 3]; START_FIRST],
) -> IntroStart {
    IntroStart { binding, variant: IntroVariant::Heli, rider: Some(rider), every_path, stop_ticks, handover, first }
}

// PORT(choice): 0.2065 m — the honest drops' farthest hand-overs from their intro's place 0.1792 m
// (exp_alaska_foraker_drop_4_survive: the snow below the helicopter), 0.1551 m (chapter_9_drop_h2h, elise's stance on
// the snow start), 0.1519 m (chapter_8_drop_gen_1) over 260 drops and 1 330 runs (the seven characters of interest's
// own ways each 0.0000 m from theirs): the largest plus its distance from the third (0.2129 m with three riders' runs,
// the third then 0.1455); tuning: the server computing the hand-over's place from the intro and the collision's snow
// depth (phase 11's re-simulation); the original has no validation.
pub const HANDOVER_MARGIN_M: f32 = 0.2065;

// PORT(choice): 0.0037, 0.0071, 0.0844, 0.1246 m — the helicopter intros' honest drops' farthest from their intro's way
// after 1, 2, 3 and 4 executions 0.002279 / 0.004566 / 0.056599 / 0.091038 m, the third 0.000883 / 0.002060 / 0.028841
// / 0.057554 m (235 drops, 1 207 runs, all twelve riders and the seven own ways, those within 0.0001 m; the runs,
// ridden with different inputs, part from the third execution on — that the input takes hold there is not decoded; the
// farthest from the third on: dd_alsk, dd_antar and dd_roc, the helicopter intros with the stop tag 40); tuning: the
// server re-simulating the first executions from the hand-over (the collision and the rider's input: phase 11); the
// original has no validation.
pub const FIRST_MARGINS_HELI_M: [f32; START_FIRST] = [0.0037, 0.0071, 0.0844, 0.1246];

// PORT(choice): 0.0588, 0.0122, 0.1489, 0.2467 m — the snow start's honest drops' farthest from its way after 1, 2, 3
// and 4 executions 0.058770 / 0.012074 / 0.140881 / 0.230615 m, the third 0.058766 / 0.011978 / 0.132909 / 0.214573 m
// (25 drops, 123 runs, all twelve riders; its places after 1 execution lie in two clusters 0.1175 m apart by drop,
// together after 2, so the largest three are equal there); tuning: the server re-simulating the first executions from
// the hand-over (the collision's snow and the rider's input: phase 11); the original has no validation.
pub const FIRST_MARGINS_SNOW_M: [f32; START_FIRST] = [0.0588, 0.0122, 0.1489, 0.2467];

impl IntroStart {
    pub fn of(choice: &IntroChoice) -> Option<&'static Self> {
        INTRO_STARTS.iter().find(|s| s.binding == choice.binding && s.variant == choice.variant && s.rider.is_none())
    }

    pub fn riders_of(choice: &IntroChoice) -> impl Iterator<Item = &'static Self> {
        INTRO_STARTS.iter().filter(|s| s.binding == choice.binding && s.variant == choice.variant && s.rider.is_some())
    }

    pub fn handover_execution(&self) -> u32 {
        self.stop_ticks.div_ceil(2) + 1
    }

    pub fn first_margins(&self) -> [f32; START_FIRST] {
        match self.variant {
            IntroVariant::Heli => FIRST_MARGINS_HELI_M,
            IntroVariant::Snow => FIRST_MARGINS_SNOW_M,
        }
    }

    pub fn race_start_execution(&self) -> u32 {
        self.handover_execution() + 1
    }

    pub fn handover_at(&self, slot: Vec3, rotation: Quat) -> Vec3 {
        slot + rotation * Vec3::from_array(self.handover)
    }

    pub fn in_slot_frame(p: Vec3, slot: Vec3, rotation: Quat) -> Vec3 {
        rotation.inverse() * (p - slot)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropIntro {
    pub choice: IntroChoice,
    pub start: IntroStart,
    pub riders: Vec<IntroStart>,
}

impl DropIntro {
    pub fn starts_of(&self, character: &str) -> Vec<IntroStart> {
        let own: Vec<IntroStart> = self.riders.iter().filter(|s| s.rider == Some(character)).copied().collect();
        let mut out = Vec::with_capacity(own.len() + 1);
        if !own.iter().any(|s| s.every_path) {
            out.push(self.start);
        }
        out.extend(own);
        out
    }
}

pub fn drop_intro(folder: &Path, event: &crate::events::UiDropRecord) -> Result<DropIntro> {
    let choice = intro_of_scripts(folder, &event.scripts).with_context(|| format!("{}: its intro", event.name))?;
    let Some(choice) = choice else {
        bail!("{}: its scripts play no intro at the load (no start the validator knows)", event.name);
    };
    let Some(start) = IntroStart::of(&choice) else {
        bail!(
            "{}: its intro {} ({:?}, {}) is not one the validator knows the start of",
            event.name,
            choice.binding,
            choice.variant,
            choice.script
        );
    };
    let riders = IntroStart::riders_of(&choice).copied().collect();
    Ok(DropIntro { choice, start: *start, riders })
}

const PLAY_NIS: &[u8] = b"DO_PlayNis";
const SNOW_START: &[u8] = b"BLN_NisSnowStart";

fn walk(chunk: &FuncBody, snow: &mut bool) -> Result<Option<String>> {
    let mut waited = false;
    for stat in &chunk.body {
        let call = match stat {
            Stat::Line(_) => continue,
            Stat::Call(Expr::Call(f, args, line)) => match f.as_ref() {
                Expr::Global(g) => Some((g.bytes(), args, *line)),
                _ => None,
            },
            _ => None,
        };
        let Some((f, args, line)) = call else {
            refuse_mentions_stat(stat)?;
            continue;
        };
        match f {
            b"BeginObjective" | b"EndObjective" => waited = false,
            PLAY_NIS if waited => args.iter().try_for_each(refuse_mentions)?,
            PLAY_NIS => return binding_of(args, line).map(Some),
            b"DO_SetGameStateValue" => match args.as_slice() {
                [Expr::Str(entity), Expr::Global(id), value] if id.bytes() == SNOW_START => {
                    if !waited && entity.bytes() == b"Player1" && matches!(value, Expr::True) {
                        *snow = true;
                    }
                }
                _ => args.iter().try_for_each(refuse_mentions)?,
            },
            w if w.starts_with(b"WAIT_") => {
                waited = true;
                args.iter().try_for_each(refuse_mentions)?;
            }
            _ => args.iter().try_for_each(refuse_mentions)?,
        }
    }
    Ok(None)
}

fn binding_of(args: &[Expr], line: u32) -> Result<String> {
    let [Expr::Table(items, _)] = args else { bail!("line {line}: DO_PlayNis without one table argument") };
    let mut name = None;
    for item in items {
        match item {
            TableItem::Named(k, Expr::Str(v)) if k.bytes() == b"name" => {
                if name.is_some() {
                    bail!("line {line}: DO_PlayNis names its binding twice");
                }
                name = Some(v.bytes().to_vec());
            }
            TableItem::Named(k, _) if k.bytes() == b"name" => bail!("line {line}: DO_PlayNis's name is not a string"),
            TableItem::Named(_, e) | TableItem::Pos(e) => refuse_mentions(e)?,
            TableItem::Keyed(k, e) => {
                refuse_mentions(k)?;
                refuse_mentions(e)?;
            }
        }
    }
    let name = name.with_context(|| format!("line {line}: DO_PlayNis without a name"))?;
    match String::from_utf8(name) {
        Ok(n) if (1..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => Ok(n),
        _ => bail!("line {line}: DO_PlayNis's name is not a binding name"),
    }
}

fn refuse_mentions_stat(s: &Stat) -> Result<()> {
    let exprs = |es: &[Expr]| es.iter().try_for_each(refuse_mentions);
    let block = |b: &[Stat]| b.iter().try_for_each(refuse_mentions_stat);
    match s {
        Stat::Call(e) => refuse_mentions(e),
        Stat::Local(_, es) | Stat::Return(es, _) => exprs(es),
        Stat::LocalFunction(_, f) => block(&f.body),
        Stat::Assign(ls, es) => {
            for l in ls {
                match l {
                    LValue::Global(g) => refuse_name(g.bytes())?,
                    LValue::Index(a, b, _) => {
                        refuse_mentions(a)?;
                        refuse_mentions(b)?;
                    }
                    LValue::Local(_) | LValue::Upval(_) => {}
                }
            }
            exprs(es)
        }
        Stat::Do(b) => block(b),
        Stat::While(e, b) | Stat::Repeat(b, e) => {
            refuse_mentions(e)?;
            block(b)
        }
        Stat::If(arms, other) => {
            for (e, b) in arms {
                refuse_mentions(e)?;
                block(b)?;
            }
            other.as_deref().map_or(Ok(()), block)
        }
        Stat::NumFor { start, limit, step, body, .. } => {
            refuse_mentions(start)?;
            refuse_mentions(limit)?;
            step.as_ref().map_or(Ok(()), refuse_mentions)?;
            block(body)
        }
        Stat::GenFor { exprs: es, body, .. } => {
            exprs(es)?;
            block(body)
        }
        Stat::Break(_) | Stat::Line(_) => Ok(()),
    }
}

fn refuse_name(n: &[u8]) -> Result<()> {
    if n == PLAY_NIS || n == SNOW_START {
        bail!("{} where the walk cannot place it", String::from_utf8_lossy(n));
    }
    Ok(())
}

fn refuse_mentions(e: &Expr) -> Result<()> {
    match e {
        Expr::Global(g) | Expr::Str(g) => refuse_name(g.bytes()),
        Expr::Func(f) => f.body.iter().try_for_each(refuse_mentions_stat),
        Expr::Index(a, b, _) | Expr::Bin(_, a, b, _) => {
            refuse_mentions(a)?;
            refuse_mentions(b)
        }
        Expr::Call(f, args, _) => {
            refuse_mentions(f)?;
            args.iter().try_for_each(refuse_mentions)
        }
        Expr::Method(o, m, args, _) => {
            refuse_mentions(o)?;
            refuse_name(m.bytes())?;
            args.iter().try_for_each(refuse_mentions)
        }
        Expr::Un(_, a, _) | Expr::Paren(a) => refuse_mentions(a),
        Expr::Table(items, _) => items.iter().try_for_each(|i| match i {
            TableItem::Pos(e) => refuse_mentions(e),
            TableItem::Named(k, e) => {
                refuse_name(k.bytes())?;
                refuse_mentions(e)
            }
            TableItem::Keyed(k, e) => {
                refuse_mentions(k)?;
                refuse_mentions(e)
            }
        }),
        Expr::Nil | Expr::True | Expr::False | Expr::Num(_) | Expr::Vararg | Expr::Local(_) | Expr::Upval(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intro(scripts: &[&str]) -> Result<Option<IntroChoice>> {
        let sources: Vec<(String, Vec<u8>)> =
            scripts.iter().enumerate().map(|(k, s)| (format!("s{k}.lua"), s.as_bytes().to_vec())).collect();
        intro_of_sources(&sources)
    }

    fn choice(binding: &str, variant: IntroVariant, script: &str) -> Option<IntroChoice> {
        Some(IntroChoice { script: script.into(), binding: binding.into(), variant })
    }

    #[test]
    fn the_first_load_time_play_nis_is_the_intro() {
        let heli = "BeginObjective(\"a\")\nDO_PlayNis{ name = \"nis_binding_intro_race\", snow = false }\n\
                    WAIT_ForNis()\nEndObjective()\n";
        assert_eq!(intro(&[heli]).unwrap(), choice("nis_binding_intro_race", IntroVariant::Heli, "s0.lua"));
        let snow = "BeginObjective(\"b\")\nDO_SetGameStateValue(\"Player1\", BLN_NisSnowStart, true)\nEndObjective()\n";
        assert_eq!(
            intro(&["x = 1\n", snow, heli]).unwrap(),
            choice("nis_binding_intro_race", IntroVariant::Snow, "s2.lua")
        );
        for other in [
            "DO_SetGameStateValue(\"Player2\", BLN_NisSnowStart, true)\n",
            "DO_SetGameStateValue(\"Player1\", BLN_NisSnowStart, false)\n",
            "BeginObjective(\"c\")\nWAIT_Seconds(1)\nDO_SetGameStateValue(\"Player1\", BLN_NisSnowStart, true)\n",
        ] {
            assert_eq!(intro(&[other, heli]).unwrap().map(|c| c.variant), Some(IntroVariant::Heli), "{other}");
        }
        let late = "BeginObjective(\"d\")\nWAIT_Seconds(2)\nDO_PlayNis{ name = \"nis_binding_intro_trick\" }\n\
                    EndObjective()\nBeginObjective(\"e\")\nDO_PlayNis{ name = \"nis_binding_intro_survive\" }\n";
        assert_eq!(intro(&[late]).unwrap(), choice("nis_binding_intro_survive", IntroVariant::Heli, "s0.lua"));
        assert_eq!(intro(&["y = 2\n"]).unwrap(), None, "no intro: the default level start");
    }

    #[test]
    fn a_play_nis_the_walk_cannot_place_fails() {
        for bad in [
            "if x then DO_PlayNis{ name = \"nis_binding_intro_race\" } end\n",
            "local f = function() DO_PlayNis{ name = \"nis_binding_intro_race\" } end\n",
            "local p = DO_PlayNis\n",
            "DO_PlayNis = nil\n",
            "t:DO_PlayNis()\n",
            "Foo(\"DO_PlayNis\")\n",
            "Foo{ BLN_NisSnowStart = true }\n",
            "while true do DO_SetGameStateValue(\"Player1\", BLN_NisSnowStart, true) end\n",
            "DO_PlayNis(\"nis_binding_intro_race\")\n",
            "DO_PlayNis{ name = \"a\", name = \"b\" }\n",
            "DO_PlayNis{ name = 5 }\n",
            "DO_PlayNis{ name = \"../x\" }\n",
            "DO_PlayNis{ snow = true }\n",
        ] {
            assert!(intro(&[bad]).is_err(), "{bad}");
        }
        assert!(intro(&["not lua ((("]).is_err(), "a script that does not parse");
    }

    #[test]
    fn script_names_stay_below_the_scripts_folder() {
        for good in ["eventtype_race.lua", "common/track_environment/drop_event_type.lua", "common\\a-b_c.lua"] {
            assert!(script_name(good), "{good}");
        }
        for bad in [
            "",
            "../x.lua",
            "a/../../x.lua",
            "/etc/x.lua",
            "C:\\x.lua",
            "a//b.lua",
            "x.txt",
            "Upper.lua",
            "a/./b.lua",
            "a\0.lua",
            &format!("{}.lua", "a".repeat(MAX_SCRIPT_NAME)),
        ] {
            assert!(!script_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_stop_tags_give_the_hand_overs_of_the_run_files() {
        let execution =
            |stop| row("x", IntroVariant::Heli, stop, [0.0; 3], [[0.0; 3]; START_FIRST]).handover_execution();
        assert_eq!((execution(40), execution(75), execution(80)), (21, 39, 41));
        assert_eq!(row("x", IntroVariant::Heli, 80, [0.0; 3], [[0.0; 3]; START_FIRST]).race_start_execution(), 42);
        for (k, a) in INTRO_STARTS.iter().enumerate() {
            assert!(
                INTRO_STARTS[..k].iter().all(|b| (b.binding, b.variant, b.rider) != (a.binding, a.variant, a.rider)),
                "{}",
                a.binding
            );
            assert!(INTRO_STARTS.iter().any(|b| (b.binding, b.variant, b.rider) == (a.binding, a.variant, None)));
            let (slot, q) =
                (Vec3::new(298.2, 1035.7, -868.7), Quat::from_xyzw(0.0, 0.72472, 0.0, -0.68904).normalize());
            let world = a.handover_at(slot, q);
            assert!(IntroStart::in_slot_frame(world, slot, q).distance(Vec3::from_array(a.handover)) < 1e-3);
        }
    }
}
