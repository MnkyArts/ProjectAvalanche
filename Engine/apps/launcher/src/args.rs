use std::path::PathBuf;

use eframe::egui::Key;

use crate::ui::{self, Page};

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
    pub nav_dump: Option<PathBuf>,
    pub pad: Vec<crate::pads::PadButton>,
    pub clicks: Vec<ui::titlebar::Click>,
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
                    out.page = Some(Page::parse(v).ok_or(format!("--page {v}: one of {}", Page::NAMES.join(", ")))?);
                }
                "--tab" => out.tab = Some(value()?.clone()),
                "--window" => {
                    let v = value()?;
                    out.window = Some(window_size(v).ok_or(format!("--window {v}: a size like 1280x720"))?);
                }
                "--hover" => out.hover = Some(value()?.clone()),
                "--scroll" => out.scroll = value()?.parse().map_err(|_| "--scroll N".to_string())?,
                "--dump-art" => out.dump_art = Some(PathBuf::from(value()?)),
                "--nav-dump" => out.nav_dump = Some(PathBuf::from(value()?)),
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
                    out.page.get_or_insert(Page::Online);
                }
                "--click" => {
                    let names = value()?;
                    let all = ui::titlebar::Click::NAMES.map(|(n, _)| n).join(", ");
                    out.clicks = names
                        .split(',')
                        .map(|n| ui::titlebar::Click::parse(n).ok_or(format!("--click {names}: a list of {all}")))
                        .collect::<Result<_, _>>()?;
                }
                "--pad" => {
                    let names = value()?;
                    let all = crate::pads::PadButton::NAMES.map(|(n, _)| n).join(", ");
                    out.pad = names
                        .split(',')
                        .map(|n| crate::pads::PadButton::parse(n).ok_or(format!("--pad {names}: a list of {all}")))
                        .collect::<Result<_, _>>()?;
                }
                "--keys" => {
                    let names = value()?;
                    let key = |name: &str| match name {
                        "up" => Ok(Key::ArrowUp),
                        "down" => Ok(Key::ArrowDown),
                        "left" => Ok(Key::ArrowLeft),
                        "right" => Ok(Key::ArrowRight),
                        "q" | "lb" => Ok(Key::Q),
                        "e" | "rb" => Ok(Key::E),
                        "end" => Ok(Key::End),
                        "pgdown" => Ok(Key::PageDown),
                        "pgup" => Ok(Key::PageUp),
                        "enter" => Ok(Key::Enter),
                        "esc" => Ok(Key::Escape),
                        "tab" => Ok(Key::Tab),
                        "space" => Ok(Key::Space),
                        _ => Err(format!(
                            "--keys {names}: a list of up, down, left, right, enter, esc, space, q (lb), e (rb), tab, \
                             backtab, end, pgdown, pgup"
                        )),
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
            let names = ui::tab_names(page);
            let known = names.iter().any(|n| n.eq_ignore_ascii_case(tab))
                || (page == Page::Content && tab.eq_ignore_ascii_case(ui::TAB_MOUNT));
            if !known {
                let more = if page == Page::Content { format!(", {}", ui::TAB_MOUNT) } else { String::new() };
                return Err(match names.is_empty() {
                    true => format!("--tab {tab}: the {} page has no sections", Page::NAMES[page as usize]),
                    false => format!("--tab {tab}: {} has {}{more}", Page::NAMES[page as usize], names.join(", ")),
                });
            }
        }
        Ok(out)
    }

    pub fn start_page(&self) -> Option<Page> {
        self.page.or((self.setup || self.extract).then_some(Page::Content))
    }

    pub fn start_tab(&self) -> Option<&str> {
        let setup = self.page.is_none() && (self.setup || self.extract);
        self.tab.as_deref().or(setup.then_some("data"))
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
        let setup = parse("--setup").unwrap();
        assert_eq!((setup.start_page(), setup.start_tab()), (Some(Page::Content), Some("data")));
        let mods = parse("--setup --page content").unwrap();
        assert_eq!((mods.start_page(), mods.start_tab()), (Some(Page::Content), None));
        assert_eq!(parse("--extract --page settings --tab audio").unwrap().start_tab(), Some("audio"));
        assert_eq!(parse("").unwrap().start_page(), None);
        assert_eq!(parse("--host start").unwrap().start_page(), Some(Page::Online));
        assert_eq!(parse("--page content --tab mount").unwrap().tab.as_deref(), Some("mount"));
        assert_eq!(parse("--page online --tab profile").unwrap().tab.as_deref(), Some("profile"));
        assert_eq!(
            parse("--keys down,backtab,esc,left,right,lb,e").unwrap().keys,
            [
                (Key::ArrowDown, false),
                (Key::Tab, true),
                (Key::Escape, false),
                (Key::ArrowLeft, false),
                (Key::ArrowRight, false),
                (Key::Q, false),
                (Key::E, false)
            ]
        );
        assert!(parse("--keys down,sideways").is_err());
        use crate::pads::PadButton;
        assert_eq!(parse("--pad a,down,rb").unwrap().pad, [PadButton::A, PadButton::Down, PadButton::Rb]);
        assert!(parse("--pad a,start").is_err());
        use crate::ui::titlebar::Click;
        assert_eq!(
            parse("--click maximize,move,resize").unwrap().clicks,
            [Click::Maximize, Click::Move, Click::Resize]
        );
        assert!(parse("--click maximize,drag").is_err());
        for bad in [
            "--page nowhere",
            "--page",
            "--window 12",
            "--window 0x0",
            "--window axb",
            "--page settings --tab about",
            "--page mods",
            "--page host",
            "--page settings --tab data",
            "--page extras --tab output",
            "--page play --tab mods",
            "--page settings --tab mount",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
