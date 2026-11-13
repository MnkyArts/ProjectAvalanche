use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ssx_content::check::{Checked, Severity};

use super::check::push;
use super::scripts::{beside, push_findings, run_helper};

const HELPER_WAIT: Duration = Duration::from_secs(300);

pub fn helper_path(given: Option<&Path>) -> PathBuf {
    beside("ssx-map-check", given)
}

pub(super) fn check(c: &mut Checked, big: &Path, others: &[PathBuf], helper: Option<&Path>) {
    let ids: Vec<String> = c.maps.keys().map(ToString::to_string).collect();
    if ids.is_empty() {
        return;
    }
    let Some(root) = c.tree.as_ref().map(|t| t.root().to_path_buf()) else { return };
    let exe = helper_path(helper);
    for id in ids {
        let mut args: Vec<&OsStr> =
            vec![root.as_os_str(), OsStr::new("--data"), big.as_os_str(), OsStr::new("--map-id"), OsStr::new(&id)];
        for dir in others {
            args.extend([OsStr::new("--with"), dir.as_os_str()]);
        }
        let (text, status) = match run_helper(&exe, &args, HELPER_WAIT) {
            Ok(r) => r,
            Err(e) => {
                let message = format!(
                    "the start of the map {id} is checked by {} ({e}): build it with `cargo build --release -p \
                     ssx-physics --bin ssx-map-check`, or name it with --map-check",
                    exe.display()
                );
                push(c, Severity::Error, "", (0, 0), message);
                continue;
            }
        };
        let errors = push_findings(c, &text, "");
        if !status.success() && errors == 0 {
            push(c, Severity::Error, "", (0, 0), format!("the start check of the map {id} failed: {status}"));
        }
    }
}
