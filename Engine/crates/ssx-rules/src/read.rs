use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

// PORT(choice): 64 MiB, a port limit about ten times the disc's largest rules file (6 484 944 bytes); a folder whose
// file is larger is refused, so a damaged or hostile data folder cannot make the reader hold more; tuning: the largest
// rules file an operator's disc or a later data format needs; the original has no dedicated server.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    let mut buf = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).with_context(|| format!("reading {}", path.display()))?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        bail!("{}: larger than {MAX_FILE_BYTES} bytes", path.display());
    }
    Ok(buf)
}

fn open_regular(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).with_context(|| format!("reading {}", path.display()))?;
    let meta = file.metadata().with_context(|| format!("reading {}", path.display()))?;
    if !meta.is_file() {
        bail!("{}: not a regular file", path.display());
    }
    if meta.len() > MAX_FILE_BYTES {
        bail!("{}: {} bytes, more than {MAX_FILE_BYTES}", path.display(), meta.len());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_regular_files_are_read() {
        let dir = std::env::temp_dir().join(format!("ssx-rules-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_file(&dir).unwrap_err().to_string().contains("not a regular file"));
        assert!(read_file(&dir.join("missing")).is_err());
        let f = dir.join("f.vlt");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(read_file(&f).unwrap(), b"abc");
        #[cfg(unix)]
        {
            let fifo = dir.join("fifo.vlt");
            let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert!(read_file(&fifo).unwrap_err().to_string().contains("not a regular file"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
