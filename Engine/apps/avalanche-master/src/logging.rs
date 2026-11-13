use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing::Metadata;
use tracing::callsite::Identifier;
use tracing::subscriber::Interest;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Filter, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

// PORT(choice): 16 MiB per file, the server's; tuning: a public master's log volume over a week; the original has no
// master server.
pub const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
// PORT(choice): 20 a second with a burst of 100 per call site, the server's; tuning: the busiest legitimate call site
// of a public master (listings and checks of servers starting at once); the original has no master server.
pub const LINES_PER_SECOND: f64 = 20.0;
pub const LINE_BURST: f64 = 100.0;

pub static SUPPRESSED_LINES: AtomicU64 = AtomicU64::new(0);

struct RateLimit {
    buckets: Mutex<HashMap<Identifier, (f64, Instant)>>,
    count_suppressed: bool,
}

impl<S> Filter<S> for RateLimit {
    fn enabled(&self, meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        if !meta.is_event() {
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

pub fn init(file: Option<&Path>) {
    use std::io::IsTerminal;
    let ansi = std::io::stdout().is_terminal();
    let stdout = tracing_subscriber::fmt::layer()
        .with_ansi(ansi)
        .fmt_fields(ssx_core::text::log::EscapedFields)
        .with_writer(std::io::stdout)
        .with_filter(RateLimit { buckets: Mutex::new(HashMap::new()), count_suppressed: false });
    let file = file.and_then(|path| match RotatingFile::open(path) {
        Ok(f) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .fmt_fields(ssx_core::text::log::EscapedFields)
                .with_writer(MakeFileWriter(Arc::new(f)))
                .with_filter(RateLimit { buckets: Mutex::new(HashMap::new()), count_suppressed: true }),
        ),
        Err(e) => {
            eprintln!("the log file {} does not open: {e}; logging to stdout only", path.display());
            None
        }
    });
    let _ = tracing_subscriber::registry().with(LevelFilter::INFO).with(stdout).with(file).try_init();
}
