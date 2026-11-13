use super::*;

static DB_TO_GAIN: LazyLock<Vec<f32>> =
    LazyLock::new(|| (-512..512).map(|e| 10f64.powf(e as f64 / 20.0) as f32).collect());

fn db_to_gain(e: i32) -> f32 {
    match usize::try_from(e + 512) {
        Ok(i) if i < DB_TO_GAIN.len() => DB_TO_GAIN[i],
        _ => 10f64.powf(f64::from(e) / 20.0) as f32,
    }
}

fn large_value(r: &mut BitReader) -> u32 {
    let mut n = 8;
    if r.bit() {
        n += 8;
        if r.bit() {
            n += 8;
            if r.bit() {
                n += 7;
            }
        }
    }
    r.bits(n)
}

impl StreamDecoder {
    pub(super) fn subframe(&mut self, r: &mut BitReader, stats: &mut FrameStats) -> Result<()> {
        let n = self.channels;
        let (mut offset, mut len) = (FRAME_SAMPLES, FRAME_SAMPLES);
        for c in &self.ch[..n] {
            if c.decoded_samples < offset {
                offset = c.decoded_samples;
                len = c.subframe_len.get(c.cur_subframe).copied().unwrap_or(0);
            }
        }
        if len == 0 {
            return Err(Error::invalid("XMA subframe: tiling inconsistent"));
        }
        self.num_sub_channels = 0;
        for c in 0..n {
            let ch = &mut self.ch[c];
            ch.grouped = false;
            if ch.decoded_samples == offset && ch.subframe_len.get(ch.cur_subframe) == Some(&len) {
                ch.decoded_samples += len;
                self.sub_channels[self.num_sub_channels] = c;
                self.num_sub_channels += 1;
            }
        }
        self.subframe_len = len;
        self.table_idx = (FRAME_SAMPLES / len).trailing_zeros() as usize;
        stats.subframes[self.table_idx] += 1;
        if r.bit() {
            stats.ext_header += 1;
            return Err(Error::invalid("XMA subframe with an extended header (none on the disc)"));
        }
        if r.bit() {
            return Err(Error::invalid("XMA subframe: reserved bit set"));
        }
        self.channel_transform(r, stats)?;
        let mut any = false;
        for &c in &self.sub_channels[..self.num_sub_channels] {
            self.ch[c].transmit_coefs = r.bit();
            any |= self.ch[c].transmit_coefs;
        }
        if any {
            self.transmit_num_vec_coeffs = r.bit();
            if self.transmit_num_vec_coeffs {
                stats.vec_count_sent += 1;
                return Err(Error::invalid("XMA subframe with transmitted vector counts (none on the disc)"));
            } else {
                for &c in &self.sub_channels[..self.num_sub_channels] {
                    self.ch[c].num_vec_coeffs = len;
                }
            }
            let mut step = r.sbits(6);
            let mut quant = QUANT_BASE + step;
            if step == -32 || step == 31 {
                stats.quant_escapes += 1;
                let sign = if step == 31 { 0 } else { -1 };
                let mut extra = 0;
                while r.pos() + 5 < r.end() {
                    step = r.bits(5) as i32;
                    if step != 31 {
                        break;
                    }
                    extra += 31;
                }
                quant += ((extra + step) ^ sign) - sign;
            }
            if self.num_sub_channels == 1 {
                self.ch[self.sub_channels[0]].quant_step = quant;
            } else {
                let modifier_bits = r.bits(3);
                for &c in &self.sub_channels[..self.num_sub_channels] {
                    let ch = &mut self.ch[c];
                    ch.quant_step = quant;
                    if r.bit() {
                        ch.quant_step += if modifier_bits > 0 { r.bits(modifier_bits) as i32 + 1 } else { 1 };
                    }
                }
            }
            self.scale_factors(r, stats)?;
        }
        for i in 0..self.num_sub_channels {
            let c = self.sub_channels[i];
            if self.ch[c].transmit_coefs && r.pos() < r.end() {
                self.coefficients(r, c, stats)?;
            } else {
                self.ch[c].coeffs[..len].fill(0.0);
            }
        }
        self.reconstruct(offset, any);
        Ok(())
    }

    fn channel_transform(&mut self, r: &mut BitReader, stats: &mut FrameStats) -> Result<()> {
        self.num_groups = 0;
        if self.channels == 1 {
            return Ok(());
        }
        if r.bit() {
            return Err(Error::invalid("XMA subframe: channel-transform bit set"));
        }
        let mut remaining = self.num_sub_channels;
        let num_bands = self.bands.num_bands[self.table_idx];
        while remaining > 0 && self.num_groups < self.num_sub_channels {
            let mut g = Group::default();
            for &c in &self.sub_channels[..self.num_sub_channels] {
                if !self.ch[c].grouped {
                    g.channels[g.num_channels] = c;
                    g.num_channels += 1;
                    self.ch[c].grouped = true;
                }
            }
            if g.num_channels == 2 {
                if r.bit() {
                    if r.bit() {
                        return Err(Error::invalid("XMA subframe: unknown channel transform type"));
                    }
                } else {
                    g.transform = true;
                }
            }
            if g.transform {
                if r.bit() {
                    g.transform_band[..num_bands].fill(true);
                    stats.transforms[1] += 1;
                } else {
                    for b in &mut g.transform_band[..num_bands] {
                        *b = r.bit();
                    }
                    stats.transforms[2] += 1;
                }
            } else if g.num_channels == 2 {
                stats.transforms[0] += 1;
            }
            remaining -= g.num_channels.max(1).min(remaining);
            self.groups[self.num_groups] = g;
            self.num_groups += 1;
        }
        Ok(())
    }
}

impl StreamDecoder {
    fn scale_factors(&mut self, r: &mut BitReader, stats: &mut FrameStats) -> Result<()> {
        let codes = &*CODES;
        let (ti, nb) = (self.table_idx, self.bands.num_bands[self.table_idx]);
        for i in 0..self.num_sub_channels {
            let ch = &mut self.ch[self.sub_channels[i]];
            let (cur, last) = (1 - ch.sf_last, ch.sf_last);
            ch.sf_cur = cur;
            if ch.reuse_sf {
                let map = &self.bands.resample[ti][ch.sf_table];
                let prev = ch.saved_sf[last];
                for (dst, &src) in ch.saved_sf[cur].iter_mut().zip(&map[..nb]) {
                    *dst = prev[usize::from(src)];
                }
            }
            if ch.cur_subframe == 0 || r.bit() {
                if !ch.reuse_sf {
                    stats.sf_sets[0] += 1;
                    ch.sf_step = r.bits(2) as i32 + 1;
                    let mut v = 45 / ch.sf_step;
                    for sf in ch.saved_sf[cur].iter_mut().take(nb) {
                        v += i32::from(codes.scale.read(r)) - 60;
                        *sf = v;
                    }
                } else {
                    stats.sf_sets[1] += 1;
                    let mut b = 0;
                    while b < nb {
                        let idx = usize::from(codes.scale_rl.read(r));
                        let (skip, level, negative) = match idx {
                            0 => {
                                let code = r.bits(14);
                                ((code & 0x3F) >> 1, code >> 6, code & 1 == 0)
                            }
                            1 => break,
                            _ => {
                                let (run, level) = codes.scale_rl_runlevel[idx];
                                (u32::from(run), u32::from(level), !r.bit())
                            }
                        };
                        b += skip as usize;
                        if b >= nb {
                            return Err(Error::invalid("XMA scale factors: run past the last band"));
                        }
                        ch.saved_sf[cur][b] += if negative { -(level as i32) } else { level as i32 };
                        b += 1;
                    }
                }
                ch.sf_last = cur;
                ch.sf_table = ti;
                ch.reuse_sf = true;
            } else {
                stats.sf_sets[2] += 1;
                stats.stale_sf += u64::from(!ch.reuse_sf);
                if !ch.reuse_sf {
                    return Err(Error::invalid("XMA subframe reusing scale factors without a set (none on the disc)"));
                }
            }
            ch.max_sf = ch.saved_sf[cur][..nb].iter().copied().max().unwrap_or(0);
        }
        Ok(())
    }

    pub(super) fn coefficients(&mut self, r: &mut BitReader, c: usize, stats: &mut FrameStats) -> Result<()> {
        let codes = &*CODES;
        let len = self.subframe_len;
        let table = usize::from(r.bit());
        let ch = &mut self.ch[c];
        let num_vec = ch.num_vec_coeffs;
        let (mut cur, mut rl_mode, mut zeros) = (0, false, 0);
        while (self.transmit_num_vec_coeffs || !rl_mode) && cur + 3 < num_vec {
            let mut vals = [0u32; 4];
            let idx = u32::from(codes.vec4.read(r));
            if idx == 0 {
                for pair in vals.chunks_mut(2) {
                    let idx2 = u32::from(codes.vec2.read(r));
                    if idx2 == 0 {
                        for v in pair.iter_mut() {
                            *v = u32::from(codes.vec1.read(r));
                            if *v == 100 {
                                *v += large_value(r);
                                stats.vec_escapes += 1;
                            }
                        }
                    } else {
                        pair[0] = (idx2 - 1) >> 4;
                        pair[1] = (idx2 - 1) & 15;
                    }
                }
            } else {
                let s = idx - 1;
                vals = [s >> 12, (s >> 8) & 15, (s >> 4) & 15, s & 15];
            }
            for v in vals {
                if v != 0 {
                    ch.coeffs[cur] = if r.bit() { v as f32 } else { -(v as f32) };
                    zeros = 0;
                } else {
                    ch.coeffs[cur] = 0.0;
                    zeros += 1;
                    rl_mode |= zeros > len >> 8;
                }
                cur += 1;
            }
        }
        if cur < len {
            ch.coeffs[cur..len].fill(0.0);
            let esc_bits = (len - 1).ilog2() + 1;
            let (vlc, rl) = (&codes.coef[table], &codes.coef_runlevel[table]);
            let mut offset = cur;
            while offset < len {
                let code = usize::from(vlc.read(r));
                let (level, negative) = match code {
                    1 => {
                        stats.rl_eob += 1;
                        return Ok(());
                    }
                    0 => {
                        stats.rl_escapes += 1;
                        let level = large_value(r);
                        if r.bit() {
                            if r.bit() {
                                if r.bit() {
                                    return Err(Error::invalid("XMA coefficients: broken escape"));
                                }
                                offset += r.bits(esc_bits) as usize + 4;
                            } else {
                                offset += r.bits(2) as usize + 1;
                            }
                        }
                        (level as f32, !r.bit())
                    }
                    _ => {
                        let (run, level) = rl[code];
                        offset += usize::from(run);
                        (f32::from(level), !r.bit())
                    }
                };
                if offset >= len {
                    return Err(Error::invalid(format!("XMA coefficients: run to {offset} past {len}")));
                }
                ch.coeffs[offset] = if negative { -level } else { level };
                offset += 1;
            }
            stats.rl_full += 1;
        }
        Ok(())
    }

    fn reconstruct(&mut self, offset: usize, any: bool) {
        let (len, ti) = (self.subframe_len, self.table_idx);
        let (nb, edges) = (self.bands.num_bands[ti], &self.bands.offsets[ti]);
        let band = |b: usize| usize::from(edges[b])..usize::from(edges[b + 1]).min(len);
        let base = FRAME_SAMPLES / 2 + offset;
        let tr = &*TRANSFORMS;
        if any {
            for g in &self.groups[..self.num_groups] {
                if !g.transform || g.num_channels != 2 {
                    continue;
                }
                let [a, b] = g.channels;
                let (lo, hi) = self.ch.split_at_mut(1);
                let (x, y) = if a == 0 { (&mut lo[0], &mut hi[0]) } else { (&mut hi[0], &mut lo[0]) };
                debug_assert!(a != b);
                for (k, &transform) in g.transform_band.iter().enumerate().take(nb) {
                    for i in band(k) {
                        if transform {
                            let (m, s) = (x.coeffs[i], y.coeffs[i]);
                            x.coeffs[i] = m - s;
                            y.coeffs[i] = m + s;
                        } else {
                            x.coeffs[i] *= 181.0 / 128.0;
                            y.coeffs[i] *= 181.0 / 128.0;
                        }
                    }
                }
            }
            let mut scaled = [0f32; FRAME_SAMPLES];
            for &c in &self.sub_channels[..self.num_sub_channels] {
                let ch = &mut self.ch[c];
                let sf = &ch.saved_sf[ch.sf_cur];
                for (k, &f) in sf.iter().enumerate().take(nb) {
                    let gain = db_to_gain(ch.quant_step - (ch.max_sf - f) * ch.sf_step);
                    for i in band(k) {
                        scaled[i] = ch.coeffs[i] * gain;
                    }
                }
                tr.imdct[ti].run(&scaled[..len], &mut ch.out[base..base + len]);
            }
        } else {
            for &c in &self.sub_channels[..self.num_sub_channels] {
                self.ch[c].out[base..base + len].fill(0.0);
            }
        }
        for &c in &self.sub_channels[..self.num_sub_channels] {
            let ch = &mut self.ch[c];
            let l = ch.prev_block_len.min(len);
            overlap_window(&mut ch.out, base, &tr.windows[(FRAME_SAMPLES / l).trailing_zeros() as usize]);
            ch.prev_block_len = len;
            ch.cur_subframe += 1;
        }
    }
}
