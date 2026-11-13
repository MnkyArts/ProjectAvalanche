use std::path::PathBuf;

use eframe::egui::Key;

use crate::ui::{ExtrasTab, ModsTab, Page, SettingsTab};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    pub screenshot: Option<PathBuf>,
    pub setup: bool,
    pub extract: bool,
    pub launch: bool,
    pub play: bool,
    pub print: bool,
    pub no_update_check: bool,
    pub page: Option<Page>,
    pub tab: Option<String>,
    pub window: Option<[f32; 2]>,
    pub scroll: f32,
    pub hover: Option<String>,
    pub dump_art: Option<PathBuf>,
    pub after_setup: bool,
    pub install: Option<PathBuf>,
    pub dlc_install: Option<PathBuf>,
    pub tuner_install: Option<PathBuf>,
    pub keys: Vec<(Key, bool)>,
    pub link: Option<String>,
    pub host: Vec<crate::host::Action>,
}

fn window_size(text: &str) -> Option<[f32; 2]> {
    let (w, h) = text.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    ((64..=16384).contains(&w) && (64..=16384).contains(&h)).then_some([w as f32, h as f32])
}

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut out = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--screenshot" => out.screenshot = Some(PathBuf::from(value()?)),
                "--setup" => out.setup = true,
                "--extract" => out.extract = true,
                "--launch" => out.launch = true,
                "--play" => out.play = true,
                "--print" => out.print = true,
                "--no-update-check" => out.no_update_check = true,
                "--page" => {
                    let v = value()?;
                    out.page =
                        Some(Page::parse(v).ok_or(format!("--page {v}: one of play, host, mods, settings, extras"))?);
                }
                "--tab" => out.tab = Some(value()?.clone()),
                "--window" => {
                    let v = value()?;
                    out.window = Some(window_size(v).ok_or(format!("--window {v}: a size like 1280x720"))?);
                }
                "--hover" => out.hover = Some(value()?.clone()),
                "--scroll" => out.scroll = value()?.parse().map_err(|_| "--scroll N".to_string())?,
                "--dump-art" => out.dump_art = Some(PathBuf::from(value()?)),
                "--install" => out.install = Some(PathBuf::from(value()?)),
                "--dlc-install" => out.dlc_install = Some(PathBuf::from(value()?)),
                "--tuner-install" => out.tuner_install = Some(PathBuf::from(value()?)),
                "--after-setup" => out.after_setup = true,
                "--host" => {
                    let names = value()?;
                    out.host = names
                        .split(',')
                        .map(|n| {
                            crate::host::Action::parse(n)
                                .ok_or(format!("--host {names}: a list of prepare, start, status, stop"))
                        })
                        .collect::<Result<_, _>>()?;
                    out.page.get_or_insert(Page::Host);
                }
                "--keys" => {
                    let names = value()?;
                    let key = |name: &str| match name {
                        "up" => Ok(Key::ArrowUp),
                        "down" => Ok(Key::ArrowDown),
                        "end" => Ok(Key::End),
                        "pgdown" => Ok(Key::PageDown),
                        "pgup" => Ok(Key::PageUp),
                        "enter" => Ok(Key::Enter),
                        "esc" => Ok(Key::Escape),
                        "tab" => Ok(Key::Tab),
                        "space" => Ok(Key::Space),
                        _ => Err(format!("--keys {names}: a list of up, down, enter, esc, tab, backtab, space")),
                    };
                    let one = |name: &str| match name {
                        "backtab" => Ok((Key::Tab, true)),
                        _ => key(name).map(|k| (k, false)),
                    };
                    out.keys = names.split(',').map(one).collect::<Result<_, _>>()?;
                }
                _ => {}
            }
        }
        if let (Some(tab), Some(page)) = (&out.tab, out.start_page()) {
            let known = match page {
                Page::Settings => SettingsTab::parse(tab).is_some(),
                Page::Extras => ExtrasTab::parse(tab).is_some(),
                Page::Mods => ModsTab::parse(tab).is_some(),
                Page::Play | Page::Host => false,
            };
            if !known {
                return Err(format!(
                    "--tab {tab}: settings has data, graphics, audio, controls, multiplayer; extras has tools, output, about, files, \
                     updates; mods has installed, report"
                ));
            }
        }
        Ok(out)
    }

    pub fn start_page(&self) -> Option<Page> {
        self.page.or((self.setup || self.extract).then_some(Page::Settings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, String> {
        Args::parse(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn the_old_flags() {
        let a = parse("--screenshot out.png --setup --extract --launch --play --print").unwrap();
        assert_eq!(a.screenshot, Some(PathBuf::from("out.png")));
        assert!(a.setup && a.extract && a.launch && a.play && a.print);
        assert!(!a.no_update_check);
        assert!(parse("--no-update-check").unwrap().no_update_check);
        assert_eq!(parse("").unwrap(), Args::default());
        assert_eq!(parse("--unknown x").unwrap(), Args::default());
        assert_eq!(parse("--scroll 400").unwrap().scroll, 400.0);
        assert_eq!(parse("--dlc-install dump.zip").unwrap().dlc_install, Some(PathBuf::from("dump.zip")));
        assert_eq!(parse("--tuner-install ssxvprofile").unwrap().tuner_install, Some(PathBuf::from("ssxvprofile")));
    }

    #[test]
    fn page_window_and_tab() {
        let a = parse("--page extras --tab about --window 1920x1080 --hover hero").unwrap();
        assert_eq!((a.page, a.window), (Some(Page::Extras), Some([1920.0, 1080.0])));
        assert_eq!((a.tab.as_deref(), a.hover.as_deref()), (Some("about"), Some("hero")));
        assert_eq!(parse("--setup").unwrap().start_page(), Some(Page::Settings));
        assert_eq!(parse("--setup --page mods").unwrap().start_page(), Some(Page::Mods));
        assert_eq!(parse("").unwrap().start_page(), None);
        assert_eq!(
            parse("--keys down,backtab,esc").unwrap().keys,
            [(Key::ArrowDown, false), (Key::Tab, true), (Key::Escape, false)]
        );
        assert!(parse("--keys down,left").is_err());
        for bad in
            ["--page nowhere", "--page", "--window 12", "--window 0x0", "--window axb", "--page settings --tab about"]
        {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
