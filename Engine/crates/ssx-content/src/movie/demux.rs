use std::collections::VecDeque;
use std::io::{Read, Seek};

use super::ebml::{Header, Reader};
use super::header::{CLUSTER, CRC32, SKIPPED, VOID, read_head};
use super::{Limits, MovieError, MovieInfo, Result, av1, sniff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub track: Track,
    pub pts_ns: i64,
    pub keyframe: bool,
    pub discard_padding_ns: i64,
    pub data: Vec<u8>,
    pub at: u64,
}

pub struct Demuxer<R> {
    r: Reader<R>,
    info: MovieInfo,
    limits: Limits,
    segment_end: u64,
    cluster: Option<(u64, i64)>,
    first_cluster: Option<Header>,
    first_cluster_at: Header,
    pending: VecDeque<Packet>,
    last_video: Option<i64>,
    last_audio: Option<i64>,
    video_frames: u64,
    audio_packets: u64,
}

impl<R: Read + Seek> Demuxer<R> {
    pub fn open(mut inner: R, len: u64, limits: &Limits) -> Result<Self> {
        if len > limits.max_file_bytes {
            return Err(MovieError::new(0, format!("{len} bytes (at most {})", limits.max_file_bytes)));
        }
        let mut head = [0u8; 8];
        let n = read_up_to(&mut inner, &mut head);
        if let Some(what) = sniff(&head[..n]) {
            return Err(MovieError::new(0, format!("named .webm, but its content is {what}")));
        }
        inner.rewind().map_err(|e| MovieError::new(0, format!("seek: {e}")))?;
        let mut r = Reader::new(inner, len);
        let (info, segment_end, first) = read_head(&mut r, limits)?;
        Ok(Self {
            r,
            info,
            limits: *limits,
            segment_end,
            cluster: None,
            first_cluster: Some(first),
            first_cluster_at: first,
            pending: VecDeque::new(),
            last_video: None,
            last_audio: None,
            video_frames: 0,
            audio_packets: 0,
        })
    }

    pub fn info(&self) -> &MovieInfo {
        &self.info
    }

    pub fn restart(&mut self) -> Result<()> {
        self.r.seek_to(self.first_cluster_at.body)?;
        self.cluster = None;
        self.first_cluster = Some(self.first_cluster_at);
        self.pending.clear();
        self.last_video = None;
        self.last_audio = None;
        self.video_frames = 0;
        self.audio_packets = 0;
        Ok(())
    }

    pub fn next_packet(&mut self) -> Result<Option<Packet>> {
        loop {
            if let Some(p) = self.pending.pop_front() {
                return Ok(Some(p));
            }
            match self.cluster {
                Some((end, ts)) if self.r.pos() < end => self.cluster_child(end, ts)?,
                Some(_) => self.cluster = None,
                None => {
                    let h = match self.first_cluster.take() {
                        Some(h) => h,
                        None if self.r.pos() >= self.segment_end => return Ok(None),
                        None => self.r.header(self.segment_end)?,
                    };
                    match h.id {
                        CLUSTER => self.enter_cluster(&h)?,
                        VOID | CRC32 => self.r.skip(&h)?,
                        id if SKIPPED.iter().any(|(s, _)| *s == id) => self.r.skip(&h)?,
                        id => {
                            let message =
                                format!("element 0x{id:X} among the clusters, which the reader does not know");
                            return Err(MovieError::new(h.start, message));
                        }
                    }
                }
            }
        }
    }

    fn enter_cluster(&mut self, h: &Header) -> Result<()> {
        let t = self.r.header(h.end())?;
        if t.id != 0xE7 {
            return Err(MovieError::new(t.start, "a cluster that does not begin with its timestamp"));
        }
        let ts = self.r.uint(&t, "a cluster's timestamp")?;
        let ts = i64::try_from(ts).map_err(|_| MovieError::new(t.start, "a cluster timestamp past 2⁶³"))?;
        self.cluster = Some((h.end(), ts));
        Ok(())
    }

    fn cluster_child(&mut self, end: u64, ts: i64) -> Result<()> {
        let h = self.r.header(end)?;
        match h.id {
            0xA3 => {
                let block = self.block(&h, ts, None)?;
                self.queue(block)
            }
            0xA0 => self.block_group(&h, ts),
            0xA7 | 0xAB | VOID | CRC32 => self.r.skip(&h),
            0xE7 => Err(MovieError::new(h.start, "a second timestamp in a cluster")),
            0xAF => Err(MovieError::new(h.start, "an encrypted block")),
            id => {
                Err(MovieError::new(h.start, format!("element 0x{id:X} in a cluster, which the reader does not know")))
            }
        }
    }

    fn block_group(&mut self, h: &Header, ts: i64) -> Result<()> {
        let (mut block, mut reference, mut padding) = (None, false, 0i64);
        while self.r.pos() < h.end() {
            let c = self.r.header(h.end())?;
            match c.id {
                0xA1 if block.is_none() => {
                    block = Some(c);
                    self.r.skip(&c)?;
                }
                0xA1 => return Err(MovieError::new(c.start, "two blocks in a block group")),
                0xFB => {
                    self.r.sint(&c, "ReferenceBlock")?;
                    reference = true;
                }
                0x75A2 => padding = self.r.sint(&c, "DiscardPadding")?,
                0x9B | 0xFA | VOID | CRC32 => self.r.skip(&c)?,
                id => {
                    let message =
                        format!("element 0x{id:X} in a block group (additions, codec state, slices are refused)");
                    return Err(MovieError::new(c.start, message));
                }
            }
        }
        let b = block.ok_or_else(|| MovieError::new(h.start, "a block group without its block"))?;
        let end = self.r.pos();
        self.r.seek_to(b.body)?;
        let mut packets = self.block(&b, ts, Some(!reference))?;
        self.r.seek_to(end)?;
        if padding != 0 {
            let last = packets.last_mut().filter(|p| p.track == Track::Audio && padding > 0);
            let Some(last) = last else {
                return Err(MovieError::new(
                    h.start,
                    format!("a discard padding of {padding} ns on a video block or below 0"),
                ));
            };
            let rate = self.info.audio.as_ref().map_or(1, |a| a.rate);
            let long = 1_000_000_000i64 * (1i64 << self.info.audio.as_ref().map_or(13, |a| a.headers.blocksize_exp[1]))
                / i64::from(rate);
            if padding > long {
                return Err(MovieError::new(
                    h.start,
                    format!("a discard padding of {padding} ns (at most one long block, {long} ns)"),
                ));
            }
            last.discard_padding_ns = padding;
        }
        self.queue(packets)
    }
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

impl<R: Read + Seek> Demuxer<R> {
    fn block(&mut self, h: &Header, cluster_ts: i64, key: Option<bool>) -> Result<Vec<Packet>> {
        let fail = |m: String| Err(MovieError::new(h.start, m));
        let mut first = [0u8; 1];
        self.r.read_exact(&mut first, "a block's track number")?;
        let len = first[0].leading_zeros() as u64 + 1;
        if len > 8 {
            return fail("a block's track number of more than 8 bytes".into());
        }
        let mut number = u64::from(first[0]) & (0xFF >> len);
        for _ in 1..len {
            let mut b = [0u8; 1];
            self.r.read_exact(&mut b, "a block's track number")?;
            number = (number << 8) | u64::from(b[0]);
        }
        let mut rest = [0u8; 3];
        self.r.read_exact(&mut rest, "a block's header")?;
        let rel = i64::from(i16::from_be_bytes([rest[0], rest[1]]));
        let flags = rest[2];
        let payload =
            h.size.checked_sub(len + 3).ok_or_else(|| MovieError::new(h.start, "a block shorter than its header"))?;
        let audio = self.info.audio.as_ref().filter(|a| a.number == number).map(|a| a.rate);
        let track = if number == self.info.video.number {
            Track::Video
        } else if audio.is_some() {
            Track::Audio
        } else {
            return fail(format!("a block of track {number}, which the movie does not have"));
        };
        let cap = if track == Track::Video { self.limits.max_video_block } else { self.limits.max_audio_block };
        if payload > cap {
            return fail(format!("a {} block of {payload} bytes (at most {cap})", name(track)));
        }
        if flags & 0x08 != 0 {
            return fail("an invisible block".into());
        }
        let keyframe = key.unwrap_or(flags & 0x80 != 0);
        let ticks = cluster_ts.checked_add(rel).filter(|t| *t >= 0);
        let pts = ticks.and_then(|t| t.checked_mul(self.info.timestamp_scale as i64));
        let latest = self.info.duration_ns.saturating_add(self.limits.max_overrun_ns);
        let Some(pts) = pts.filter(|&p| p as u64 <= latest) else {
            let d = self.info.duration_seconds();
            return fail(format!("a block at tick {cluster_ts} + {rel}, outside the movie's {d:.3} s"));
        };
        let lacing = (flags >> 1) & 3;
        if lacing != 0 && track == Track::Video {
            return fail("laced video (a video block holds one temporal unit)".into());
        }
        let mut data =
            vec![0u8; usize::try_from(payload).map_err(|_| MovieError::new(h.start, "a block larger than memory"))?];
        self.r.read_exact(&mut data, "a block's frames")?;
        let sizes = match lacing {
            0 => vec![data.len()],
            _ => lace_sizes(lacing, &data, self.limits.max_lace_frames).map_err(|m| MovieError::new(h.start, m))?,
        };
        let mut out = Vec::with_capacity(sizes.len());
        let mut pos = data.len() - sizes.iter().sum::<usize>();
        for s in sizes {
            let frame = data[pos..pos + s].to_vec();
            pos += s;
            out.push(Packet { track, pts_ns: pts, keyframe, discard_padding_ns: 0, data: frame, at: h.start });
        }
        Ok(out)
    }

    fn queue(&mut self, packets: Vec<Packet>) -> Result<()> {
        for p in packets {
            let fail = |m: String| Err(MovieError::new(p.at, m));
            match p.track {
                Track::Video => {
                    if self.last_video.is_none() && !p.keyframe {
                        return fail("the first picture is not a key frame".into());
                    }
                    if let Some(last) = self.last_video {
                        let min = self.limits.min_frame_ns as i64 - self.info.timestamp_scale as i64;
                        if p.pts_ns <= last || p.pts_ns - last < min {
                            let fps = 1e9 / self.limits.min_frame_ns as f64;
                            return fail(format!(
                                "a picture at {} ns after one at {last} ns (pictures come in order, at most {fps:.0} a second)",
                                p.pts_ns
                            ));
                        }
                    }
                    av1::check_temporal_unit(&p.data, &self.info.video.config, &self.limits)
                        .map_err(|m| MovieError::new(p.at, format!("AV1: {m}")))?;
                    self.last_video = Some(p.pts_ns);
                    self.video_frames += 1;
                }
                Track::Audio => {
                    if self.last_audio.is_some_and(|last| p.pts_ns < last) {
                        return fail("audio blocks out of order".into());
                    }
                    let rate = self.info.audio.as_ref().map_or(0, |a| u64::from(a.rate));
                    let cap = (self.info.duration_ns + self.limits.max_overrun_ns) / 1_000_000 * rate / 32_000 + 64;
                    if self.audio_packets >= cap {
                        return fail(format!(
                            "more than {cap} audio packets for {:.3} s",
                            self.info.duration_seconds()
                        ));
                    }
                    self.last_audio = Some(p.pts_ns);
                    self.audio_packets += 1;
                }
            }
            self.pending.push_back(p);
        }
        Ok(())
    }

    pub fn counts(&self) -> (u64, u64) {
        (self.video_frames, self.audio_packets)
    }
}

fn name(t: Track) -> &'static str {
    match t {
        Track::Video => "video",
        Track::Audio => "audio",
    }
}

fn lace_sizes(lacing: u8, data: &[u8], max: usize) -> std::result::Result<Vec<usize>, String> {
    let count = usize::from(*data.first().ok_or("a laced block without its frame count")?) + 1;
    if count > max {
        return Err(format!("{count} frames laced in one block (at most {max})"));
    }
    let mut pos = 1;
    let mut sizes = Vec::with_capacity(count);
    match lacing {
        1 => {
            for _ in 0..count - 1 {
                let mut s = 0usize;
                loop {
                    let v = *data.get(pos).ok_or("a Xiph lace's sizes are cut short")?;
                    pos += 1;
                    s += usize::from(v);
                    if v != 255 {
                        break;
                    }
                    if s > data.len() {
                        return Err("a Xiph lace's frames reach past its block".into());
                    }
                }
                sizes.push(s);
            }
        }
        3 => {
            let (first, n) = vint(&data[pos..]).ok_or("an EBML lace's first size is malformed")?;
            pos += n;
            let too_large = "an EBML lace's frame larger than its block";
            let mut s = usize::try_from(first).ok().filter(|&v| v <= data.len()).ok_or(too_large)?;
            sizes.push(s);
            for _ in 1..count - 1 {
                let (raw, n) = vint(&data[pos..]).ok_or("an EBML lace's size difference is malformed")?;
                pos += n;
                let delta = i64::try_from(raw).map_err(|_| too_large)? - ((1i64 << (7 * n - 1)) - 1);
                let next = i64::try_from(s).map_err(|_| too_large)?.checked_add(delta).ok_or(too_large)?;
                s = usize::try_from(next)
                    .ok()
                    .filter(|&v| v <= data.len())
                    .ok_or("an EBML lace's size below 0 or past its block")?;
                sizes.push(s);
            }
        }
        _ => {
            let rest = data.len() - pos;
            if !rest.is_multiple_of(count) {
                return Err(format!("a fixed lace of {count} frames in {rest} bytes"));
            }
            sizes.extend(std::iter::repeat_n(rest / count, count - 1));
        }
    }
    let used = sizes.iter().try_fold(pos, |a, &s| a.checked_add(s)).filter(|&u| u < data.len());
    let last = used.map(|u| data.len() - u).ok_or("a lace's frames reach past its block")?;
    sizes.push(last);
    if sizes.contains(&0) {
        return Err("an empty frame in a lace".into());
    }
    Ok(sizes)
}

fn vint(b: &[u8]) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut v = u64::from(first) & (0xFF >> len);
    for i in 1..len {
        v = (v << 8) | u64::from(*b.get(i)?);
    }
    Some((v, len))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Walked {
    pub info: MovieInfo,
    pub pictures: u64,
    pub keyframes: u64,
    pub audio_packets: u64,
    pub largest_video_block: u64,
    pub largest_audio_packet: u64,
    pub last_picture_ns: i64,
}

pub fn walk<R: Read + Seek>(inner: R, len: u64, limits: &Limits) -> Result<Walked> {
    let mut d = Demuxer::open(inner, len, limits)?;
    let mut w = Walked {
        info: d.info().clone(),
        pictures: 0,
        keyframes: 0,
        audio_packets: 0,
        largest_video_block: 0,
        largest_audio_packet: 0,
        last_picture_ns: 0,
    };
    while let Some(p) = d.next_packet()? {
        let n = p.data.len() as u64;
        match p.track {
            Track::Video => {
                w.pictures += 1;
                w.keyframes += u64::from(p.keyframe);
                w.largest_video_block = w.largest_video_block.max(n);
                w.last_picture_ns = p.pts_ns;
            }
            Track::Audio => {
                w.audio_packets += 1;
                w.largest_audio_packet = w.largest_audio_packet.max(n);
            }
        }
    }
    if w.pictures == 0 {
        return Err(MovieError::new(len, "no pictures"));
    }
    Ok(w)
}
