use std::fmt::{self, Write as _};

use tracing::field::{Field, Visit};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::fmt::format::Writer;

use super::{Bounded, Mode};

pub const FIELD_CHARS: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct EscapedFields;

impl<'writer> FormatFields<'writer> for EscapedFields {
    fn format_fields<R: RecordFields>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result {
        let mut v = Visitor { writer, first: true, result: Ok(()) };
        fields.record(&mut v);
        v.result
    }
}

struct Visitor<'a> {
    writer: Writer<'a>,
    first: bool,
    result: fmt::Result,
}

impl Visitor<'_> {
    fn field(&mut self, field: &Field, value: fmt::Arguments<'_>) {
        let name = field.name();
        if self.result.is_err() || name.starts_with("log.") {
            return;
        }
        self.result = self.write(name, value);
    }

    fn write(&mut self, name: &str, value: fmt::Arguments<'_>) -> fmt::Result {
        if !std::mem::take(&mut self.first) {
            self.writer.write_char(' ')?;
        }
        if name != "message" {
            write!(self.writer, "{}=", super::shown_max(name.strip_prefix("r#").unwrap_or(name), 64))?;
        }
        let mut b = Bounded::new(&mut self.writer, FIELD_CHARS, Mode::Plain);
        b.write_fmt(value)?;
        b.finish()
    }
}

impl Visit for Visitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.field(field, format_args!("{value}"));
        } else {
            self.field(field, format_args!("{value:?}"));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.field(field, format_args!("{}", super::one_line(value)));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.field(field, format_args!("{value:?}"));
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct QueuedStderr;

#[derive(Clone, Copy, Debug, Default)]
pub struct QueuedStdout;

pub struct LineWriter {
    stream: crate::output::Stream,
    keep: crate::output::Keep,
    line: Vec<u8>,
}

impl LineWriter {
    fn new(stream: crate::output::Stream, meta: Option<&tracing::Metadata<'_>>) -> Self {
        let keep = match meta {
            Some(m) if *m.level() > tracing::Level::WARN => crate::output::Keep::Droppable,
            _ => crate::output::Keep::Always,
        };
        LineWriter { stream, keep, line: Vec::new() }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for QueuedStderr {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> LineWriter {
        LineWriter::new(crate::output::Stream::Err, None)
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> LineWriter {
        LineWriter::new(crate::output::Stream::Err, Some(meta))
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for QueuedStdout {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> LineWriter {
        LineWriter::new(crate::output::Stream::Out, None)
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> LineWriter {
        LineWriter::new(crate::output::Stream::Out, Some(meta))
    }
}

impl std::io::Write for LineWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.line.extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if !self.line.is_empty() {
            let _ = crate::output::line(self.stream, self.keep, std::mem::take(&mut self.line));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use super::EscapedFields;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().map_err(|_| io::Error::other("poisoned"))?.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn logged(emit: impl FnOnce()) -> String {
        let sink = Sink::default();
        let out = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .fmt_fields(EscapedFields)
            .with_writer(move || out.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, emit);
        let bytes = sink.0.lock().map(|b| b.clone()).unwrap_or_default();
        String::from_utf8(bytes).unwrap_or_default()
    }

    #[test]
    fn a_hostile_message_and_field_reach_the_log_escaped_and_bounded() {
        let hostile = "\u{1b}]0;TITLE\u{7}\u{1b}[2J\u{9b}31m\u{202e}evil\u{200b}\r\nINFO forged line";
        let long = "k".repeat(1 << 20);
        let line = logged(|| {
            tracing::warn!(package = hostile, "mods: skipped {hostile}: {long}");
        });
        assert_eq!(line.matches('\n').count(), 1, "one line: {line:?}");
        assert!(!line.trim_end_matches('\n').chars().any(super::super::is_hidden), "{line:?}");
        assert!(
            line.contains("mods: skipped \\u{1b}]0;TITLE\\u{7}\\u{1b}[2J\\u{9b}31m\\u{202e}evil\\u{200b}\\r\\nINFO")
        );
        assert!(line.len() < super::FIELD_CHARS + 512, "{} bytes", line.len());
        assert!(line.contains("bytes)"), "the cut says how long the field was");
        assert!(line.contains("package=\"\\u{1b}]0;TITLE"), "{line:?}");
    }

    #[test]
    fn an_error_field_is_escaped_with_its_causes() {
        let e = io::Error::other("bad \u{1b}[31m name\nnext");
        let line = logged(|| tracing::error!(error = &e as &(dyn std::error::Error + 'static), "failed"));
        assert!(line.contains("failed error=bad \\u{1b}[31m name\\nnext"), "{line:?}");
    }
}
