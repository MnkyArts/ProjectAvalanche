use glam::{Quat, Vec3};
use ssx_assets::anim_eval::{DriverEvent, PinEvent};
use ssx_assets::anim_graph::{DriverTag, PinTag};
use ssx_assets::tags::TagEvent;

const MIN_REMAINING: f32 = 1.525_878_9e-5;

#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentDriver {
    pub tag: DriverTag,
    pub remaining: f32,
    pub source: Option<(Quat, Vec3)>,
}

impl AttachmentDriver {
    pub fn new(tag: DriverTag) -> Self {
        let remaining = tag.blend_in;
        Self { tag, remaining, source: None }
    }

    pub fn on_events(slot: &mut Option<AttachmentDriver>, events: &[DriverEvent]) {
        let mut kept = false;
        for e in events {
            match e.event {
                TagEvent::Enter => {
                    *slot = Some(Self::new(e.tag.clone()));
                    kept = true;
                }
                TagEvent::Apply(_) => kept |= slot.as_ref().is_some_and(|d| d.tag == e.tag),
                TagEvent::Leave(_) => {}
            }
        }
        if !kept {
            *slot = None;
        }
    }

    pub fn update(&mut self, root: (Quat, Vec3), target: (Quat, Vec3), dt: f32) -> Option<(Quat, Vec3)> {
        let (qs, ts) = self.source?;
        let rem = self.remaining;
        let (span, left) = if rem - dt >= 0.0 { (rem, rem - dt) } else { (dt, 0.0) };
        self.remaining = left;
        let f = dt / span.max(MIN_REMAINING);
        let (q, p) = root;
        let offset_q = Quat::from_array(self.tag.offset_q).normalize();
        let (qd, pd) = (target.0 * offset_q, target.1 + target.0 * Vec3::from_array(self.tag.offset_t));
        let x = (q.conjugate() * qd * qs.conjugate()).normalize();
        let id = Quat::IDENTITY;
        let c = if x.w > 0.0 { id + (x - id) * f } else { id - (x + id) * f };
        let c = c.normalize();
        let source_full = p + q * (x * ts);
        Some(((q * c).normalize(), p + (pd - source_full) * f))
    }
}

const PIN_ACTIVATION: f32 = 1.525_878_9e-5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinSignals {
    pub position: Vec3,
    pub orientation: Quat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PinTrajectoryDriver {
    pub tag: PinTag,
    pub timer: f32,
    pub active: bool,
    pub position: Option<Vec3>,
    pub orientation: Option<Quat>,
}

impl PinTrajectoryDriver {
    pub fn new(tag: PinTag) -> Self {
        let timer = tag.ticks_until_activation;
        Self { tag, timer, active: false, position: None, orientation: None }
    }

    pub fn on_events(slot: &mut Option<PinTrajectoryDriver>, events: &[PinEvent]) {
        let mut kept = false;
        for e in events {
            match e.event {
                TagEvent::Enter => {
                    *slot = Some(Self::new(e.tag.clone()));
                    kept = true;
                }
                TagEvent::Apply(_) => kept |= slot.as_ref().is_some_and(|d| d.tag == e.tag),
                TagEvent::Leave(_) => {}
            }
        }
        if !kept {
            *slot = None;
        }
    }

    pub fn update(&mut self, dt: f32, signals: PinSignals) -> Option<(Option<Vec3>, Option<Quat>)> {
        if !self.active {
            self.timer -= dt;
            if self.timer.partial_cmp(&PIN_ACTIVATION) == Some(std::cmp::Ordering::Greater) {
                return None;
            }
            self.active = true;
            self.position = (self.tag.position.as_deref() == Some("VEC_Position")).then_some(signals.position);
            self.orientation =
                (self.tag.orientation.as_deref() == Some("QUAT_Orientation")).then_some(signals.orientation);
        }
        Some((self.position, self.orientation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(blend_in: f32) -> DriverTag {
        DriverTag {
            window: ssx_assets::tags::TagWindow::new(0.0, 42.0, false),
            target: "ID_TargetAnimatable".into(),
            source_template: "Ground".into(),
            source_joint: Some("Ground".into()),
            target_template: "Heli_Floor".into(),
            target_joints: vec![("helicopter_rig".into(), "Floor".into())],
            offset_t: [0.0; 3],
            offset_q: [0.0, 0.0, 0.0, 1.0],
            blend_in,
        }
    }

    #[test]
    fn no_blend_in_places_the_source_on_the_target() {
        let mut d = AttachmentDriver::new(tag(0.0));
        d.source = Some((Quat::from_rotation_x(0.3), Vec3::new(0.1, -0.9, 0.2)));
        let root = (Quat::from_rotation_y(1.0), Vec3::new(5.0, 2.0, -3.0));
        let target = (Quat::from_rotation_z(0.4) * Quat::from_rotation_y(-0.7), Vec3::new(-1.0, 7.0, 4.0));
        let (q, p) = d.update(root, target, 2.0).unwrap();
        let (qs, ts) = d.source.unwrap();
        assert!((q * qs).angle_between(target.0) < 1e-4);
        assert!((p + q * ts).distance(target.1) < 1e-4);
    }

    #[test]
    fn the_blend_in_closes_the_gap_linearly() {
        let mut d = AttachmentDriver::new(tag(20.0));
        d.source = Some((Quat::IDENTITY, Vec3::new(0.0, -1.0, 0.0)));
        let target = (Quat::IDENTITY, Vec3::new(10.0, 0.0, 0.0));
        let mut root = (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0));
        for k in 1..=10 {
            root = d.update(root, target, 2.0).unwrap();
            let src = root.1 + Vec3::new(0.0, -1.0, 0.0);
            assert!((src.x - k as f32).abs() < 1e-4, "update {k}: {src}");
        }
        assert_eq!(d.remaining, 0.0);
        root = d.update(root, target, 2.0).unwrap();
        assert!((root.1 - Vec3::new(10.0, 1.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn the_tag_callbacks_drive_the_slot() {
        let t = tag(12.0);
        let ev = |event| DriverEvent { tag: t.clone(), event };
        let mut slot = None;
        AttachmentDriver::on_events(&mut slot, &[ev(TagEvent::Enter), ev(TagEvent::Apply(0.0))]);
        assert_eq!(slot.as_ref().map(|d| d.remaining), Some(12.0));
        slot.as_mut().unwrap().remaining = 4.0;
        AttachmentDriver::on_events(&mut slot, &[ev(TagEvent::Apply(0.5))]);
        assert_eq!(slot.as_ref().map(|d| d.remaining), Some(4.0));
        AttachmentDriver::on_events(&mut slot, &[ev(TagEvent::Leave(1.0))]);
        assert!(slot.is_none());
        AttachmentDriver::on_events(&mut slot, &[ev(TagEvent::Enter), ev(TagEvent::Apply(0.0))]);
        AttachmentDriver::on_events(&mut slot, &[]);
        assert!(slot.is_none());
    }

    fn pin_tag(ticks: f32) -> PinTag {
        PinTag {
            window: ssx_assets::tags::TagWindow::new(0.0, 0.0, true),
            position: Some("VEC_Position".into()),
            orientation: Some("QUAT_Orientation".into()),
            ticks_until_activation: ticks,
        }
    }

    #[test]
    fn the_pin_holds_the_values_of_its_activation() {
        let mut d = PinTrajectoryDriver::new(pin_tag(30.0));
        let at = |x: f32| PinSignals { position: Vec3::new(x, 1.0, 2.0), orientation: Quat::from_rotation_y(x) };
        for k in 1..15 {
            assert_eq!(d.update(2.0, at(k as f32)), None, "update {k}");
        }
        let held = (Some(at(15.0).position), Some(at(15.0).orientation));
        assert_eq!(d.update(2.0, at(15.0)), Some(held));
        assert!(d.active && d.timer == 0.0);
        assert_eq!(d.update(2.0, at(16.0)), Some(held));
        assert_eq!(d.timer, 0.0, "an active driver's timer stays");
    }

    #[test]
    fn the_pin_captures_only_the_signals_its_tag_names() {
        let mut t = pin_tag(0.0);
        t.orientation = None;
        let mut d = PinTrajectoryDriver::new(t);
        let s = PinSignals { position: Vec3::X, orientation: Quat::from_rotation_z(0.5) };
        assert_eq!(d.update(1.0, s), Some((Some(Vec3::X), None)));
        let mut t = pin_tag(0.0);
        t.position = Some("VEC_PrevPosition".into());
        let mut d = PinTrajectoryDriver::new(t);
        assert_eq!(d.update(1.0, s), Some((None, Some(s.orientation))));
    }

    #[test]
    fn the_pin_tag_callbacks_drive_the_slot() {
        let t = pin_tag(30.0);
        let ev = |event| PinEvent { tag: t.clone(), event };
        let mut slot = None;
        PinTrajectoryDriver::on_events(&mut slot, &[ev(TagEvent::Enter), ev(TagEvent::Apply(0.0))]);
        assert_eq!(slot.as_ref().map(|d| d.timer), Some(30.0));
        slot.as_mut().unwrap().timer = 4.0;
        PinTrajectoryDriver::on_events(&mut slot, &[ev(TagEvent::Apply(0.5))]);
        assert_eq!(slot.as_ref().map(|d| d.timer), Some(4.0));
        PinTrajectoryDriver::on_events(&mut slot, &[]);
        assert!(slot.is_none());
    }
}
