use super::*;

fn matrix_mix(buf: &[Vec<f32>], gains: &[[f32; 8]], last: &mut Vec<[f32; 8]>, outputs: usize) -> Vec<Vec<f32>> {
    if last.len() != gains.len() {
        *last = gains.to_vec();
    }
    let mut out = vec![vec![0.0; BLOCK]; outputs];
    dsp::fused(
        #[inline(always)]
        |f| {
            for (i, input) in buf.iter().enumerate().take(gains.len()) {
                for (o, dst) in out.iter_mut().enumerate() {
                    let (g0, g1) = (last[i][o], gains[i][o]);
                    if g0 == 0.0 && g1 == 0.0 {
                        continue;
                    }
                    for (k, (d, x)) in dst.iter_mut().zip(input).enumerate() {
                        *d = dsp::madd(f, dsp::ramp_gain_in(f, k, g0, g1), *x, *d);
                    }
                }
            }
        },
    );
    *last = gains.to_vec();
    out
}

impl Graph {
    pub fn process(&mut self, host: &mut dyn SourceHost, ended: &mut Vec<usize>) -> Vec<Vec<f32>> {
        for k in 0..self.order.len() {
            let slot = self.order[k];
            let Some(mut v) = self.voices[slot].take() else { continue };
            if Self::paused(&v) {
                self.voices[slot] = Some(v);
                continue;
            }
            let t0 = self.profile.is_some().then(std::time::Instant::now);
            if !self.run_voice(&mut v, host) {
                ended.push(slot);
            }
            if let (Some(p), Some(t0)) = (self.profile.as_mut(), t0) {
                let e = p.entry(v.name.clone()).or_default();
                e.0 += t0.elapsed().as_nanos() as u64;
                e.1 += 1;
            }
            if let Some((_, m)) = self.meter.as_mut() {
                let e = m.entry(v.name.clone()).or_default();
                e.0 += v.meter;
                e.1 += BLOCK;
            }
            v.first = false;
            self.voices[slot] = Some(v);
        }
        if let Some((n, m)) = self.meter.as_mut() {
            *n += 1;
            if *n % self.meter_blocks == 0 {
                let line: Vec<String> = m
                    .iter()
                    .filter(|e| e.1.0 > 0.0)
                    .map(|(k, (s, c))| format!("{k} {:.4}", (s / *c as f64).sqrt()))
                    .collect();
                self.log.push(format!("meter {}", line.join(", ")));
                m.clear();
            }
        }
        let dac = &mut self.accs[self.dac];
        let out = std::mem::replace(&mut dac.chans, vec![vec![0.0; BLOCK]; 6]);
        (dac.fed, dac.dirty) = (false, false);
        out
    }

    fn resolve_target(&self, key: &(String, u8), target: &mut Option<usize>) -> Option<usize> {
        if target.is_none() {
            *target = self.accs.iter().position(|a| &a.key == key);
        }
        *target
    }

    fn run_voice(&mut self, v: &mut MixVoice, host: &mut dyn SourceHost) -> bool {
        let restable = self.rest && Self::restable(v);
        if restable && v.rest >= REST_BLOCKS && !v.first && self.inputs_quiet(v) {
            self.rest_voice(v);
            v.meter = 0.0;
            return true;
        }
        let mut still = restable;
        let mut alive = true;
        let mut buf: Vec<Vec<f32>> = vec![vec![0.0; BLOCK]];
        let n = v.plugins.len();
        let mut i = 0;
        while i < n {
            let first = v.first;
            let p = &mut v.plugins[i];
            if p.bypass && !matches!(p.stage, Stage::SubMix { .. } | Stage::Source) {
                i += 1;
                continue;
            }
            let t_stage = self.profile.is_some().then(std::time::Instant::now);
            match &mut p.stage {
                Stage::Source => {
                    let mut ch = p.outputs;
                    let mut pitch = 1.0;
                    for q in &v.plugins[i + 1..] {
                        match &q.fourcc {
                            b"Rch0" => ch = q.outputs,
                            b"Rsp0" => pitch = q.attr(0) as f32,
                            _ => break,
                        }
                    }
                    buf = vec![vec![0.0; BLOCK]; ch];
                    if let Some(src) = v.source {
                        alive = host.fill(src, pitch, &mut buf);
                    }
                }
                Stage::Pass => {
                    if buf.len() != p.outputs && !matches!(&p.fourcc, b"Rch0" | b"Rsp0") {
                        buf = dsp::rechannel(&buf, p.outputs);
                    }
                }
                Stage::Gain { last } => {
                    let new = p.attrs[0] as f32;
                    let old = if first { new } else { last.unwrap_or(new) };
                    buf.iter_mut().for_each(|c| dsp::apply_gain(c, old, new));
                    *last = Some(new);
                }
                Stage::Fader { fade } => match fade.as_mut() {
                    Some(f) => {
                        let gains: Vec<f32> = (0..buf.first().map_or(0, Vec::len)).map(|_| f.next()).collect();
                        for c in &mut buf {
                            c.iter_mut().zip(&gains).for_each(|(x, g)| *x *= g);
                        }
                        if f.done >= f.n {
                            p.attrs[0] = f64::from(f.to);
                            *fade = None;
                        }
                    }
                    None => {
                        let g = p.attrs[0] as f32;
                        buf.iter_mut().for_each(|c| dsp::apply_gain(c, g, g));
                    }
                },
                Stage::Send { last, target, key } => {
                    let new = p.attrs[0] as f32;
                    let old = if first { new } else { last.unwrap_or(new) };
                    *last = Some(new);
                    if let Some(t) = self.resolve_target(key, target) {
                        if !still {
                            dsp::send_into(&buf, &mut self.accs[t].chans, old, new);
                            self.accs[t].dirty = true;
                        }
                        self.accs[t].fed = true;
                    }
                }
                Stage::Route { last, target, key, window } => {
                    let new = p.attrs[0] as f32;
                    let old = if first { new } else { last.unwrap_or(new) };
                    *last = Some(new);
                    if let Some(t) = self.resolve_target(key, target) {
                        if !still {
                            dsp::route_into(&buf, &mut self.accs[t].chans, *window, old, new);
                            self.accs[t].dirty = true;
                        }
                        self.accs[t].fed = true;
                    }
                }
                Stage::SubMix { input } => {
                    buf = vec![vec![0.0; BLOCK]; p.outputs];
                    if let Some(a) = *input {
                        let acc = &mut self.accs[a];
                        if acc.dirty {
                            for (d, s) in buf.iter_mut().zip(acc.chans.iter_mut()) {
                                d.copy_from_slice(s);
                                s.fill(0.0);
                            }
                        }
                        (acc.fed, acc.dirty) = (false, false);
                    }
                }
                Stage::Filter(kind, f) => f.process(
                    *kind,
                    p.attrs[0],
                    p.attrs.get(1).copied().unwrap_or(1.0),
                    p.attrs.get(2).copied().unwrap_or(1.0),
                    &mut buf,
                ),
                Stage::Compressor { state, side } => {
                    let setup =
                        dsp::CompressorSetup::new(p.attrs[0], p.attrs[1], p.attrs[2], p.attrs[3], p.attrs[4] as i32);
                    let mut sc = None;
                    if let Some(a) = *side {
                        let acc = &mut self.accs[a];
                        if acc.fed {
                            let n = acc.chans.len();
                            sc = Some(std::mem::replace(&mut acc.chans, vec![vec![0.0; BLOCK]; n]));
                            (acc.fed, acc.dirty) = (false, false);
                        }
                    }
                    state.process(p.attrs[0], p.attrs[1], &setup, &mut buf, sc.as_deref());
                    still &= state.settled;
                }
                Stage::Limiter(state) => {
                    let release = p.attrs.get(1).copied().unwrap_or(0.1).clamp(0.0, 10.0);
                    let mode = p.attrs.get(2).copied().unwrap_or(1.0) as i32;
                    let setup = dsp::CompressorSetup::new(p.attrs[0], 10.0, 0.05, release, mode);
                    state.process(p.attrs[0], 10.0, &setup, &mut buf, None);
                    still &= state.settled;
                }
                Stage::Clip => dsp::distortion_clip(p.attrs[0], &mut buf),
                Stage::Matrix { last } => {
                    let mut out = vec![vec![0.0; BLOCK]; p.outputs];
                    for (c, dst) in out.iter_mut().enumerate() {
                        let a = dsp::matrix_panner_attr(p.outputs, c).unwrap_or(0);
                        let new = p.attrs.get(a).copied().unwrap_or(1.0) as f32;
                        dst.copy_from_slice(&buf[0]);
                        dsp::apply_gain(dst, if first { new } else { last[c] }, new);
                        last[c] = new;
                    }
                    buf = out;
                }
                Stage::Pan { last } => {
                    let g = Self::pan_gains(&p.attrs, &p.ctor, p.outputs, buf.len());
                    buf = matrix_mix(&buf, &g, last, p.outputs);
                }
                Stage::Pan3d(pan) => pan.process(first, p.attrs[0] as f32, &mut buf),
                Stage::Silent => buf = vec![vec![0.0; BLOCK]; p.outputs],
                Stage::ReverbIr(r) => {
                    let f = |k: usize| p.attrs.get(k).copied().unwrap_or(0.0);
                    let a = dsp::reverb_ir::ReverbAttrs {
                        gain: [f(0) as f32, f(1) as f32],
                        env_type: [f(2) as i32, f(5) as i32],
                        env_start: [f(3) as f32, f(6) as f32],
                        env_length: [f(4) as f32, f(7) as f32],
                    };
                    if buf.len() != p.outputs {
                        buf = dsp::rechannel(&buf, p.outputs);
                    }
                    r.process(&a, &mut buf);
                }
                Stage::BandPass(f) => f.process(p.attrs[0], p.attrs[1], &mut buf),
                Stage::FreqShift(f) => f.process(p.attrs[0], &mut buf),
                Stage::Delay(d) => d.process(p.attrs[0], p.attrs[1], &mut buf),
                Stage::Chorus(m) => {
                    m.chorus(p.attrs[0] as i32, p.attrs[1], p.attrs[2], p.attrs[3]);
                    m.process(&mut buf);
                }
                Stage::Vibrato(m) => {
                    m.vibrato(p.attrs[0], p.attrs[1]);
                    m.process(&mut buf);
                }
                Stage::Reverb(r) => r.process(p.attrs[0], p.attrs[1], p.attrs[2], &mut buf),
                Stage::BeatDetect(b) => {
                    b.process(p.attrs[5], p.attrs[6], &mut buf);
                    p.attrs[..5].copy_from_slice(&b.outputs());
                }
            }
            if let (Some(prof), Some(t0)) = (self.profile.as_mut(), t_stage) {
                let e = prof.entry(format!("stage {}", String::from_utf8_lossy(&v.plugins[i].fourcc))).or_default();
                e.0 += t0.elapsed().as_nanos() as u64;
                e.1 += 1;
            }
            if still && !matches!(v.plugins[i].stage, Stage::Send { .. } | Stage::Route { .. }) {
                still = quiet(&buf);
            }
            i += 1;
        }
        v.rest = if still { v.rest.saturating_add(1) } else { 0 };
        v.meter = if self.meter.is_some() { buf.iter().flatten().map(|x| f64::from(x * x)).sum() } else { 0.0 };
        alive
    }

    fn paused(v: &MixVoice) -> bool {
        v.plugins.iter().any(|p| &p.fourcc == b"Pau0" && p.attr(0) != 0.0)
    }

    pub fn voice_paused(&self, slot: usize) -> bool {
        self.voices.get(slot).and_then(Option::as_ref).is_some_and(Self::paused)
    }

    fn restable(v: &MixVoice) -> bool {
        v.source.is_none()
            && v.plugins.iter().all(|p| {
                matches!(
                    p.stage,
                    Stage::Pass
                        | Stage::Gain { .. }
                        | Stage::Fader { .. }
                        | Stage::Send { .. }
                        | Stage::Route { .. }
                        | Stage::SubMix { .. }
                        | Stage::Filter(..)
                        | Stage::Compressor { .. }
                        | Stage::Limiter(_)
                        | Stage::Clip
                        | Stage::Matrix { .. }
                        | Stage::Pan { .. }
                        | Stage::Pan3d(_)
                        | Stage::Silent
                )
            })
    }

    fn inputs_quiet(&self, v: &MixVoice) -> bool {
        v.plugins.iter().all(|p| match &p.stage {
            Stage::SubMix { input: Some(a) } => !self.accs[*a].dirty,
            Stage::Compressor { side: Some(a), .. } => p.bypass || !self.accs[*a].dirty,
            Stage::Pan3d(pan) => p.bypass || pan.waiting(),
            _ => true,
        })
    }

    fn rest_voice(&mut self, v: &mut MixVoice) {
        let mut ch = 1;
        for p in &mut v.plugins {
            if p.bypass && !matches!(p.stage, Stage::SubMix { .. }) {
                continue;
            }
            match &mut p.stage {
                Stage::Pass => {
                    if !matches!(&p.fourcc, b"Rch0" | b"Rsp0") {
                        ch = p.outputs;
                    }
                }
                Stage::Gain { last } => *last = Some(p.attrs[0] as f32),
                Stage::Fader { fade } => {
                    if let Some(f) = fade.as_mut() {
                        for _ in 0..BLOCK {
                            f.next();
                        }
                        if f.done >= f.n {
                            p.attrs[0] = f64::from(f.to);
                            *fade = None;
                        }
                    }
                }
                Stage::Send { last, target, key } | Stage::Route { last, target, key, .. } => {
                    *last = Some(p.attrs[0] as f32);
                    if let Some(t) = self.resolve_target(key, target) {
                        self.accs[t].fed = true;
                    }
                }
                Stage::SubMix { input } => {
                    ch = p.outputs;
                    if let Some(a) = *input {
                        self.accs[a].fed = false;
                    }
                }
                Stage::Filter(kind, f) => f.rest(*kind, p.attrs[0], p.attrs.get(1).copied().unwrap_or(1.0)),
                Stage::Compressor { state, side } => {
                    if let Some(a) = *side {
                        self.accs[a].fed = false;
                    }
                    state.rest(p.attrs[0], p.attrs[1]);
                }
                Stage::Limiter(state) => state.rest(p.attrs[0], 10.0),
                Stage::Matrix { last } => {
                    for (c, l) in last.iter_mut().enumerate().take(p.outputs) {
                        let a = dsp::matrix_panner_attr(p.outputs, c).unwrap_or(0);
                        *l = p.attrs.get(a).copied().unwrap_or(1.0) as f32;
                    }
                    ch = p.outputs;
                }
                Stage::Pan { last } => {
                    *last = Self::pan_gains(&p.attrs, &p.ctor, p.outputs, ch);
                    ch = p.outputs;
                }
                Stage::Pan3d(pan) => {
                    pan.rest(p.attrs[0] as f32);
                    ch = pan.outputs();
                }
                Stage::Silent => ch = p.outputs,
                _ => {}
            }
        }
    }

    fn pan_gains(attrs: &[f64], ctor: &[f64], outputs: usize, inputs: usize) -> Vec<[f32; 8]> {
        let a = |k: usize| attrs.get(k).copied().unwrap_or(0.0);
        let params = dsp::PanParams {
            angle: a(0),
            distance: a(1),
            size: a(2),
            twist: a(3),
            center: a(4),
            main: a(5),
            source_front: a(7),
            source_rear: a(8),
        };
        dsp::pan2d_gains(&params, inputs, outputs, ctor[2] as i32, ctor[0] as f32, ctor[1] as f32)
    }
}

pub const DAC_CHANNELS: usize = 6;
const XAUDIO_ORDER: [usize; DAC_CHANNELS] = [0, 2, 1, 5, 3, 4];

pub fn interleave_dac(dac: &[Vec<f32>], out: &mut [f32]) {
    for (k, frame) in out.as_chunks_mut::<DAC_CHANNELS>().0.iter_mut().take(BLOCK).enumerate() {
        for (o, &c) in frame.iter_mut().zip(&XAUDIO_ORDER) {
            *o = dac[c][k];
        }
    }
}

// TODO(guess): the console's own stereo fold is outside the disc (EA's text: the consoles' folds "seem to produce the same results as EAAudioCore on PC", whose fold this is); settle: a capture of a console's stereo output. Six or more device channels take the 5.1 unfolded (`sfx::output`).
#[inline]
pub fn fold(fl: f32, c: f32, fr: f32, rl: f32, rr: f32) -> (f32, f32) {
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    (K * K.mul_add(c + rl, fl), K * K.mul_add(c + rr, fr))
}

pub fn downmix(dac: &[Vec<f32>], out: &mut [f32]) {
    for k in 0..BLOCK.min(out.len() / 2) {
        let (l, r) = fold(dac[0][k], dac[1][k], dac[2][k], dac[3][k], dac[4][k]);
        out[2 * k] += l;
        out[2 * k + 1] += r;
    }
}

pub fn fold_interleaved(six: &[f32], out: &mut [f32]) {
    // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the fold's 2 multiply-adds a frame are
    // the instruction in line instead of calls to the runtime's `fmaf` (the same single rounding: the same samples bit
    // for bit); the flag is not used (they stay fused without FMA too, as before); the console's own fold is not on
    // the disc (this is EAAudioCore's, see `fold`).
    dsp::fused(
        #[inline(always)]
        |_| {
            for (f, o) in six.as_chunks::<DAC_CHANNELS>().0.iter().zip(out.as_chunks_mut::<2>().0.iter_mut()) {
                let (l, r) = fold(f[0], f[2], f[1], f[4], f[5]);
                o[0] += l;
                o[1] += r;
            }
        },
    );
}
