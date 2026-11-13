use crate::DecodeError;

pub const MAX_ARGS: usize = 64 << 10;
pub const MAX_ITEM: usize = MAX_ARGS + 1024;
pub const MAX_WIRE: usize = 1 << 10;
const PIECE_HEADER: usize = 3;
const FIRST: u8 = 1;
const LAST: u8 = 2;

pub const MAX_PACKAGE: usize = 64;
pub const MAX_NAME: usize = 128;
pub const MAX_KIND: usize = 64;
pub const MAX_ID: usize = 256;
pub const MAX_TITLE: usize = 256;
pub const MAX_TEXT: usize = 1024;
pub const MAX_LATENT: u32 = 16 << 20;
// PORT(choice): 1 000 000 B/s, the host's top rate for one transfer (MTA's default per transfer is 50 000 B/s; its
// per-client bandwidth was not checked here); tuning: latent transfers measured on real links beside the rides; the
// original runs no mod scripts.
pub const LATENT_DOWN: u32 = 1_000_000;
// PORT(choice): 64 KiB/s, half of FiveM's netEventSize (128 KiB/s), the server's bucket a burst of 3 s (net bunching);
// tuning: a gamemode's client uploads (screenshots, replays) measured beside its remote events; the original runs no
// mod scripts.
pub const LATENT_UP: u32 = 64 << 10;

#[derive(Clone, Debug, PartialEq)]
pub struct WireMarker {
    pub id: String,
    pub position: [f32; 3],
    pub heading: f32,
    pub size: f32,
    pub colour: u32,
    pub visible: bool,
    pub checkpoint: Option<u16>,
    pub instance: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScriptBody {
    Event { name: String, source: u64, args: Vec<u8> },
    LatentStart { transfer: u64, name: String, source: u64, total: u32 },
    LatentChunk { transfer: u64, offset: u32, bytes: Vec<u8> },
    LatentCancel { transfer: u64 },
    DataWrite { element: u64, key: String, value: Option<Vec<u8>> },
    Reset,
    Created { element: u64, kind: String, name: String, parent: u64 },
    Destroyed { element: u64 },
    Moved { element: u64, parent: u64 },
    Renamed { element: u64, name: String },
    Data { element: u64, key: String, value: Option<Vec<u8>> },
    Hello { player: u64 },
    Rider { element: u64, player: u64, number: u16 },
    Marker { element: u64, marker: WireMarker },
    Notice { title: String, text: String, seconds: f32 },
    Sound { id: String, position: Option<[f32; 3]> },
    Started,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptItem {
    pub package: Option<String>,
    pub body: ScriptBody,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToServer,
    ToClient,
}

impl ScriptBody {
    fn kind(&self) -> u8 {
        match self {
            Self::Event { .. } => 1,
            Self::LatentStart { .. } => 2,
            Self::LatentChunk { .. } => 3,
            Self::LatentCancel { .. } => 4,
            Self::DataWrite { .. } => 5,
            Self::Reset => 6,
            Self::Created { .. } => 7,
            Self::Destroyed { .. } => 8,
            Self::Moved { .. } => 9,
            Self::Renamed { .. } => 10,
            Self::Data { .. } => 11,
            Self::Hello { .. } => 12,
            Self::Rider { .. } => 13,
            Self::Marker { .. } => 14,
            Self::Notice { .. } => 15,
            Self::Sound { .. } => 16,
            Self::Started => 17,
        }
    }

    pub fn from_client(&self) -> bool {
        matches!(
            self,
            Self::Event { .. }
                | Self::LatentStart { .. }
                | Self::LatentChunk { .. }
                | Self::LatentCancel { .. }
                | Self::DataWrite { .. }
                | Self::Started
        )
    }

    pub fn from_server(&self) -> bool {
        !matches!(self, Self::DataWrite { .. } | Self::Started)
    }

    pub fn is_event(&self) -> bool {
        matches!(self, Self::Event { .. } | Self::LatentStart { .. })
    }
}

fn err(detail: impl Into<String>) -> DecodeError {
    DecodeError::Text { kind: "script item", detail: detail.into() }
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    fn text(&mut self, s: &str, max: usize) -> Result<(), DecodeError> {
        if s.len() > max {
            return Err(err(format!("a text of {} bytes (at most {max})", s.len())));
        }
        self.u16(s.len() as u16);
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
    fn bytes(&mut self, b: &[u8], max: usize) -> Result<(), DecodeError> {
        if b.len() > max {
            return Err(err(format!("{} bytes (at most {max})", b.len())));
        }
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn opt_bytes(&mut self, b: &Option<Vec<u8>>) -> Result<(), DecodeError> {
        match b {
            None => self.u8(0),
            Some(b) => {
                self.u8(1);
                self.bytes(b, MAX_ARGS)?;
            }
        }
        Ok(())
    }
    fn vec3(&mut self, p: [f32; 3]) {
        p.iter().for_each(|v| self.f32(*v));
    }
}

struct R<'a>(&'a [u8]);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.0.len() < n {
            return Err(err("cut short"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(a))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        let v = f32::from_bits(self.u32()?);
        if !v.is_finite() {
            return Err(err("a number that is not finite"));
        }
        Ok(v)
    }
    fn text(&mut self, max: usize) -> Result<String, DecodeError> {
        let n = usize::from(self.u16()?);
        if n > max {
            return Err(err(format!("a text of {n} bytes (at most {max})")));
        }
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| err("a text that is not UTF-8"))?;
        if s.chars().any(char::is_control) {
            return Err(err("a text with control characters"));
        }
        Ok(s.to_string())
    }
    fn bytes(&mut self, max: usize) -> Result<Vec<u8>, DecodeError> {
        let n = usize::try_from(self.u32()?).map_err(|_| err("a size beyond this system's"))?;
        if n > max {
            return Err(err(format!("{n} bytes (at most {max})")));
        }
        Ok(self.take(n)?.to_vec())
    }
    fn opt_bytes(&mut self) -> Result<Option<Vec<u8>>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.bytes(MAX_ARGS)?)),
            b => Err(err(format!("an option byte {b}"))),
        }
    }
    fn vec3(&mut self) -> Result<[f32; 3], DecodeError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn element(&mut self) -> Result<u64, DecodeError> {
        let e = self.u64()?;
        if e == 0 || e >= 1 << 53 {
            return Err(err(format!("element id {e}")));
        }
        Ok(e)
    }
}

impl ScriptItem {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut w = W(Vec::new());
        w.text(self.package.as_deref().unwrap_or(""), MAX_PACKAGE)?;
        w.u8(self.body.kind());
        match &self.body {
            ScriptBody::Event { name, source, args } => {
                w.text(name, MAX_NAME)?;
                w.u64(*source);
                w.bytes(args, MAX_ARGS)?;
            }
            ScriptBody::LatentStart { transfer, name, source, total } => {
                w.u64(*transfer);
                w.text(name, MAX_NAME)?;
                w.u64(*source);
                w.u32(*total);
            }
            ScriptBody::LatentChunk { transfer, offset, bytes } => {
                w.u64(*transfer);
                w.u32(*offset);
                w.bytes(bytes, MAX_ARGS)?;
            }
            ScriptBody::LatentCancel { transfer } => w.u64(*transfer),
            ScriptBody::DataWrite { element, key, value } | ScriptBody::Data { element, key, value } => {
                w.u64(*element);
                w.text(key, MAX_NAME)?;
                w.opt_bytes(value)?;
            }
            ScriptBody::Reset | ScriptBody::Started => {}
            ScriptBody::Created { element, kind, name, parent } => {
                w.u64(*element);
                w.text(kind, MAX_KIND)?;
                w.text(name, MAX_NAME)?;
                w.u64(*parent);
            }
            ScriptBody::Destroyed { element } => w.u64(*element),
            ScriptBody::Moved { element, parent } => {
                w.u64(*element);
                w.u64(*parent);
            }
            ScriptBody::Renamed { element, name } => {
                w.u64(*element);
                w.text(name, MAX_NAME)?;
            }
            ScriptBody::Hello { player } => w.u64(*player),
            ScriptBody::Rider { element, player, number } => {
                w.u64(*element);
                w.u64(*player);
                w.u16(*number);
            }
            ScriptBody::Marker { element, marker: m } => {
                w.u64(*element);
                w.text(&m.id, MAX_ID)?;
                w.vec3(m.position);
                w.f32(m.heading);
                w.f32(m.size);
                w.u32(m.colour);
                w.u8(u8::from(m.visible));
                match m.checkpoint {
                    None => w.u8(0),
                    Some(i) => {
                        w.u8(1);
                        w.u16(i);
                    }
                }
                match m.instance {
                    None => w.u8(0),
                    Some(e) => {
                        w.u8(1);
                        w.u64(e);
                    }
                }
            }
            ScriptBody::Notice { title, text, seconds } => {
                w.text(title, MAX_TITLE)?;
                w.text(text, MAX_TEXT)?;
                w.f32(*seconds);
            }
            ScriptBody::Sound { id, position } => {
                w.text(id, MAX_ID)?;
                match position {
                    None => w.u8(0),
                    Some(p) => {
                        w.u8(1);
                        w.vec3(*p);
                    }
                }
            }
        }
        if w.0.len() > MAX_ITEM {
            return Err(err(format!("an item of {} bytes (at most {MAX_ITEM})", w.0.len())));
        }
        Ok(w.0)
    }

    pub fn decode(b: &[u8], direction: Direction) -> Result<Self, DecodeError> {
        if b.len() > MAX_ITEM {
            return Err(err(format!("an item of {} bytes (at most {MAX_ITEM})", b.len())));
        }
        let mut r = R(b);
        let package = r.text(MAX_PACKAGE)?;
        let named = package.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c));
        if !named || package == "." || package == ".." {
            return Err(err("a package name of other characters than a-z 0-9 _ . - (or a dot segment)"));
        }
        let package = (!package.is_empty()).then_some(package);
        let body = match r.u8()? {
            1 => ScriptBody::Event { name: r.text(MAX_NAME)?, source: r.element()?, args: r.bytes(MAX_ARGS)? },
            2 => {
                let (transfer, name, source, total) = (r.u64()?, r.text(MAX_NAME)?, r.element()?, r.u32()?);
                if total > MAX_LATENT {
                    return Err(err(format!("a latent transfer of {total} bytes")));
                }
                ScriptBody::LatentStart { transfer, name, source, total }
            }
            3 => ScriptBody::LatentChunk { transfer: r.u64()?, offset: r.u32()?, bytes: r.bytes(MAX_ARGS)? },
            4 => ScriptBody::LatentCancel { transfer: r.u64()? },
            5 => ScriptBody::DataWrite { element: r.element()?, key: r.text(MAX_NAME)?, value: r.opt_bytes()? },
            6 => ScriptBody::Reset,
            7 => ScriptBody::Created {
                element: r.element()?,
                kind: r.text(MAX_KIND)?,
                name: r.text(MAX_NAME)?,
                parent: r.element()?,
            },
            8 => ScriptBody::Destroyed { element: r.element()? },
            9 => ScriptBody::Moved { element: r.element()?, parent: r.element()? },
            10 => ScriptBody::Renamed { element: r.element()?, name: r.text(MAX_NAME)? },
            11 => ScriptBody::Data { element: r.element()?, key: r.text(MAX_NAME)?, value: r.opt_bytes()? },
            12 => ScriptBody::Hello { player: r.element()? },
            13 => ScriptBody::Rider { element: r.element()?, player: r.element()?, number: r.u16()? },
            14 => {
                let element = r.element()?;
                let (id, position, heading, size, colour) = (r.text(MAX_ID)?, r.vec3()?, r.f32()?, r.f32()?, r.u32()?);
                let visible = match r.u8()? {
                    0 => false,
                    1 => true,
                    v => return Err(err(format!("a visibility byte {v}"))),
                };
                let checkpoint = match r.u8()? {
                    0 => None,
                    1 => Some(r.u16()?),
                    v => return Err(err(format!("an option byte {v}"))),
                };
                let instance = match r.u8()? {
                    0 => None,
                    1 => Some(r.element()?),
                    v => return Err(err(format!("an option byte {v}"))),
                };
                let marker = WireMarker { id, position, heading, size, colour, visible, checkpoint, instance };
                ScriptBody::Marker { element, marker }
            }
            15 => ScriptBody::Notice { title: r.text(MAX_TITLE)?, text: r.text(MAX_TEXT)?, seconds: r.f32()? },
            16 => {
                let id = r.text(MAX_ID)?;
                let position = match r.u8()? {
                    0 => None,
                    1 => Some(r.vec3()?),
                    v => return Err(err(format!("an option byte {v}"))),
                };
                ScriptBody::Sound { id, position }
            }
            17 => ScriptBody::Started,
            k => return Err(err(format!("kind {k}"))),
        };
        if !r.0.is_empty() {
            return Err(err(format!("{} bytes after the item", r.0.len())));
        }
        let allowed = match direction {
            Direction::ToServer => body.from_client(),
            Direction::ToClient => body.from_server(),
        };
        if !allowed {
            return Err(err(format!("kind {} does not travel {direction:?}", body.kind())));
        }
        Ok(ScriptItem { package, body })
    }
}

mod framing;
pub use framing::{Assembler, Outgoing, QueueFull};

#[cfg(test)]
mod tests;
