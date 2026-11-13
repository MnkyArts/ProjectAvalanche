use std::fmt;
use std::path::Path;

use ssx_net_proto::operator::Expiry;

pub const MAX_REASON_BYTES: usize = 200;
pub const MAX_BY_BYTES: usize = 9 + crate::acl::MAX_RESOURCE_BYTES;
// PORT(choice): 16 MiB, room for the entries' caps (65 536 bans of about 200 bytes) with their comments; tuning: the
// ban lists of long-running public servers; the original has no dedicated server.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_DURATION_SECS: u64 = 100 * 365 * 86_400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, ssx_core::text::shown_max(&self.message, 512))
    }
}

impl FileError {
    pub fn at(text: &str, offset: usize, message: impl Into<String>) -> Self {
        let before = text.get(..offset.min(text.len())).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        Self { line, column, message: message.into() }
    }
}

pub fn reason(text: &str) -> Result<&str, String> {
    if text.is_empty() {
        return Ok(text);
    }
    if text.len() > MAX_REASON_BYTES {
        return Err(format!("the reason is {} bytes, at most {MAX_REASON_BYTES}", text.len()));
    }
    ssx_net_proto::chat::chat_text(text).map_err(|e| format!("the reason: {e}"))
}

pub fn by(text: &str) -> Result<&str, String> {
    if text.is_empty() || text.len() > MAX_BY_BYTES || !text.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
        return Err(format!("who made it: 1 to {MAX_BY_BYTES} printable ASCII characters"));
    }
    Ok(text)
}

pub fn player_id(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!("{} is not a player id (64 lowercase hex digits)", ssx_core::text::quoted(text)));
    }
    ssx_net_proto::keys::from_hex(text).map_err(|e| e.to_string())
}

pub fn duration(text: &str) -> Result<Option<u64>, String> {
    if text == "forever" {
        return Ok(None);
    }
    let bad = || format!("{} is not a duration (`30m`, `12h`, `7d`, `2w`, `forever`)", ssx_core::text::quoted(text));
    let (digits, unit) = text.char_indices().last().map_or(("", ""), |(i, _)| text.split_at(i));
    let size: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(bad()),
    };
    if digits.is_empty() || digits.len() > 10 || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let secs = n.checked_mul(size).filter(|s| *s <= MAX_DURATION_SECS);
    secs.map(Some).ok_or_else(|| format!("{}: longer than 100 years (write `forever`)", ssx_core::text::quoted(text)))
}

pub(crate) fn expiry(value: i64) -> Option<Expiry> {
    u64::try_from(value).ok().map(Expiry::from_wire)
}

pub fn read_file(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a regular file"));
    }
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("larger than {MAX_FILE_BYTES} bytes"),
        ));
    }
    Ok(Some(text))
}

pub(crate) fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_reasons_and_ids() {
        assert_eq!(duration("forever"), Ok(None));
        assert_eq!(duration("30s"), Ok(Some(30)));
        assert_eq!(duration("90m"), Ok(Some(5400)));
        assert_eq!(duration("2w"), Ok(Some(1_209_600)));
        for bad in [
            "",
            "s",
            "0d",
            "01d",
            "1D",
            "1 d",
            "-1d",
            "1.5h",
            "1y",
            "99999999999d",
            "36501d",
            "never",
            "1dd",
            "1ë",
            "ë",
        ] {
            assert!(duration(bad).is_err(), "{bad:?}");
        }
        assert_eq!(duration("36500d"), Ok(Some(MAX_DURATION_SECS)));
        assert_eq!(reason(""), Ok(""));
        assert_eq!(reason("Zoë said so"), Ok("Zoë said so"));
        for bad in ["a\nb", "   ", "\u{202E}x", &"w".repeat(101)] {
            assert!(reason(bad).is_err(), "{bad:?}");
        }
        assert!(by("console").is_ok() && by("").is_err() && by("é").is_err());
        assert!(player_id(&"ab".repeat(32)).is_ok());
        assert!(player_id(&"AB".repeat(32)).is_err() && player_id("ab").is_err());
        assert_eq!(toml_string("a\"\\\u{7}"), "\"a\\\"\\\\\\u0007\"");
        assert_eq!(FileError::at("ab\ncd", 4, "x").to_string(), "2:2: x");
    }
}
