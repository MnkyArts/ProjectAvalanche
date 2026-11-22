mod about;
mod display;
mod game_data;
mod host;
mod mods;
mod mods_report;
mod options;
mod output;
mod play;
mod profile;
mod tools;
mod updates;

use eframe::egui::Ui;

use crate::app::Launcher;
use crate::ui::details::{self, Detail};
use crate::ui::layout::Layout;
use crate::ui::{ContentTab, ExtrasTab, OnlineTab, Page, Section, SettingsTab, panel};

fn applies(page: Page) -> &'static str {
    match page {
        Page::Online => HOST_APPLIES,
        Page::Extras => details::APPLIES_TOOLS,
        _ => details::APPLIES_GAME,
    }
}

pub const HOST_APPLIES: &str = "Saved in the server's settings; applies when the server starts.";

impl Launcher {
    pub fn panel_page(&mut self, ui: &mut Ui, l: &Layout) {
        let page = self.page;
        panel::frame(ui.painter(), l, page.title());
        let device = self.device;
        match page {
            Page::Play => {}
            Page::Online => {
                panel::tabs(ui, l, &mut self.online_tab, device);
            }
            Page::Content => {
                panel::tabs(ui, l, &mut self.content_tab, device);
            }
            Page::Settings => {
                panel::tabs(ui, l, &mut self.settings_tab, device);
            }
            Page::Extras => {
                panel::tabs(ui, l, &mut self.extras_tab, device);
            }
        }
        let (label, wide) = match page {
            Page::Play => return,
            Page::Online => (self.online_tab.label(), self.online_tab.wide()),
            Page::Content => (self.content_tab.label(), self.content_tab.wide()),
            Page::Settings => (self.settings_tab.label(), self.settings_tab.wide()),
            Page::Extras => (self.extras_tab.label(), self.extras_tab.wide()),
        };
        if page == Page::Online {
            self.poll_host(ui.ctx());
            self.host_script(ui.ctx());
        }
        details::begin(ui.ctx());
        let rect = if wide { l.body_wide } else { l.body };
        let id = format!("{}-{label}", page.title());
        panel::body(ui, rect, &id, |ui| {
            if self.shows_settings_notes() {
                self.settings_notes(ui);
            }
            self.section(ui);
        });
        let offered = details::take(ui.ctx());
        if !wide {
            let detail = offered.unwrap_or_else(|| self.idle_detail());
            details::draw(ui, l.details, l.ppp, &detail);
        }
    }

    fn shows_settings_notes(&self) -> bool {
        match self.page {
            Page::Play => false,
            Page::Online | Page::Settings => true,
            Page::Content => self.content_tab == ContentTab::GameData,
            Page::Extras => self.extras_tab == ExtrasTab::Tools,
        }
    }

    fn section(&mut self, ui: &mut Ui) {
        match self.page {
            Page::Play => {}
            Page::Online => match self.online_tab {
                OnlineTab::Host => self.host_section(ui),
                OnlineTab::Profile => self.profile_section(ui),
            },
            Page::Content => match self.content_tab {
                ContentTab::Mods => self.mods_section(ui),
                ContentTab::Dlc => self.dlc_section(ui),
                ContentTab::TitleUpdate => self.tu_section(ui),
                ContentTab::GameData => self.game_data_section(ui),
            },
            Page::Settings => match self.settings_tab {
                SettingsTab::Display => self.display_section(ui),
                SettingsTab::Graphics => self.graphics_section(ui),
                SettingsTab::Audio => self.audio_section(ui),
                SettingsTab::Controls => self.controls_section(ui),
                SettingsTab::Game => self.game_section(ui),
            },
            Page::Extras => match self.extras_tab {
                ExtrasTab::Tools => self.tools_section(ui),
                ExtrasTab::Log => self.output_section(ui),
                ExtrasTab::Files => self.files_section(ui),
                ExtrasTab::Updates => self.update_section(ui),
                ExtrasTab::About => self.about_section(ui),
            },
        }
    }

    fn idle_detail(&self) -> Detail {
        let page = self.page;
        let (label, intro) = match page {
            Page::Play => return Detail::default(),
            Page::Online => match self.online_tab {
                OnlineTab::Host => ("HOST", host::INTRO),
                OnlineTab::Profile => ("PROFILE & LINKS", details::PICK),
            },
            Page::Content => match self.content_tab {
                ContentTab::Mods => ("MODS", mods::INTRO),
                ContentTab::Dlc => ("DLC", game_data::DLC_INTRO),
                ContentTab::TitleUpdate => ("TITLE UPDATE", game_data::TU_INTRO),
                ContentTab::GameData => ("GAME DATA", game_data::INTRO),
            },
            Page::Settings => (self.settings_tab.label(), details::PICK),
            Page::Extras => match self.extras_tab {
                ExtrasTab::Updates => ("UPDATES", updates::INTRO),
                tab => (tab.label(), details::PICK),
            },
        };
        let detail = details::idle(label, intro);
        match (page, intro == details::PICK) {
            (Page::Content, _) => detail,
            (Page::Extras, false) => detail,
            _ => detail.applies(applies(page)),
        }
    }
}
