use bevy::log::tracing;
use bevy::log::tracing_subscriber::layer::SubscriberExt;

use super::*;

fn temp_log(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ssx-server-log-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join(LOG_FOLDER).join("server.log")
}

fn with_file_layer(log: &LogFile, emit: impl FnOnce()) {
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .fmt_fields(ssx_core::text::log::EscapedFields)
        .with_writer(FileLines(Arc::clone(&log.0)));
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, emit);
}

#[test]
fn the_file_log_is_written_by_its_thread_and_kept_to_the_last_line() {
    let path = temp_log("thread");
    let (running, log) = start_file(&path).expect("the log file opens");
    assert!(running.is_some(), "the writer thread started");
    with_file_layer(&log, || {
        for i in 0..2_000 {
            tracing::warn!("line {i} of the server's log");
        }
    });
    drop(running);
    let text = std::fs::read_to_string(&path).expect("the log file");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2_000, "every line in the file");
    assert!(lines[0].contains("line 0 of the server's log") && lines[1_999].contains("line 1999 of"));
    with_file_layer(&log, || tracing::warn!("a line after the writer ended"));
    let text = std::fs::read_to_string(&path).expect("the log file");
    assert!(text.lines().last().is_some_and(|l| l.contains("a line after the writer ended")), "{text}");
    let _ = std::fs::remove_dir_all(path.parent().and_then(Path::parent).expect("the test's folder"));
}

#[test]
fn a_flush_waits_for_the_file_logs_lines() {
    let path = temp_log("flush");
    let (running, log) = start_file(&path).expect("the log file opens");
    with_file_layer(&log, || {
        for i in 0..500 {
            tracing::error!("before the panic {i}");
        }
    });
    ssx_core::output::flush();
    let text = std::fs::read_to_string(&path).expect("the log file");
    assert_eq!(text.lines().filter(|l| l.contains("before the panic")).count(), 500);
    drop(running);
    let _ = std::fs::remove_dir_all(path.parent().and_then(Path::parent).expect("the test's folder"));
}

#[test]
fn a_panic_waits_for_the_lines_before_it() {
    let path = temp_log("panic");
    let (running, log) = start_file(&path).expect("the log file opens");
    let log2 = log.clone();
    let panicked = std::thread::spawn(move || {
        with_file_layer(&log2, || {
            for i in 0..500 {
                tracing::error!("said before the panic {i}");
            }
        });
        panic!("a test's panic after its log lines");
    })
    .join();
    assert!(panicked.is_err(), "the thread panicked");
    let text = std::fs::read_to_string(&path).expect("the log file");
    assert_eq!(text.lines().filter(|l| l.contains("said before the panic")).count(), 500);
    with_file_layer(&log, || tracing::error!("after the panic"));
    drop(running);
    assert!(std::fs::read_to_string(&path).expect("the log file").contains("after the panic"));
    let _ = std::fs::remove_dir_all(path.parent().and_then(Path::parent).expect("the test's folder"));
}

struct Keeps(Arc<FileLog>, Arc<Mutex<Vec<Keep>>>);

impl<S: bevy::log::tracing::Subscriber> Layer<S> for Keeps {
    fn on_event(&self, event: &bevy::log::tracing::Event<'_>, _cx: Context<'_, S>) {
        let line = FileLines(Arc::clone(&self.0)).make_writer_for(event.metadata());
        if let Ok(mut k) = self.1.lock() {
            k.push(line.keep);
        }
    }
}

#[test]
fn the_chat_record_and_warnings_are_kept_the_rest_may_be_left_out() {
    let path = temp_log("keep");
    let (running, log) = start_file(&path).expect("the log file opens");
    let keeps = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(Keeps(Arc::clone(&log.0), Arc::clone(&keeps)));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("a join");
        tracing::info!(target: crate::systems::CHAT_LOG, "chat: 1 [ab12] Elise: hello");
        tracing::warn!("a stall");
        tracing::error!("an error");
        tracing::debug!("a detail");
    });
    let keeps = keeps.lock().map(|k| k.clone()).unwrap_or_default();
    assert_eq!(keeps, [Keep::Droppable, Keep::Always, Keep::Always, Keep::Always, Keep::Droppable]);
    drop(running);
    let _ = std::fs::remove_dir_all(path.parent().and_then(Path::parent).expect("the test's folder"));
}
