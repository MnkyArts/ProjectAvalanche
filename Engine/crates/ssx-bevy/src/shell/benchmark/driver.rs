use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

use super::report::{FORMAT, Outcome, Part, Report, SceneReport};
use super::{SCENES, Scene, scene, scene_names};

const SCENE_TIMEOUT: Duration = Duration::from_secs(900);
const POLL: Duration = Duration::from_millis(200);
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DriverArgs {
    pub scenes: Vec<String>,
    pub out: Option<PathBuf>,
    pub forwarded: Vec<OsString>,
}

pub fn run(args: DriverArgs) -> Result<(PathBuf, bool)> {
    let scenes: Vec<&'static Scene> = if args.scenes.is_empty() {
        SCENES.iter().collect()
    } else {
        args.scenes
            .iter()
            .map(|id| scene(id).with_context(|| format!("--benchmark {id}: no such scene (scenes: {})", scene_names())))
            .collect::<Result<_>>()?
    };
    let started = SystemTime::now();
    let stamp = utc(started);
    let out = args.out.clone().unwrap_or_else(|| {
        ssx_assets::setup::app_dir().join("Benchmarks").join(format!("benchmark-{}.json", stamp.file_stamp()))
    });
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("the report's folder {}", dir.display()))?;
    }
    let exe = std::env::current_exe().context("the benchmark starts the game's own program: its path")?;
    let work = std::env::temp_dir().join(format!("avalanche-benchmark-{}", std::process::id()));
    if work.exists() {
        std::fs::remove_dir_all(&work).with_context(|| format!("the scenes' old folder {}", work.display()))?;
    }
    std::fs::create_dir_all(&work).with_context(|| format!("the scenes' folder {}", work.display()))?;
    ssx_core::println!("benchmark: {} scene(s), the report to {}", scenes.len(), out.display());
    let mut reports = Vec::new();
    let mut machine = None;
    for s in &scenes {
        let begin = Instant::now();
        let part = run_scene(&exe, &args.forwarded, &work, s);
        let (report, m) = match part {
            Ok(p) => (p.scene, p.machine),
            Err(e) => (
                SceneReport {
                    id: s.id.into(),
                    title: s.title.into(),
                    outcome: Outcome::Failed { reason: format!("{e:#}") },
                },
                None,
            ),
        };
        let what = match &report.outcome {
            Outcome::Measured(m) => format!(
                "{} frames, {:.2} ms a frame ({:.1} fps), 1 % low {:.1} fps",
                m.frame.frames, m.frame.frame_ms.avg, m.frame.fps, m.frame.low_1_fps
            ),
            Outcome::Skipped { reason } => format!("skipped: {reason}"),
            Outcome::Failed { reason } => format!("FAILED: {reason}"),
        };
        ssx_core::println!("benchmark: scene {} ({:.0} s): {what}", s.id, begin.elapsed().as_secs_f32());
        if machine.is_none() {
            machine = m;
        }
        reports.push(report);
    }
    let measured = reports.iter().any(|r| matches!(r.outcome, Outcome::Measured(_)));
    let home = home_dir();
    let private = |s: String| match &home {
        Some(h) if !h.is_empty() => s.replace(h.as_str(), "~"),
        _ => s,
    };
    let report = Report {
        format: FORMAT,
        program: "Project Avalanche".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        started: stamp.text(),
        machine,
        options: args.forwarded.iter().map(|a| private(a.to_string_lossy().into_owned())).collect(),
        switches: std::env::vars()
            .filter(|(k, _)| k.starts_with("SSX_") || k.starts_with("WGPU_"))
            .map(|(k, v)| private(format!("{k}={v}")))
            .collect(),
        scenes: reports,
    };
    let json = serde_json::to_vec(&report).context("the report as JSON")?;
    ssx_files::replace(&out, &json, &ssx_files::Options::new())
        .with_context(|| format!("the report {}", out.display()))?;
    let text = report.text();
    let text_path = out.with_extension("txt");
    ssx_files::replace(&text_path, text.as_bytes(), &ssx_files::Options::new())
        .with_context(|| format!("the report {}", text_path.display()))?;
    for line in text.lines() {
        ssx_core::println!("{line}");
    }
    ssx_core::println!("benchmark: the report: {} and {}", out.display(), text_path.display());
    if let Err(e) = std::fs::remove_dir_all(&work) {
        ssx_core::eprintln!("benchmark: the scenes' folder {} stays: {e}", work.display());
    }
    Ok((out, measured))
}

fn run_scene(exe: &Path, forwarded: &[OsString], work: &Path, s: &Scene) -> Result<Part> {
    let dir = work.join(s.id);
    std::fs::create_dir_all(&dir).with_context(|| format!("the scene's folder {}", dir.display()))?;
    let part = work.join(format!("{}.json", s.id));
    let mut command = Command::new(exe);
    command
        .args(forwarded)
        .arg("--benchmark-scene")
        .arg(s.id)
        .arg("--benchmark-part")
        .arg(&part)
        .arg("--save")
        .arg(dir.join("ssxvprofile"))
        .env_remove("SSX_PERF");
    if std::env::var_os("SSX_RANDOM_SEED").is_none() {
        command.env("SSX_RANDOM_SEED", "1");
    }
    let mut child = command.spawn().with_context(|| format!("starting {}", exe.display()))?;
    let begin = Instant::now();
    let status: ExitStatus = loop {
        if let Some(status) = child.try_wait().context("waiting for the scene's process")? {
            break status;
        }
        if begin.elapsed() > SCENE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the scene's process did not end within {} s and was stopped", SCENE_TIMEOUT.as_secs());
        }
        std::thread::sleep(POLL);
    };
    let read = read_part(&part, s);
    match (read, status.success()) {
        (Ok(p), _) => Ok(p),
        (Err(e), true) => Err(e),
        (Err(e), false) => Err(e.context(format!("the scene's process ended with {status}"))),
    }
}

fn read_part(path: &Path, s: &Scene) -> Result<Part> {
    let size = std::fs::metadata(path).with_context(|| format!("no result from the scene ({})", path.display()))?.len();
    if size > MAX_PART_BYTES {
        bail!("the scene's result is {size} bytes (at most {MAX_PART_BYTES})");
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let part: Part =
        serde_json::from_slice(&bytes).with_context(|| format!("the scene's result {}", path.display()))?;
    if part.format != FORMAT || part.scene.id != s.id {
        bail!("the result names scene {:?} in format {} (expected {:?}, {FORMAT})", part.scene.id, part.format, s.id);
    }
    Ok(part)
}

fn home_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|h| h.len() > 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Utc {
    fn text(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    fn file_stamp(&self) -> String {
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }
}

pub fn utc(t: SystemTime) -> Utc {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    Utc { year, month, day, hour: (rest / 3600) as u32, minute: (rest % 3600 / 60) as u32, second: (rest % 60) as u32 }
}

pub fn forwarded(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut it = args.into_iter().peekable();
    while let Some(a) = it.next() {
        let text = a.to_string_lossy();
        let name = text.split_once('=').map_or(text.as_ref(), |(n, _)| n);
        let with_value = text.contains('=');
        match name {
            "--benchmark" => {
                if !with_value {
                    while it.peek().is_some_and(|v| !v.to_string_lossy().starts_with('-')) {
                        it.next();
                    }
                }
            }
            "--out" | "--save" => {
                if !with_value {
                    it.next();
                }
            }
            _ => out.push(a),
        }
    }
    out
}
