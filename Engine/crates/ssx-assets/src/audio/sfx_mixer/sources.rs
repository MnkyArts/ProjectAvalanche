use super::*;

enum Dec {
    Xma(ssx_formats::xma_decode::SectionDecoder),
    L3(ssx_formats::ealayer3_decode::SectionDecoder),
}

#[derive(Clone)]
pub(super) enum Data {
    Abk(Arc<Vec<u8>>),
    Ast(Arc<crate::music::Bytes>),
}

impl Data {
    pub(super) fn bytes(&self) -> &[u8] {
        match self {
            Data::Abk(v) => v,
            Data::Ast(b) => b,
        }
    }
}

pub(super) struct Cursor {
    data: Data,
    pub(super) header: SoundHeader,
    blocks: Vec<(usize, usize, u32)>,
    next: usize,
    dec: Option<(usize, Dec)>,
    tmp: Vec<f32>,
}

impl Cursor {
    pub(super) fn new(data: Data, header: SoundHeader, body: usize) -> Result<Self> {
        let sb = read_v0_blocks(&header, data.bytes(), body)?;
        let blocks = sb
            .sections
            .iter()
            .enumerate()
            .flat_map(|(s, sec)| sec.iter().map(move |b| (b.offset, s, b.samples)))
            .collect();
        Ok(Self { data, header, blocks, next: 0, dec: None, tmp: Vec::new() })
    }

    fn from_blocks(data: Data, header: SoundHeader, sb: &ssx_formats::eaac::SoundBlocks) -> Self {
        let blocks = sb
            .sections
            .iter()
            .enumerate()
            .flat_map(|(s, sec)| sec.iter().map(move |b| (b.offset, s, b.samples)))
            .collect();
        Self { data, header, blocks, next: 0, dec: None, tmp: Vec::new() }
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
            let ch = self.header.channels;
            self.dec = Some((
                section,
                match self.header.codec {
                    Codec::EaXma => Dec::Xma(ssx_formats::xma_decode::SectionDecoder::new(
                        ch,
                        ssx_formats::xma::rate_index_for(self.header.sample_rate),
                    )?),
                    Codec::EaLayer3V1 => Dec::L3(ssx_formats::ealayer3_decode::SectionDecoder::new(ch)?),
                    c => bail!("codec {c:?} not on the disc"),
                },
            ));
        }
        let block = Block::read(self.data.bytes(), offset)?;
        match &mut self.dec.as_mut().unwrap().1 {
            Dec::Xma(d) => {
                d.decode_block(block.payload, samples, out)?;
            }
            Dec::L3(d) => {
                self.tmp.clear();
                let n = d.decode_payload(block.payload, &mut self.tmp)?.min(samples as usize);
                out.extend_from_slice(&self.tmp[..n * usize::from(self.header.channels)]);
            }
        }
        Ok(true)
    }
}

pub struct StreamedSound {
    pub(super) cursor: Cursor,
}

impl StreamedSound {
    pub fn new(
        data: Arc<crate::music::Bytes>,
        header: SoundHeader,
        blocks: &ssx_formats::eaac::SoundBlocks,
    ) -> Result<Self> {
        if header.looped {
            bail!("a looped sound cannot stream into a PCM voice");
        }
        match header.codec {
            Codec::EaXma | Codec::EaLayer3V1 => {}
            c => bail!("codec {c:?} not on the disc"),
        }
        Ok(Self { cursor: Cursor::from_blocks(Data::Ast(data), header, blocks) })
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
        let mut cursor = Cursor::new(Data::Abk(data.clone()), header.clone(), offset as usize + header.size)?;
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

pub(super) enum Frames {
    Ram(Arc<RamSound>),
    Stream { cursor: Box<Cursor>, buf: Vec<f32>, base: usize, ended: bool },
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

impl Source {
    pub(super) fn frame(&mut self, i: usize, out: &mut [f32; 8]) -> bool {
        let ch = self.channels;
        match &mut self.frames {
            Frames::Ram(r) => match r.frame(i) {
                Some(f) => {
                    out[..ch].copy_from_slice(f);
                    true
                }
                None => false,
            },
            Frames::Package { sample, looped } => {
                let n = sample.frames();
                if n == 0 || (!*looped && i >= n) {
                    return false;
                }
                let j = i % n;
                out[..ch].copy_from_slice(&sample.samples[j * ch..j * ch + ch]);
                true
            }
            Frames::Stream { cursor, buf, base, ended } => {
                while *base + buf.len() / ch <= i {
                    if *ended {
                        return false;
                    }
                    let keep = (self.resampler.index as usize).saturating_sub(1).max(*base);
                    if keep > *base + 8192 {
                        let drop = (keep - *base).min(buf.len() / ch);
                        buf.drain(..drop * ch);
                        *base += drop;
                    }
                    match cursor.decode_next(buf) {
                        Ok(true) => {}
                        Ok(false) => *ended = true,
                        Err(e) => {
                            ssx_core::eprintln!("sfx: decoding {}: {e:#}", self.label);
                            *ended = true;
                        }
                    }
                }
                let k = (i - *base) * ch;
                out[..ch].copy_from_slice(&buf[k..k + ch]);
                true
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
        sfx_dsp::fused(
            #[inline(always)]
            |f| {
                for k in 0..BLOCK {
                    let i = self.resampler.index as usize;
                    if !self.frame(i, &mut a) {
                        self.finished = true;
                        break;
                    }
                    if !self.frame(i + 1, &mut b) {
                        b = a;
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
            for k in 0..BLOCK {
                let i = self.music_pos.index as usize;
                while self.music_buf.len() / 2 < i + 2 {
                    let n = self.music_buf.len();
                    self.music_buf.resize(n + 2048, 0.0);
                    if let Some(m) = self.music.as_mut() {
                        m.render(&mut self.music_buf[n..]);
                    }
                    if let Some(t) = self.music_tail.as_mut() {
                        self.tail_buf.resize(2048, 0.0);
                        t.render(&mut self.tail_buf);
                        for (o, x) in self.music_buf[n..].iter_mut().zip(&self.tail_buf) {
                            *o += x;
                        }
                    }
                }
                for (c, n) in native.iter_mut().enumerate() {
                    n[k] = self.music_pos.sample(self.music_buf[2 * i + c], self.music_buf[2 * i + 2 + c]);
                }
                self.music_pos.advance(step);
            }
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
