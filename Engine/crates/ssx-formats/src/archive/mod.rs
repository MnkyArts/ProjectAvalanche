pub mod big;
pub mod chunk;
pub mod dlc;
pub mod lzx;
pub mod refpack;
pub mod tu;
pub mod xcontent;
pub mod xdvdfs;

pub fn is_reserved_device_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
