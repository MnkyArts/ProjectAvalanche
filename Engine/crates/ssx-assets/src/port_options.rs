use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RangeMovies {
    #[default]
    EveryTime,
    FirstTimeOnly,
}

impl RangeMovies {
    pub const ALL: [RangeMovies; 2] = [RangeMovies::EveryTime, RangeMovies::FirstTimeOnly];
    pub const MEMBER: &str = "range_movies";
    pub const FLAG: &str = "--range-movies";

    pub fn name(self) -> &'static str {
        match self {
            RangeMovies::EveryTime => "EveryTime",
            RangeMovies::FirstTimeOnly => "FirstTimeOnly",
        }
    }

    pub fn from_name(name: &str) -> Option<RangeMovies> {
        RangeMovies::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            RangeMovies::EveryTime => "every-time",
            RangeMovies::FirstTimeOnly => "first-time-only",
        }
    }
}

impl fmt::Display for RangeMovies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for RangeMovies {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RangeMovies::ALL.into_iter().find(|r| r.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = RangeMovies::ALL.iter().map(|r| r.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BootMovie {
    #[default]
    EveryStart,
    Skip,
}

impl BootMovie {
    pub const ALL: [BootMovie; 2] = [BootMovie::EveryStart, BootMovie::Skip];
    pub const MEMBER: &str = "boot_movie";
    pub const FLAG: &str = "--boot-movie";

    pub fn name(self) -> &'static str {
        match self {
            BootMovie::EveryStart => "EveryStart",
            BootMovie::Skip => "Skip",
        }
    }

    pub fn from_name(name: &str) -> Option<BootMovie> {
        BootMovie::ALL.into_iter().find(|b| b.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            BootMovie::EveryStart => "every-start",
            BootMovie::Skip => "skip",
        }
    }

    pub fn skip_intro(self) -> bool {
        self == BootMovie::Skip
    }
}

impl fmt::Display for BootMovie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for BootMovie {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BootMovie::ALL.into_iter().find(|b| b.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = BootMovie::ALL.iter().map(|b| b.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MenuFocus {
    #[default]
    FirstItem,
    LastItem,
}

impl MenuFocus {
    pub const ALL: [MenuFocus; 2] = [MenuFocus::FirstItem, MenuFocus::LastItem];
    pub const MEMBER: &str = "menu_focus";
    pub const FLAG: &str = "--menu-focus";

    pub fn name(self) -> &'static str {
        match self {
            MenuFocus::FirstItem => "FirstItem",
            MenuFocus::LastItem => "LastItem",
        }
    }

    pub fn from_name(name: &str) -> Option<MenuFocus> {
        MenuFocus::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            MenuFocus::FirstItem => "first-item",
            MenuFocus::LastItem => "last-item",
        }
    }
}

impl fmt::Display for MenuFocus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for MenuFocus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MenuFocus::ALL.into_iter().find(|r| r.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = MenuFocus::ALL.iter().map(|r| r.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_read_back_by_name_and_by_flag_value() {
        assert_eq!(RangeMovies::default(), RangeMovies::EveryTime);
        for r in RangeMovies::ALL {
            assert_eq!(RangeMovies::from_name(r.name()), Some(r));
            assert_eq!(r.arg().parse::<RangeMovies>(), Ok(r));
            assert_eq!(r.to_string(), r.arg());
        }
        for bad in ["", "everytime", "firsttimeonly", "FIRST-TIME-ONLY", " every-time", "EveryTime", "first"] {
            assert!(bad.parse::<RangeMovies>().is_err(), "{bad:?}");
        }
        for bad in ["", "everyTime", "first-time-only", "FirstTimeOnly ", "Once"] {
            assert_eq!(RangeMovies::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<RangeMovies>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("every-time, first-time-only"), "{e}");
    }

    #[test]
    fn the_boot_movie_choices_read_back_by_name_and_by_flag_value() {
        assert_eq!(BootMovie::default(), BootMovie::EveryStart);
        assert!(!BootMovie::EveryStart.skip_intro() && BootMovie::Skip.skip_intro());
        for b in BootMovie::ALL {
            assert_eq!(BootMovie::from_name(b.name()), Some(b));
            assert_eq!(b.arg().parse::<BootMovie>(), Ok(b));
            assert_eq!(b.to_string(), b.arg());
        }
        for bad in ["", "Skip", "skip ", "SKIP", "every_start", "EveryStart", "off", "true"] {
            assert!(bad.parse::<BootMovie>().is_err(), "{bad:?}");
        }
        for bad in ["", "skip", "every-start", "Skip ", "Off", "true"] {
            assert_eq!(BootMovie::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<BootMovie>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("every-start, skip"), "{e}");
    }

    #[test]
    fn the_menu_focus_reads_back_by_name_and_by_flag_value() {
        assert_eq!(MenuFocus::default(), MenuFocus::FirstItem);
        for m in MenuFocus::ALL {
            assert_eq!(MenuFocus::from_name(m.name()), Some(m));
            assert_eq!(m.arg().parse::<MenuFocus>(), Ok(m));
            assert_eq!(m.to_string(), m.arg());
        }
        for bad in ["", "firstitem", "LAST-ITEM", " last-item", "LastItem", "last", "remember"] {
            assert!(bad.parse::<MenuFocus>().is_err(), "{bad:?}");
        }
        for bad in ["", "firstItem", "last-item", "LastItem ", "Remember"] {
            assert_eq!(MenuFocus::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<MenuFocus>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("first-item, last-item"), "{e}");
    }
}
