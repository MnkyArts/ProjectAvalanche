use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::save::chunks::TuningChunk;
use ssx_formats::save::{self, PROFILE_BUFFER_SIZE, ProfileFile};
use ssx_formats::tuner::TunerPack;

pub const TUNER_DIR: &str = "Tuner";
pub const TUNER_FILE: &str = "tuner.bin";
pub const MAX_FILE_BYTES: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    SaveFile,
    SaveBuffer,
    Pack,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::SaveFile => "level-15 save (save library file)",
            Kind::SaveBuffer => "level-15 save (profile buffer)",
            Kind::Pack => "tuner pack",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub kind: Kind,
    pub bytes: usize,
    pub source: String,
    pub pack: TunerPack,
}

impl Loaded {
    pub fn line(&self) -> String {
        format!(
            "tuner: {} ({} bytes, {}): version {}, {} collections, EB archive {} bytes",
            self.kind.text(),
            self.bytes,
            self.source,
            self.pack.version,
            self.pack.collections().unwrap_or(0),
            self.pack.archive_size
        )
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let meta = file.metadata().with_context(|| format!("reading {}", path.display()))?;
    if !meta.is_file() {
        bail!("{}: not a file", path.display());
    }
    if meta.len() > MAX_FILE_BYTES {
        bail!("{}: {} bytes, at most {MAX_FILE_BYTES} are read", path.display(), meta.len());
    }
    let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0).min(MAX_FILE_BYTES as usize));
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut data).with_context(|| format!("reading {}", path.display()))?;
    if data.len() as u64 > MAX_FILE_BYTES {
        bail!("{}: more than {MAX_FILE_BYTES} bytes", path.display());
    }
    Ok(data)
}

pub fn parse(data: &[u8]) -> Result<Option<Loaded>> {
    let is_save = data.starts_with(save::PROFILE_HEADER_NAME) || data.len() >= PROFILE_BUFFER_SIZE;
    if is_save {
        let (kind, buffer) = if data.starts_with(save::PROFILE_HEADER_NAME) {
            (Kind::SaveFile, save::buffer(data)?)
        } else {
            (Kind::SaveBuffer, save::buffer(data)?)
        };
        let file = ProfileFile::parse(buffer)?;
        let chunk = file.chunks.get(7).with_context(|| "a profile without chunk 7 (the tuning cache)")?;
        let tuning = TuningChunk::parse(chunk)?;
        if tuning.unk_word == 0 && tuning.buffer.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        let pack = TunerPack::parse(&tuning.write())?;
        Ok(Some(Loaded { kind, bytes: data.len(), source: String::new(), pack }))
    } else {
        let pack = TunerPack::parse(data)?;
        Ok(Some(Loaded { kind: Kind::Pack, bytes: data.len(), source: String::new(), pack }))
    }
}

pub fn read(path: &Path) -> Result<Option<Loaded>> {
    let data = read_bounded(path)?;
    let mut loaded = parse(&data).with_context(|| format!("{}", path.display()))?;
    if let Some(l) = &mut loaded {
        l.source = path.display().to_string();
    }
    Ok(loaded)
}

pub fn load(explicit: Option<&Path>, profile: Option<&Path>) -> Result<Option<Loaded>> {
    if let Some(path) = explicit {
        let file = if path.is_dir() { path.join(TUNER_FILE) } else { path.to_path_buf() };
        if !file.is_file() {
            return Ok(None);
        }
        return read(&file).with_context(|| format!("--tuner {}", file.display()));
    }
    let Some(profile) = profile.filter(|p| p.is_file()) else { return Ok(None) };
    read(profile).with_context(|| format!("the profile {} (chunk 7)", profile.display()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub loaded: Loaded,
    pub target: PathBuf,
    pub profile: Option<PathBuf>,
}

impl Report {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{}; installed as {}", self.loaded.line(), self.target.display())];
        if let Some(p) = &self.profile {
            lines.push(format!("the player's profile copied to {}", p.display()));
        }
        lines
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    Exists { tuner: bool, profile: bool },
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Exists { tuner: true, profile: true } => {
                write!(f, "a tuner file and the player's profile are installed already")
            }
            InstallError::Exists { tuner: true, profile: false } => write!(f, "a tuner file is installed already"),
            InstallError::Exists { tuner: false, profile: true } => {
                write!(f, "the player's profile is installed already")
            }
            InstallError::Exists { tuner: false, profile: false } => write!(f, "the files are installed already"),
            InstallError::Failed(e) => f.write_str(e),
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn installed(dir: &Path) -> Option<PathBuf> {
    let path = dir.join(TUNER_FILE);
    path.is_file().then_some(path)
}

pub fn install(source: &Path, dir: &Path, profile: Option<&Path>, replace: bool) -> Result<Report, InstallError> {
    let data = read_bounded(source).map_err(|e| InstallError::Failed(format!("{e:#}")))?;
    let loaded = parse(&data).map_err(|e| InstallError::Failed(format!("{e:#}")))?;
    let Some(loaded) = loaded else {
        return Err(InstallError::Failed(format!(
            "{}: the save has no tuner pack in chunk 7 (a profile without one keeps 10 levels)",
            source.display()
        )));
    };
    let target = dir.join(TUNER_FILE);
    let profile_target =
        profile.filter(|_| matches!(loaded.kind, Kind::SaveFile | Kind::SaveBuffer)).filter(|p| !same_file(source, p));
    let tuner_exists = target.exists() && !same_file(source, &target);
    let profile_exists = profile_target.is_some_and(|p| p.exists());
    if !replace && (tuner_exists || profile_exists) {
        return Err(InstallError::Exists { tuner: tuner_exists, profile: profile_exists });
    }
    if !same_file(source, &target) {
        ssx_files::create_dir_all(dir).map_err(|e| InstallError::Failed(e.to_string()))?;
        ssx_files::replace(&target, &data, &ssx_files::Options::new())
            .map_err(|e| InstallError::Failed(format!("{}: {e}", target.display())))?;
    }
    if let Some(profile) = profile_target {
        if let Some(parent) = profile.parent().filter(|d| !d.as_os_str().is_empty()) {
            ssx_files::create_dir_all(parent).map_err(|e| InstallError::Failed(e.to_string()))?;
        }
        ssx_files::replace(profile, &data, &ssx_files::Options::new())
            .map_err(|e| InstallError::Failed(format!("{}: {e}", profile.display())))?;
    }
    Ok(Report { loaded, target, profile: profile_target.map(Path::to_path_buf) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_formats::save::chunks::TuningChunk;

    fn profile(tuning: TuningChunk) -> Vec<u8> {
        let mut chunks: Vec<Vec<u8>> = (0..11).map(|_| Vec::new()).collect();
        chunks[7] = tuning.write();
        ProfileFile { chunks, tail: Vec::new() }.write()
    }

    #[test]
    fn a_profile_without_a_tuner_is_no_tuner() {
        let buffer = profile(TuningChunk::default());
        assert_eq!(parse(&buffer).unwrap(), None);
        let wrapped = save::Header::new(&buffer).wrap(&buffer);
        assert_eq!(parse(&wrapped).unwrap(), None);
        assert!(parse(&vec![0u8; PROFILE_BUFFER_SIZE]).is_err());
        assert!(parse(&[0u8; 64]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn a_damaged_chunk_is_refused() {
        let mut tuning = TuningChunk { unk_word: 4, ..TuningChunk::default() };
        tuning.buffer[..4].copy_from_slice(b"junk");
        assert!(parse(&profile(tuning)).is_err());
    }

    #[test]
    fn oversized_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("ssx-tuner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        std::fs::write(&path, vec![0u8; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
