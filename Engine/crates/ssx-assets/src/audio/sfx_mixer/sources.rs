use super::*;

enum Dec {
    Xma(ssx_formats::xma_decode::SectionDecoder),
    L3(ssx_formats::ealayer3_decode::SectionDecoder),
}

pub(super) struct Cursor {
    data: Arc<Vec<u8>>,
    pub(super) header: SoundHeader,
    blocks: Vec<(usize, usize, u32)>,
    next: usize,
    dec: Option<(usize, Dec)>,
    tmp: Vec<f32>,
}

impl Cursor {
    pub(super) fn new(data: Arc<Vec<u8>>, header: SoundHeader, body: usize) -> Result<Self> {
        let sb = read_v0_blocks(&header, &data, body)?;
        let blocks = sb
            .sections
            .iter()
            .enumerate()
            .flat_map(|(s, sec)| sec.iter().map(move |b| (b.offset, s, b.samples)))
            .collect();
        Ok(Self { data, header, blocks, next: 0, dec: None, tmp: Vec::new() })
    }

    fn loop_section(&self) -> usize {
        usize::from(self.header.has_intro_section())
    }

    fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if self.next >= self.blocks.len() {
            if !self.header.looped {
                return Ok(false);
            }
            let ls = self.loop_section();
            self.next = self.blocks.iter().position(|b| b.1 == ls).context("no loop section")?;
            self.dec = None;
        }
        let (offset, section, samples) = self.blocks[self.next];
        self.next += 1;
        if self.dec.as_ref().is_none_or(|(s, _)| *s != section) {
            self.dec = Some((section, Dec::new(&self.header)?));
        }
        let block = Block::read(&self.data, offset)?;
        let Some((_, dec)) = self.dec.as_mut() else { bail!("no decoder") };
        dec.decode(block.payload, samples, usize::from(self.header.channels), out, &mut self.tmp)?;
        Ok(true)
    }
}

impl Dec {
    fn new(header: &SoundHeader) -> Result<Self> {
        let ch = header.channels;
        Ok(match header.codec {
            Codec::EaXma => Dec::Xma(ssx_formats::xma_decode::SectionDecoder::new(
                ch,
                ssx_formats::xma::rate_index_for(header.sample_rate),
            )?),
            Codec::EaLayer3V1 => Dec::L3(ssx_formats::ealayer3_decode::SectionDecoder::new(ch)?),
            c => bail!("codec {c:?} not on the disc"),
        })
    }

    fn decode(
        &mut self,
        payload: &[u8],
        samples: u32,
        channels: usize,
        out: &mut Vec<f32>,
        tmp: &mut Vec<f32>,
    ) -> Result<()> {
        match self {
            Dec::Xma(d) => {
                d.decode_block(payload, samples, out)?;
            }
            Dec::L3(d) => {
                tmp.clear();
                let n = d.decode_payload(payload, tmp)?.min(samples as usize);
                out.extend_from_slice(&tmp[..n * channels]);
            }
        }
        Ok(())
    }
}

pub(super) struct StreamCursor {
    reader: Reader,
    pub(super) header: SoundHeader,
    file: Arc<sfx_stream::StreamFile>,
    kind: sfx_stream::BlockKind,
    loop_at: Option<u64>,
    dec: Option<(u64, Dec)>,
    tmp: Vec<f32>,
}

pub(super) enum Step {
    Decoded,
    Pending,
    End,
}

const LOOP_REQUESTS_AHEAD: usize = 2;

enum Reader {
    Own(Box<sfx_stream::Slot>),
    Pooled(sfx_stream::Lease),
    Refused,
    Done,
}

pub(super) enum SlotFrom<'a> {
    Own(sfx_stream::Ring),
    Pool(&'a sfx_stream::Pool, f32, &'a str),
    Nothing,
}

impl StreamCursor {
    pub(super) fn open(
        file: Arc<sfx_stream::StreamFile>,
        header: SoundHeader,
        offset: u64,
        kind: sfx_stream::BlockKind,
        from: SlotFrom<'_>,
    ) -> Result<Self> {
        match header.codec {
            Codec::EaXma | Codec::EaLayer3V1 => {}
            c => bail!("codec {c:?} not on the disc"),
        }
        let loop_at = match (header.looped, kind) {
            (false, _) => None,
            (true, sfx_stream::BlockKind::Sps) => bail!("a looped SPS sound (none is on the disc)"),
            (true, sfx_stream::BlockKind::V0) if header.has_intro_section() => {
                let rel = header.loop_offset.context("looped stream without loop offset")?;
                Some(offset + u64::from(rel))
            }
            (true, sfx_stream::BlockKind::V0) => Some(offset),
        };
        let reader = match from {
            SlotFrom::Own(ring) => Reader::Own(Box::new(sfx_stream::Slot::new(ring)?)),
            SlotFrom::Pool(pool, priority, label) => {
                pool.acquire(sfx_stream::new_owner(), priority, label)?.map_or(Reader::Refused, Reader::Pooled)
            }
            SlotFrom::Nothing => Reader::Refused,
        };
        let mut c = Self { reader, header, file, kind, loop_at, dec: None, tmp: Vec::new() };
        let first = (c.file.clone(), offset, kind, loop_at);
        c.on_slot(|slot| {
            slot.queue(first.0.clone(), first.1, first.2);
            queue_loops(slot, &first.0, first.3, first.2);
        });
        Ok(c)
    }

    pub(super) fn refused(&self) -> bool {
        matches!(self.reader, Reader::Refused)
    }

    fn on_slot<R>(&mut self, f: impl FnOnce(&mut sfx_stream::Slot) -> R) -> Option<R> {
        match &mut self.reader {
            Reader::Own(slot) => Some(f(slot)),
            Reader::Pooled(lease) => lease.with(f),
            Reader::Refused | Reader::Done => None,
        }
    }

    pub(super) fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<Step> {
        if matches!(self.reader, Reader::Refused) {
            return Ok(Step::Pending);
        }
        let Self { reader, header, file, kind, loop_at, dec, tmp } = self;
        let parts = Parts { header, file, kind: *kind, loop_at: *loop_at, dec, tmp };
        let step = match reader {
            Reader::Own(slot) => Some(read(slot, parts, out)),
            Reader::Pooled(lease) => lease.with(|slot| read(slot, parts, out)),
            Reader::Refused | Reader::Done => None,
        };
        let step = match step {
            Some(step) => step?,
            None => Step::End,
        };
        if matches!(step, Step::End) && matches!(self.reader, Reader::Pooled(_)) {
            self.reader = Reader::Done;
        }
        Ok(step)
    }
}

fn queue_loops(
    slot: &mut sfx_stream::Slot,
    file: &Arc<sfx_stream::StreamFile>,
    loop_at: Option<u64>,
    kind: sfx_stream::BlockKind,
) {
    if let Some(at) = loop_at {
        while slot.queued() < 1 + LOOP_REQUESTS_AHEAD {
            slot.queue(file.clone(), at, kind);
        }
    }
}

struct Parts<'a> {
    header: &'a SoundHeader,
    file: &'a Arc<sfx_stream::StreamFile>,
    kind: sfx_stream::BlockKind,
    loop_at: Option<u64>,
    dec: &'a mut Option<(u64, Dec)>,
    tmp: &'a mut Vec<f32>,
}

fn read(slot: &mut sfx_stream::Slot, p: Parts<'_>, out: &mut Vec<f32>) -> Result<Step> {
    let Parts { header, file, kind, loop_at, dec, tmp } = p;
    loop {
        let block = match slot.next_block() {
            sfx_stream::Next::Block(b) => b,
            sfx_stream::Next::Pending => return Ok(Step::Pending),
            sfx_stream::Next::Idle => return Ok(Step::End),
            sfx_stream::Next::Failed { error, .. } => bail!("{error}"),
        };
        let (tag, last) = (block.tag, block.last);
        let decoded = match block.ty {
            sfx_stream::BlockType::Data => {
                if dec.as_ref().is_none_or(|(t, _)| *t != tag) {
                    *dec = Some((tag, Dec::new(header)?));
                }
                let Some((_, d)) = dec.as_mut() else { bail!("no decoder") };
                d.decode(block.payload, block.samples, usize::from(header.channels), out, tmp)?;
                true
            }
            sfx_stream::BlockType::Header | sfx_stream::BlockType::End | sfx_stream::BlockType::Other => false,
        };
        if last {
            queue_loops(slot, file, loop_at, kind);
        }
        if decoded {
            return Ok(Step::Decoded);
        }
    }
}

pub struct StreamedSound {
    pub(super) cursor: StreamCursor,
}

impl StreamedSound {
    pub fn open(
        file: Arc<sfx_stream::StreamFile>,
        header: SoundHeader,
        offset: u64,
        kind: sfx_stream::BlockKind,
        ring: sfx_stream::Ring,
    ) -> Result<Self> {
        if header.looped {
            bail!("a looped sound cannot stream into a PCM voice");
        }
        Ok(Self { cursor: StreamCursor::open(file, header, offset, kind, SlotFrom::Own(ring))? })
    }
}

pub(super) struct RamSound {
    pub(super) channels: usize,
    pub(super) looped: bool,
    pub(super) sections: Vec<Vec<f32>>,
}

#[derive(Default)]
pub struct DecodedRam(pub(super) Vec<(usize, Arc<RamSound>)>);

// PORT(choice): RAM sounds decode as a whole set at the bank's load (272 MiB of f32 for the night drop's 24 banks, 0.11
// s on 8 threads), the frames the same; a block decoder resuming inside a RAM sound's single block would let the mixer
// thread decode as it plays; the console's XMA hardware decodes as a voice plays.
pub fn decode_ram_sounds(bank: &Bank) -> Result<DecodedRam> {
    let mut out = Vec::new();
    for (idx, slot) in bank.abk.s10a.sounds.iter().enumerate() {
        let Some(s) = slot else { continue };
        if s.header.storage == Storage::Ram {
            out.push((idx, Arc::new(RamSound::decode(&bank.data, s.offset, &s.header)?)));
        }
    }
    Ok(DecodedRam(out))
}

impl RamSound {
    pub(super) fn decode(data: &Arc<Vec<u8>>, offset: u32, header: &SoundHeader) -> Result<Self> {
        let mut cursor = Cursor::new(data.clone(), header.clone(), offset as usize + header.size)?;
        let mut sections: Vec<Vec<f32>> = Vec::new();
        let mut last = usize::MAX;
        let total = cursor.blocks.len();
        while cursor.next < total {
            let s = cursor.blocks[cursor.next].1;
            if s != last {
                sections.push(Vec::new());
                last = s;
            }
            cursor.decode_next(sections.last_mut().unwrap())?;
        }
        Ok(Self { channels: usize::from(header.channels), looped: header.looped, sections })
    }

    pub(super) fn frames(&self, s: usize) -> usize {
        self.sections[s].len() / self.channels
    }

    pub(super) fn reader(&self) -> RamReader<'_> {
        let frames = |s: usize| self.sections.get(s).map_or(0, |x| x.len() / self.channels);
        let (n0, ls) = (frames(0), self.sections.len().saturating_sub(1));
        let base = if ls == 0 { 0 } else { n0 };
        RamReader { sound: self, n0, ls, base, nl: frames(ls).max(1), last: None }
    }

    pub(super) fn frame(&self, i: usize) -> Option<&[f32]> {
        let ch = self.channels;
        let n0 = self.frames(0);
        let (s, j) = if i < n0 {
            (0, i)
        } else if !self.looped {
            return None;
        } else {
            let ls = self.sections.len() - 1;
            let base = if ls == 0 { 0 } else { n0 };
            let nl = self.frames(ls).max(1);
            (ls, (i - base) % nl)
        };
        self.sections[s].get(j * ch..j * ch + ch)
    }
}

// PORT(choice): the layout worked out once a block and the loop position carried from read to read (the same frames:
// sfx_ride, fe_audio, movie_audio renders identical as raw floats) instead of up to three 64-bit divisions for each
// of a voice's two reads a sample; the console's SndPlayer1 (0x82A39778) reads its own buffers.
pub(super) struct RamReader<'a> {
    sound: &'a RamSound,
    n0: usize,
    ls: usize,
    base: usize,
    nl: usize,
    last: Option<(usize, usize)>,
}

impl RamReader<'_> {
    #[inline(always)]
    pub(super) fn frame(&mut self, i: usize) -> Option<&[f32]> {
        let ch = self.sound.channels;
        let (s, j) = if i < self.n0 {
            (0, i)
        } else if !self.sound.looped {
            return None;
        } else {
            let j = match self.last {
                Some((li, lj)) if i >= li && i - li < self.nl => {
                    let j = lj + (i - li);
                    if j >= self.nl { j - self.nl } else { j }
                }
                _ => (i - self.base) % self.nl,
            };
            self.last = Some((i, j));
            (self.ls, j)
        };
        self.sound.sections.get(s)?.get(j * ch..j * ch + ch)
    }
}

pub(super) enum Frames {
    Ram(Arc<RamSound>),
    Stream { cursor: Box<StreamCursor>, buf: Vec<f32>, base: usize, ended: bool },
    Package { sample: Arc<crate::sfx_override::Samples48>, looped: bool },
}

pub(super) struct Source {
    pub(super) frames: Frames,
    pub(super) channels: usize,
    pub(super) rate: u32,
    pub(super) num_samples: u32,
    pub(super) looped: bool,
    pub(super) resampler: Resampler,
    pub(super) paused: bool,
    pub(super) finished: bool,
    pub(super) label: String,
    pub(super) fade: Option<(f32, f32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Got {
    Frame,
    End,
    Starved,
}

impl Source {
    pub(super) fn frame(&mut self, i: usize, out: &mut [f32; 8]) -> Got {
        let ch = self.channels;
        match &mut self.frames {
            Frames::Ram(r) => match r.frame(i) {
                Some(f) => {
                    out[..ch].copy_from_slice(f);
                    Got::Frame
                }
                None => Got::End,
            },
            Frames::Package { sample, looped } => {
                let n = sample.frames();
                if n == 0 || (!*looped && i >= n) {
                    return Got::End;
                }
                let j = i % n;
                out[..ch].copy_from_slice(&sample.samples[j * ch..j * ch + ch]);
                Got::Frame
            }
            Frames::Stream { cursor, buf, base, ended } => {
                while *base + buf.len() / ch <= i {
                    if *ended {
                        return Got::End;
                    }
                    let keep = (self.resampler.index as usize).saturating_sub(1).max(*base);
                    if keep > *base + 8192 {
                        let drop = (keep - *base).min(buf.len() / ch);
                        buf.drain(..drop * ch);
                        *base += drop;
                    }
                    match cursor.decode_next(buf) {
                        Ok(Step::Decoded) => {}
                        Ok(Step::Pending) => return Got::Starved,
                        Ok(Step::End) => *ended = true,
                        Err(e) => {
                            ssx_core::eprintln!("sfx: streaming {}: {e:#}", self.label);
                            *ended = true;
                        }
                    }
                }
                let k = (i - *base) * ch;
                out[..ch].copy_from_slice(&buf[k..k + ch]);
                Got::Frame
            }
        }
    }

    pub(super) fn render(&mut self, pitch: f32, out: &mut [Vec<f32>]) -> bool {
        if self.finished {
            return false;
        }
        if self.paused {
            return true;
        }
        let ch = self.channels;
        let step = Resampler::step(pitch * self.rate as f32 / MIXER_RATE as f32);
        let mut native = vec![vec![0.0f32; BLOCK]; ch];
        let (mut a, mut b) = ([0f32; 8], [0f32; 8]);
        let ram = match &self.frames {
            Frames::Ram(r) => Some(r.clone()),
            _ => None,
        };
        let mut ram = ram.as_deref().map(RamSound::reader);
        sfx_dsp::fused(
            #[inline(always)]
            |f| {
                let mut read = |src: &mut Self, i: usize, out: &mut [f32; 8]| match ram.as_mut() {
                    Some(r) => match r.frame(i) {
                        Some(fr) => {
                            out[..ch].copy_from_slice(fr);
                            Got::Frame
                        }
                        None => Got::End,
                    },
                    None => src.frame(i, out),
                };
                for k in 0..BLOCK {
                    let i = self.resampler.index as usize;
                    // TODO(guess): a stream whose next block is not read yet holds its position and is silent for the
                    // rest of this 256-sample block, then tries again — decoded: SndPlayer1's process 0x82A39778 with
                    // an empty ring produces nothing and returns 0 (position +0x1B4, ring read index +0x1DE kept), the
                    // voice's block loop 0x82A2BD40 zero-fills the rest of the block; guessed is the voice's tail: the
                    // loop first runs zeros through the chain (0x82A177D0, up to voice +0x30 / +0x28 frames), moving
                    // the resampler's phase and history, where this resampler waits on its last frames; settle: the
                    // stream voice's +0x28 (the plug-ins' tails summed) and the Resample plug-in on zero input.
                    match read(self, i, &mut a) {
                        Got::Frame => {}
                        Got::End => {
                            self.finished = true;
                            break;
                        }
                        Got::Starved => break,
                    }
                    match read(self, i + 1, &mut b) {
                        Got::Frame => {}
                        Got::End => b = a,
                        Got::Starved => break,
                    }
                    let g = match &mut self.fade {
                        Some((g, d)) => {
                            *g = (*g - *d).max(0.0);
                            *g
                        }
                        None => 1.0,
                    };
                    for (c, n) in native.iter_mut().enumerate() {
                        n[k] = self.resampler.sample_in(f, a[c], b[c]) * g;
                    }
                    self.resampler.advance(step);
                    if self.fade.is_some_and(|f| f.0 <= 0.0) {
                        self.finished = true;
                        break;
                    }
                }
            },
        );
        let conv = sfx_dsp::rechannel(&native, out.len());
        for (o, c) in out.iter_mut().zip(conv) {
            o.copy_from_slice(&c);
        }
        true
    }
}

#[derive(Default)]
pub struct Sources {
    pub(super) voices: Vec<Option<Source>>,
    pub music: Option<Box<dyn crate::sfx::MusicSource>>,
    pub music_rate: u32,
    pub music_tail: Option<Box<dyn crate::sfx::MusicSource>>,
    tail_buf: Vec<f32>,
    music_buf: Vec<f32>,
    music_pos: Resampler,
    pub movie: Option<Arc<crate::sfx::movie::MovieAudio>>,
}

pub const MUSIC_SOURCE: u32 = u32::MAX;

impl SourceHost for Sources {
    fn fill(&mut self, source: u32, pitch: f32, out: &mut [Vec<f32>]) -> bool {
        if source == crate::sfx::movie::MOVIE_SOURCE {
            return self.movie.as_ref().is_some_and(|m| m.fill(pitch, out));
        }
        if source == MUSIC_SOURCE {
            let rate = self.music_rate;
            if self.music.is_none() && self.music_tail.is_none() {
                return true;
            }
            let step = Resampler::step(pitch * rate as f32 / MIXER_RATE as f32);
            let mut native = vec![vec![0.0f32; BLOCK]; 2];
            let (music, tail) = (&mut self.music, &mut self.music_tail);
            let (buf, tail_buf, pos) = (&mut self.music_buf, &mut self.tail_buf, &mut self.music_pos);
            // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the interpolation's multiply-add
            // a sample is the instruction in line instead of a call to the runtime's `fmaf` (the same single rounding:
            // the same samples bit for bit); the flag is not used (it stays fused without FMA too, as before); the
            // console has the Resample kernel 0x82A17360, whose multiply-add this is.
            sfx_dsp::fused(
                #[inline(always)]
                |_| {
                    for k in 0..BLOCK {
                        let i = pos.index as usize;
                        while buf.len() / 2 < i + 2 {
                            let n = buf.len();
                            buf.resize(n + 2048, 0.0);
                            if let Some(m) = music.as_mut() {
                                m.render(&mut buf[n..]);
                            }
                            if let Some(t) = tail.as_mut() {
                                tail_buf.resize(2048, 0.0);
                                t.render(tail_buf);
                                for (o, x) in buf[n..].iter_mut().zip(tail_buf.iter()) {
                                    *o += x;
                                }
                            }
                        }
                        for (c, n) in native.iter_mut().enumerate() {
                            n[k] = pos.sample_in(true, buf[2 * i + c], buf[2 * i + 2 + c]);
                        }
                        pos.advance(step);
                    }
                },
            );
            let used = (self.music_pos.index as usize).min(self.music_buf.len() / 2);
            self.music_buf.drain(..used * 2);
            self.music_pos.index -= used as u64;
            let conv = sfx_dsp::rechannel(&native, out.len());
            for (o, c) in out.iter_mut().zip(conv) {
                o.copy_from_slice(&c);
            }
            return true;
        }
        match self.voices.get_mut(source as usize).and_then(Option::as_mut) {
            Some(s) => s.render(pitch, out),
            None => false,
        }
    }
}
