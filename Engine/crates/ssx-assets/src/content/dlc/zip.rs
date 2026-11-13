use std::fs::File;
use std::io::{BufReader, Write};

use ssx_content::zip::{ZipArchive, ZipEntry, ZipLimits, ZipNames};

const LIMITS: ZipLimits = ZipLimits {
    max_archive_bytes: 2 << 30,
    max_entries: 4096,
    max_file_bytes: 1 << 30,
    max_total_bytes: 8 << 30,
    max_ratio: 200,
    ratio_floor: 1 << 20,
    names: ZipNames::PlayerDump,
};

pub struct ZipFile {
    archive: ZipArchive<BufReader<File>>,
}

impl ZipFile {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let archive =
            ZipArchive::open(BufReader::new(file), &LIMITS).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(ZipFile { archive })
    }

    pub fn entries(&self) -> &[ZipEntry] {
        self.archive.entries()
    }

    pub fn extract(&mut self, i: usize, out: &mut dyn Write) -> Result<(), String> {
        self.archive.extract(i, out).map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_players_dump_and_refuses_a_damaged_byte() {
        use ssx_content::zip::{PackFile, write_zip};
        let files = [("DLC- 4541096D/a", b"hello".to_vec()), ("Title Update/b", b"the dlc package bytes".repeat(3))];
        let mut zip = Vec::new();
        let it = files.iter().map(|(n, b)| Ok(PackFile { name: n.replace(' ', "_"), bytes: b.clone() }));
        write_zip(&mut zip, it, &ZipLimits::default()).unwrap();
        for (name, _) in &files {
            let written = name.replace(' ', "_");
            for at in 0..zip.len() - written.len() {
                if zip[at..at + written.len()] == *written.as_bytes() {
                    zip[at..at + name.len()].copy_from_slice(name.as_bytes());
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("ssx-dlc-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dump.zip");
        std::fs::write(&path, &zip).unwrap();
        let mut z = ZipFile::open(&path).unwrap();
        assert_eq!(z.entries().len(), 2);
        assert_eq!(z.entries()[0].name, "DLC- 4541096D/a");
        let mut out = Vec::new();
        z.extract(1, &mut out).unwrap();
        assert_eq!(out, b"the dlc package bytes".repeat(3));
        let at = zip.windows(5).position(|w| w == b"hello").unwrap();
        zip[at] ^= 0xFF;
        std::fs::write(&path, &zip).unwrap();
        let mut z = ZipFile::open(&path).unwrap();
        assert!(z.extract(0, &mut std::io::sink()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
