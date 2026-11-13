use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ssx_content::check::{Checked, Severity};
use ssx_content::manifest::Manifest;

use super::check::push;

const HELPER_WAIT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: u64 = 4 << 20;

pub fn helper_path(given: Option<&Path>) -> PathBuf {
    beside("ssx-script-check", given)
}

pub(super) fn beside(name: &str, given: Option<&Path>) -> PathBuf {
    if let Some(p) = given {
        return p.to_path_buf();
    }
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(&exe))).unwrap_or_else(|| PathBuf::from(exe))
}

pub(super) fn run_helper(
    exe: &Path,
    args: &[&OsStr],
    wait: Duration,
) -> Result<(String, std::process::ExitStatus), String> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no output pipe")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(MAX_OUTPUT).read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if start.elapsed() > wait => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("it did not finish within {} s and was stopped", wait.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let bytes = reader.join().map_err(|_| "its output reader failed".to_string())?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), status))
}

pub(super) fn check(c: &mut Checked, m: &Manifest, helper: Option<&Path>) {
    let s = &m.scripts;
    if s.client.is_empty() && s.server.is_empty() && s.shared.is_empty() {
        return;
    }
    let Some(root) = c.tree.as_ref().map(|t| t.root().to_path_buf()) else { return };
    let exe = helper_path(helper);
    let (text, status) = match run_helper(&exe, &[root.as_os_str()], HELPER_WAIT) {
        Ok(r) => r,
        Err(e) => {
            let message = format!(
                "the scripts' check needs {} ({e}): build it with `cargo build --release -p ssx-script --bin \
                 ssx-script-check`, or name it with --script-check",
                exe.display()
            );
            return push(c, Severity::Error, "", (0, 0), message);
        }
    };
    let errors = push_findings(c, &text, "script: ");
    if !status.success() && errors == 0 {
        push(c, Severity::Error, "", (0, 0), format!("the scripts' check failed: {status}"));
    }
}

pub(super) fn push_findings(c: &mut Checked, text: &str, prefix: &str) -> usize {
    let mut errors = 0;
    for l in text.lines() {
        let parts: Vec<&str> = l.splitn(5, '\t').collect();
        let [severity, file, line, column, message] = parts.as_slice() else { continue };
        let severity = match *severity {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            "note" => Severity::Note,
            _ => continue,
        };
        errors += usize::from(severity == Severity::Error);
        let at = (line.parse().unwrap_or(0), column.parse().unwrap_or(0));
        push(c, severity, file, at, format!("{prefix}{message}"));
    }
    errors
}
