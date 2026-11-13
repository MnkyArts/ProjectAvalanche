use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::app::App;
use bevy::ecs::resource::Resource;
use bevy::log::tracing::Metadata;
use bevy::log::tracing::callsite::Identifier;
use bevy::log::tracing::subscriber::Interest;
use bevy::log::tracing_subscriber::Layer;
use bevy::log::tracing_subscriber::fmt::MakeWriter;
use bevy::log::tracing_subscriber::layer::{Context, Filter};
use bevy::log::{BoxedFmtLayer, BoxedLayer, tracing_subscriber};

pub const LOG_FOLDER: &str = "logs";

// PORT(choice): 16 MiB per file; tuning: the log volume of phase 4's servers over a week; the original has no dedicated
// server.
pub const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

// PORT(choice): 20 a second with a burst of 100 per call site; tuning: the busiest legitimate call site of phase 2's
// sessions (joins and kicks of a full server); the original has no dedicated server.
pub const LINES_PER_SECOND: f64 = 20.0;
pub const LINE_BURST: f64 = 100.0;

pub static SUPPRESSED_LINES: AtomicU64 = AtomicU64::new(0);

#[derive(Resource, Clone)]
pub struct LogFile(pub PathBuf);

struct RateLimit {
    buckets: Mutex<HashMap<Identifier, (f64, Instant)>>,
    count_suppressed: bool,
}

impl RateLimit {
    fn new(count_suppressed: bool) -> Self {
        Self { buckets: Mutex::new(HashMap::new()), count_suppressed }
    }
}

impl<S> Filter<S> for RateLimit {
    fn enabled(&self, meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        if !meta.is_event() || meta.target() == crate::systems::CHAT_LOG {
            return true;
        }
        let now = Instant::now();
        let Ok(mut buckets) = self.buckets.lock() else { return true };
        let (tokens, at) = buckets.entry(meta.callsite()).or_insert((LINE_BURST, now));
        *tokens = (*tokens + now.saturating_duration_since(*at).as_secs_f64() * LINES_PER_SECOND).min(LINE_BURST);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            if self.count_suppressed {
                SUPPRESSED_LINES.fetch_add(1, Ordering::Relaxed);
            }
            false
        }
    }

    fn callsite_enabled(&self, _meta: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }
}

struct RotatingFile {
    path: PathBuf,
    state: Mutex<(Option<File>, u64)>,
}

impl RotatingFile {
    fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self { path: path.to_path_buf(), state: Mutex::new((Some(file), size)) })
    }

    fn write_line(&self, buf: &[u8]) -> std::io::Result<usize> {
        let Ok(mut state) = self.state.lock() else { return Ok(buf.len()) };
        if state.1 + buf.len() as u64 > MAX_LOG_BYTES {
            state.0 = None;
            let mut old = self.path.clone().into_os_string();
            old.push(".1");
            std::fs::rename(&self.path, &old)?;
            state.0 = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
            state.1 = 0;
        }
        match state.0.as_mut() {
            Some(file) => {
                file.write_all(buf)?;
                state.1 += buf.len() as u64;
                Ok(buf.len())
            }
            None => Ok(buf.len()),
        }
    }
}

struct FileWriter(Arc<RotatingFile>);

impl Write for FileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_line(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct MakeFileWriter(Arc<RotatingFile>);

impl<'a> MakeWriter<'a> for MakeFileWriter {
    type Writer = FileWriter;

    fn make_writer(&'a self) -> FileWriter {
        FileWriter(Arc::clone(&self.0))
    }
}

pub fn file_layer(app: &mut App) -> Option<BoxedLayer> {
    let path = app.world().get_resource::<LogFile>()?.0.clone();
    let file = match RotatingFile::open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("the log file {} does not open: {e}; logging to stdout only", path.display());
            return None;
        }
    };
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .fmt_fields(ssx_core::text::log::EscapedFields)
        .with_writer(MakeFileWriter(Arc::new(file)))
        .with_filter(RateLimit::new(true));
    Some(Box::new(layer))
}

pub fn stdout_layer(_app: &mut App) -> Option<BoxedFmtLayer> {
    use std::io::IsTerminal;
    let ansi = std::io::stdout().is_terminal();
    Some(Box::new(
        tracing_subscriber::fmt::layer()
            .with_ansi(ansi)
            .fmt_fields(ssx_core::text::log::EscapedFields)
            .with_writer(std::io::stdout)
            .with_filter(RateLimit::new(false)),
    ))
}
