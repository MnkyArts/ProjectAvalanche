use super::{CSIS_EVENTS, SfxEngine, state};

pub const SET_HUD_PAUSE: (u32, &str) = (0x004D_6CAE, "Set_Hud_Pause");
pub const HUD_PAUSED: i32 = 2;
pub const HUD_RUNNING: i32 = 1;
pub const ENTER_PAUSE: usize = 96;
pub const EXIT_PAUSE: usize = 97;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseEdge {
    pub event: usize,
    pub hud_pause: i32,
}

#[derive(Clone, Debug, Default)]
pub struct HudPause {
    last_state: i32,
}

impl HudPause {
    pub fn state(&mut self, state: i32) -> Option<PauseEdge> {
        let edge = match (self.last_state == state::PAUSE, state == state::PAUSE) {
            (false, true) => Some(PauseEdge { event: ENTER_PAUSE, hud_pause: HUD_PAUSED }),
            (true, false) => Some(PauseEdge { event: EXIT_PAUSE, hud_pause: HUD_RUNNING }),
            _ => None,
        };
        self.last_state = state;
        edge
    }

    pub fn state_message(&mut self, sfx: &mut SfxEngine, state: i32) -> Option<String> {
        let edge = self.state(state)?;
        let (csis, name) = CSIS_EVENTS[edge.event];
        let fired = sfx.aems.resolve(csis, name).map(|key| sfx.fire(key, &[0])).is_some();
        let set = match sfx.aems.resolve(SET_HUD_PAUSE.0, SET_HUD_PAUSE.1) {
            Some(key) => {
                sfx.aems.set_global(key, edge.hud_pause);
                true
            }
            None => false,
        };
        Some(format!(
            "hud pause: {name} {}, {} = {}{}",
            if fired { "fired" } else { "not loaded" },
            SET_HUD_PAUSE.1,
            edge.hud_pause,
            if set { "" } else { " (not loaded)" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_the_pause_events_where_the_handler_reads_them() {
        assert_eq!(CSIS_EVENTS[ENTER_PAUSE], (0x0DB2_4E59, "Enter_Pause"));
        assert_eq!(CSIS_EVENTS[EXIT_PAUSE], (0x0DB2_24EE, "exit_pause"));
    }

    #[test]
    fn only_the_crossings_of_the_pause_menus_state_act() {
        let mut h = HudPause::default();
        let enter = Some(PauseEdge { event: ENTER_PAUSE, hud_pause: 2 });
        let leave = Some(PauseEdge { event: EXIT_PAUSE, hud_pause: 1 });
        let edges: Vec<_> = [state::RACE, state::PAUSE, state::PAUSE, state::RACE, state::RACE, state::PAUSE]
            .into_iter()
            .map(|s| h.state(s))
            .collect();
        assert_eq!(edges, [None, enter, None, leave, None, enter]);
        assert_eq!(h.state(state::MAIN_MENU), leave);
    }

    #[test]
    fn the_first_message_counts_from_the_images_zero() {
        assert_eq!(HudPause::default().state(state::PAUSE), Some(PauseEdge { event: ENTER_PAUSE, hud_pause: 2 }));
        assert_eq!(HudPause::default().state(state::BOOT), None);
    }
}
