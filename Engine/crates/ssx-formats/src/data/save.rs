pub mod chunks;

use crate::reader::Reader;
use crate::{Error, Result};

pub const PROFILE_BUFFER_SIZE: usize = 0x57800;

pub const PROFILE_CHUNKS: usize = 11;

pub const LIBRARY_HEADER_LEN: usize = 0x1C;

pub const PROFILE_HEADER_NAME: &[u8; 16] = b"TXT_PROF_SAVEGR\0";

pub const PROFILE_GROUP_WORD: u32 = 0x49;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: [u8; 16],
    pub checksum: u32,
    pub group: u32,
    pub zero: u32,
}

impl Header {
    pub fn new(buffer: &[u8]) -> Self {
        Header { name: *PROFILE_HEADER_NAME, checksum: crc32_ieee(buffer), group: PROFILE_GROUP_WORD, zero: 0 }
    }

    pub fn parse(header: &[u8]) -> Result<Self> {
        if header.len() != LIBRARY_HEADER_LEN {
            return Err(Error::invalid(format!(
                "save: the library header is {} bytes, expected {LIBRARY_HEADER_LEN}",
                header.len()
            )));
        }
        let mut name = [0u8; 16];
        name.copy_from_slice(&header[..16]);
        Ok(Header {
            name,
            checksum: u32::from_be_bytes([header[0x10], header[0x11], header[0x12], header[0x13]]),
            group: u32::from_be_bytes([header[0x14], header[0x15], header[0x16], header[0x17]]),
            zero: u32::from_be_bytes([header[0x18], header[0x19], header[0x1A], header[0x1B]]),
        })
    }

    pub fn wrap(&self, buffer: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(LIBRARY_HEADER_LEN + buffer.len());
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&crc32_ieee(buffer).to_be_bytes());
        out.extend_from_slice(&self.group.to_be_bytes());
        out.extend_from_slice(&self.zero.to_be_bytes());
        out.extend_from_slice(buffer);
        out
    }
}

pub fn split(data: &[u8]) -> Result<(Option<Header>, &[u8])> {
    let unchecked = data.len() == PROFILE_BUFFER_SIZE + LIBRARY_HEADER_LEN && data[0x10..0x14] == [0; 4];
    if unchecked {
        let header = Header::parse(&data[..LIBRARY_HEADER_LEN])?;
        return Ok((Some(header), &data[LIBRARY_HEADER_LEN..]));
    }
    if data.starts_with(PROFILE_HEADER_NAME) {
        if data.len() != PROFILE_BUFFER_SIZE + LIBRARY_HEADER_LEN {
            return Err(Error::invalid(format!(
                "save: the library header's file is {} bytes, expected {}",
                data.len(),
                PROFILE_BUFFER_SIZE + LIBRARY_HEADER_LEN
            )));
        }
        let header = Header::parse(&data[..LIBRARY_HEADER_LEN])?;
        if header.group != PROFILE_GROUP_WORD {
            return Err(Error::invalid(format!(
                "save: the library header's group {:#x}, the profile group is {PROFILE_GROUP_WORD:#x}",
                header.group
            )));
        }
        let buffer = &data[LIBRARY_HEADER_LEN..];
        let crc = crc32_ieee(buffer);
        if header.checksum != crc {
            return Err(Error::invalid(format!(
                "save: the library header's checksum {:#010x}, computed {crc:#010x}",
                header.checksum
            )));
        }
        return Ok((Some(header), buffer));
    }
    if data.len() == PROFILE_BUFFER_SIZE + LIBRARY_HEADER_LEN {
        return Err(Error::invalid(
            "save: a library-header-sized file without the profile group's name key (TXT_PROF_SAVEGR)",
        ));
    }
    if data.len() < PROFILE_BUFFER_SIZE {
        return Err(Error::invalid(format!("save: {} bytes, the profile buffer is {PROFILE_BUFFER_SIZE}", data.len())));
    }
    Ok((None, data))
}

pub fn buffer(data: &[u8]) -> Result<&[u8]> {
    split(data).map(|(_, buffer)| buffer)
}

pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub fn crc32_bzip2(data: &[u8]) -> u32 {
    !crate::t3db::crc32(data, 0xFFFF_FFFF)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkInfo {
    pub name: &'static str,
    pub owner: &'static str,
    pub save: u32,
    pub load: u32,
}

pub const CHUNKS: [ChunkInfo; PROFILE_CHUNKS] = [
    ChunkInfo {
        name: "database",
        owner: "ProfileDb +0xF68 (T3DB group SSXV_PROFILE)",
        save: 0x824A_76C0,
        load: 0x824A_7780,
    },
    ChunkInfo { name: "online", owner: "online manager [0x838AB460] +4", save: 0x8289_BD90, load: 0x8289_BE18 },
    ChunkInfo { name: "flow", owner: "mpFlowManager +0x1FD4", save: 0x8246_7E98, load: 0x8246_7F78 },
    ChunkInfo { name: "stats", owner: "mStatsDataManager +0x1FDC", save: 0x824B_4A28, load: 0x824B_4AB8 },
    ChunkInfo { name: "badges", owner: "mBadgeManager +0xF64", save: 0x8248_87F8, load: 0x8248_88B0 },
    ChunkInfo { name: "geotags", owner: "geotag manager +0xF98 (+4)", save: 0x8243_FFD8, load: 0x8243_FFE0 },
    ChunkInfo { name: "friends", owner: "friend challenges +0xF94 (+4)", save: 0x8246_A330, load: 0x8246_B398 },
    ChunkInfo { name: "tuning", owner: "TuningCache +0xFA0", save: 0x824B_4CA0, load: 0x824B_4D10 },
    ChunkInfo { name: "activity", owner: "ActivityManager +0xF5C", save: 0x8245_2328, load: 0x8245_2C90 },
    ChunkInfo { name: "front end", owner: "mFESaveData +0xFA8", save: 0x824B_2AD0, load: 0x824B_2C48 },
    ChunkInfo { name: "key set", owner: "[0x838AB910] +0x14C", save: 0x823E_2338, load: 0x823E_23E8 },
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileFile {
    pub chunks: Vec<Vec<u8>>,
    pub tail: Vec<u8>,
}

impl ProfileFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        let crc = r.u32()?;
        let computed = crc32_bzip2(&data[4..]);
        if crc != computed {
            return Err(Error::invalid(format!("save: CRC {crc:#010x}, computed {computed:#010x}")));
        }
        let count = r.u32()? as usize;
        if count != PROFILE_CHUNKS {
            return Err(Error::invalid(format!("save: {count} chunks, the game registers {PROFILE_CHUNKS}")));
        }
        let mut chunks = Vec::with_capacity(count);
        for _ in 0..count {
            let n = r.u32()? as usize;
            chunks.push(r.bytes(n)?.to_vec());
        }
        let tail = data[r.pos()..].to_vec();
        Ok(ProfileFile { chunks, tail })
    }

    fn body(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend((self.chunks.len() as u32).to_be_bytes());
        for c in &self.chunks {
            b.extend((c.len() as u32).to_be_bytes());
            b.extend(c);
        }
        b
    }

    pub fn used_len(&self) -> usize {
        4 + self.body().len()
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4];
        out.extend(self.body());
        out.extend(&self.tail);
        out.resize(out.len().max(PROFILE_BUFFER_SIZE), 0);
        let crc = crc32_bzip2(&out[4..]);
        out[..4].copy_from_slice(&crc.to_be_bytes());
        out
    }

    pub fn write_with_header(&self, header: &Header) -> Vec<u8> {
        header.wrap(&self.write())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_bzip2() {
        assert_eq!(crc32_bzip2(b"123456789"), 0xFC89_1918);
    }

    #[test]
    fn the_header_crc_is_crc32_ieee() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_ne!(crc32_ieee(b"123456789"), crc32_bzip2(b"123456789"));
    }

    #[test]
    fn buffers_round_trip_and_check_their_crc() {
        let f = ProfileFile { chunks: (0..11).map(|i| vec![i as u8; i]).collect(), tail: Vec::new() };
        let bytes = f.write();
        assert_eq!(bytes.len(), PROFILE_BUFFER_SIZE);
        let back = ProfileFile::parse(&bytes).unwrap();
        assert_eq!(back.chunks, f.chunks);
        assert_eq!(back.write(), bytes);
        let mut bad = bytes.clone();
        bad[100] ^= 1;
        assert!(ProfileFile::parse(&bad).is_err());
        let short = ProfileFile { chunks: vec![vec![]; 3], tail: Vec::new() }.write();
        assert!(ProfileFile::parse(&short).is_err());
    }

    #[test]
    fn the_library_header_is_validated_preserved_and_size_checked() {
        let file = ProfileFile { chunks: (0..11).map(|_| Vec::new()).collect(), tail: Vec::new() };
        let bytes = file.write();
        assert_eq!(buffer(&bytes).unwrap(), &bytes[..]);
        let header = Header::new(&bytes);
        assert_eq!(header.name, *PROFILE_HEADER_NAME);
        assert_eq!(header.group, PROFILE_GROUP_WORD);
        assert_eq!(header.checksum, crc32_ieee(&bytes));
        let wrapped = file.write_with_header(&header);
        let (read, buffer) = split(&wrapped).unwrap();
        assert_eq!(read, Some(header));
        assert_eq!(buffer, &bytes[..]);
        assert_eq!(Header::new(buffer).wrap(buffer), wrapped);
        let mut bad = wrapped.clone();
        bad[LIBRARY_HEADER_LEN + 100] ^= 1;
        assert!(split(&bad).is_err());
        let mut wrong_group = wrapped.clone();
        wrong_group[0x14..0x18].copy_from_slice(&0x0000_000Cu32.to_be_bytes());
        assert!(split(&wrong_group).is_err());
        let mut cut = wrapped.clone();
        cut.pop();
        assert!(split(&cut).is_err());
        let mut wrong_name = wrapped.clone();
        wrong_name[0] = b'X';
        assert!(split(&wrong_name).is_err());
        assert!(split(&bytes[..PROFILE_BUFFER_SIZE - 1]).is_err());
        let mut zero = wrong_name.clone();
        zero[0x10..0x14].fill(0);
        zero[0x14..0x18].copy_from_slice(&0x0000_000Cu32.to_be_bytes());
        zero[LIBRARY_HEADER_LEN + 100] ^= 1;
        let (read, buffer) = split(&zero).unwrap();
        assert_eq!(read.map(|h| h.checksum), Some(0));
        assert_eq!(buffer, &zero[LIBRARY_HEADER_LEN..]);
    }
}
