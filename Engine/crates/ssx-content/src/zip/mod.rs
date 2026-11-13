mod read;
mod write;

pub use read::{ZipArchive, ZipEntry};
pub use write::{PackFile, write_zip};

use crate::limits::{MAX_FILE_BYTES, MAX_PACKAGE_BYTES, MAX_ZIP_RATIO, RATIO_FLOOR};
use crate::path::MAX_PACKAGE_ENTRIES;

pub const INDEX_FILE: &str = "package-index.tsv";

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const END_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const DESCRIPTOR_SIG: u32 = 0x0807_4b50;
const MAX_VERSION_NEEDED: u16 = 20;
const KNOWN_FLAGS: u16 = 0x0002 | 0x0004 | 0x0008 | 0x0800;
const FLAG_DESCRIPTOR: u16 = 0x0008;
const MAX_EXTRA: usize = 4096;
const MAX_COMMENT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflated,
}

impl Method {
    pub const fn code(self) -> u16 {
        match self {
            Method::Stored => 0,
            Method::Deflated => 8,
        }
    }
}

fn method_name(code: u16) -> String {
    let name = match code {
        1 => "Shrink",
        2..=5 => "Reduce",
        6 => "Implode",
        9 => "Deflate64",
        10 => "PKWARE DCL Implode",
        12 => "BZIP2",
        14 => "LZMA",
        18 => "IBM TERSE",
        19 => "IBM LZ77",
        20 | 93 => "Zstandard",
        95 => "XZ",
        96 => "JPEG",
        97 => "WavPack",
        98 => "PPMd",
        99 => "AES encryption",
        _ => "an unknown method",
    };
    format!("{name} (method {code})")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZipNames {
    #[default]
    Package,
    PlayerDump,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZipLimits {
    pub max_archive_bytes: u64,
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_ratio: u64,
    pub ratio_floor: u64,
    pub names: ZipNames,
}

impl Default for ZipLimits {
    fn default() -> Self {
        ZipLimits {
            max_archive_bytes: MAX_PACKAGE_BYTES,
            max_entries: MAX_PACKAGE_ENTRIES,
            max_file_bytes: MAX_FILE_BYTES,
            max_total_bytes: MAX_PACKAGE_BYTES,
            max_ratio: MAX_ZIP_RATIO,
            ratio_floor: RATIO_FLOOR,
            names: ZipNames::Package,
        }
    }
}

impl ZipLimits {
    pub fn bomb(&self, size: u64, stored: u64) -> bool {
        size > self.ratio_floor && size > stored.saturating_mul(self.max_ratio)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ZipError {
    #[error("{}", ssx_core::text::shown_max(.0, ssx_core::text::LINE_CHARS))]
    Refused(String),
    #[error("{}", ssx_core::text::Text::new(.0))]
    Io(#[from] std::io::Error),
}

pub(crate) fn refused(message: impl Into<String>) -> ZipError {
    ZipError::Refused(message.into())
}

pub(crate) fn shown(name: &[u8]) -> String {
    ssx_core::text::shown_max(&String::from_utf8_lossy(name), 80).to_string()
}

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
