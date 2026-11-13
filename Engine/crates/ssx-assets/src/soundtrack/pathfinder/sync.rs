use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct StreamStatus {
    pub node: Option<u16>,
    pub duration: u32,
    pub bar: u32,
    pub beat: u32,
    pub remaining: u32,
    pub to_bar: u32,
    pub to_beat: u32,
    pub beat_number: u32,
}

impl Pathfinder {
    // TODO(guess): the status is computed from the stream's position at every call; the exe rebuilds its block
    // (0x835CCC08) only while the stream's +0x20 is set — by its queue (0x835CD4E8 / 0x835CDBE8), start (0x835CDAF0)
    // and stop (through vtable +8, 0x835D18E8), cleared by the rebuild — and reads it as it was in between; no other
    // setter was found (the stores to +0x20 in 0x835C4000…0x835D2000; no call of +8 or +0x14 on a player's +0x12C);
    // settle: 0x835C8400's output frame by frame in a run of the exe, or a caller of the vtables' +8 outside Pathfinder.
    pub fn status(&self, i: usize) -> StreamStatus {
        let Some(p) = self.players.get(i).and_then(Option::as_ref) else { return StreamStatus::default() };
        let mut s = match p.stream.items.front() {
            Some(head) => {
                let node = &self.mpf.nodes[head.node as usize];
                let dur = head.duration_ms;
                let played = (p.stream.head_frames * 1000.0 / f64::from(SAMPLE_RATE)) as u32;
                let remaining = dur.saturating_sub(played);
                let bars = u32::from(node.bars).max(1);
                let bar = dur / bars;
                let beat = if node.beats_per_bar > 0 { dur / (u32::from(node.beats_per_bar) * bars) } else { bar };
                StreamStatus {
                    node: Some(head.node),
                    duration: dur,
                    bar,
                    beat,
                    remaining,
                    to_bar: if bar > 0 { (remaining + 1) % bar } else { 0 },
                    to_beat: if beat > 0 { (remaining + 1) % beat } else { 0 },
                    beat_number: (dur - remaining).checked_div(beat).map_or(0, |b| b + 1),
                }
            }
            None => StreamStatus::default(),
        };
        if p.pitch != PITCH_ONE {
            if p.pitch == 0 {
                let m = 0x7FFF_FFFF;
                (s.duration, s.bar, s.beat, s.remaining, s.to_bar, s.to_beat) = (m, m, m, m, m, m);
            } else {
                let k = f64::from(PITCH_ONE) / f64::from(p.pitch);
                for v in [&mut s.duration, &mut s.bar, &mut s.beat, &mut s.remaining, &mut s.to_bar, &mut s.to_beat] {
                    *v = (f64::from(*v) * k) as u32;
                }
            }
        }
        s
    }

    pub fn first_status(&self) -> Option<StreamStatus> {
        let i = self.players.iter().position(Option::is_some)?;
        Some(self.status(i))
    }

    pub(super) fn beat_frames(&self, m: usize) -> Option<(u64, f64)> {
        let p = self.players[m].as_ref()?;
        let head = p.stream.items.front()?;
        let node = &self.mpf.nodes[head.node as usize];
        let pieces = (u32::from(node.beats_per_bar) * u32::from(node.bars)).max(1);
        let beat = f64::from(head.frames) / f64::from(pieces);
        let next = (p.stream.head_frames / beat).floor() + 1.0;
        let ratio = f64::from(p.pitch.max(1)) / f64::from(PITCH_ONE);
        Some((((next * beat - p.stream.head_frames) / ratio).round() as u64, beat / ratio))
    }

    pub fn consume(&mut self, i: usize, frames: f64) -> Vec<Queued> {
        let mut ended = Vec::new();
        let Some(p) = self.players.get_mut(i).and_then(Option::as_mut) else { return ended };
        p.stream.head_frames += frames;
        while let Some(head) = p.stream.items.front() {
            if p.stream.head_frames + 1e-6 < f64::from(head.frames) {
                break;
            }
            p.stream.head_frames -= f64::from(head.frames);
            ended.push(p.stream.items.pop_front().unwrap());
        }
        if p.stream.items.is_empty() {
            p.stream.head_frames = 0.0;
        }
        ended
    }

    pub(crate) fn flush(&mut self, i: usize) {
        if let Some(p) = self.players[i].as_mut() {
            p.stream.items.clear();
            p.stream.head_frames = 0.0;
            p.sync_frame = None;
            p.current = -1;
            p.beat = -1;
        }
    }
}

pub fn sync_start(node: &mpf::Node, duration_ms: u32, mode: u8, master: &StreamStatus) -> (u32, i32) {
    let mut beat = duration_ms as f32;
    if node.beats_per_bar > 0 {
        beat /= (u32::from(node.beats_per_bar) * u32::from(node.bars)) as f32;
    }
    if node.word3 & mpf::FLAG_MASTER_BEAT != 0 {
        beat = master.beat as f32;
    }
    let elapsed = master.duration as i32 - master.remaining as i32;
    let rem_beats = (master.remaining as f32 * (1.0 / beat)) as u32;
    let el_beats = (elapsed as f32 * (1.0 / beat)) as u32;
    let bpb = u32::from(node.beats_per_bar).max(1);
    let (start, piece) = match mode {
        1 => {
            let next = el_beats + 1;
            (((next as f32 * beat) - elapsed as f32) as i32, (next % bpb) as i32 + 1)
        }
        2 => {
            let whole = ((rem_beats as f32) * beat) as u32;
            let start = if whole == 0 { 0 } else { master.remaining % whole };
            (start as i32, (bpb - rem_beats % bpb) as i32 + 1)
        }
        3 => ((((el_beats + 1) as f32) * beat - elapsed as f32) as i32, 1),
        _ => (0, -1),
    };
    (start.max(0) as u32, piece)
}
