use serde_json::{Map, Value as Json};

// PORT(choice): 64 MiB … 8 GiB, 2 GiB by default (`ssx_net_join::resources::DEFAULT_LIMIT`: one package at its cap; a
// map within M8b's budgets is at most 512 MiB of models and 1 GiB of images); tuning: what players set; the original
// downloads no packages.
pub const DOWNLOAD_STEPS_MIB: [u32; 8] = [64, 128, 256, 512, 1024, 2048, 4096, 8192];
pub const DEFAULT_DOWNLOAD_MIB: u32 = 2048;
// PORT(choice): 1 … 64 GiB, 8 GiB by default (four servers' worth of the 2 GiB a package may be); tuning: the cache
// sizes players keep; the original downloads no packages.
pub const CACHE_STEPS_MIB: [u32; 7] = [1024, 2048, 4096, 8192, 16384, 32768, 65536];
pub const DEFAULT_CACHE_MIB: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceSettings {
    pub download_limit_mib: u32,
    pub cache_mib: u32,
    pub server_scripts: bool,
}

impl Default for ResourceSettings {
    fn default() -> Self {
        ResourceSettings {
            download_limit_mib: DEFAULT_DOWNLOAD_MIB,
            cache_mib: DEFAULT_CACHE_MIB,
            server_scripts: true,
        }
    }
}

impl ResourceSettings {
    pub fn from_json(v: Option<&Json>, problems: &mut Vec<String>) -> ResourceSettings {
        let mut s = ResourceSettings::default();
        let Some(v) = v else { return s };
        let Some(m) = v.as_object() else {
            problems.push("settings.json: `resources` is not an object".into());
            return s;
        };
        let mut bad =
            |k: &str| problems.push(format!("settings.json: `resources.{k}` has a value the game cannot read"));
        let step =
            |x: &Json, steps: &[u32]| x.as_u64().and_then(|n| steps.iter().copied().find(|&s| u64::from(s) == n));
        if let Some(x) = m.get("download_limit_mib") {
            match step(x, &DOWNLOAD_STEPS_MIB) {
                Some(n) => s.download_limit_mib = n,
                None => bad("download_limit_mib"),
            }
        }
        if let Some(x) = m.get("cache_mib") {
            match step(x, &CACHE_STEPS_MIB) {
                Some(n) => s.cache_mib = n,
                None => bad("cache_mib"),
            }
        }
        if let Some(x) = m.get("server_scripts") {
            match x.as_bool() {
                Some(b) => s.server_scripts = b,
                None => bad("server_scripts"),
            }
        }
        s
    }

    pub fn to_json(self) -> Json {
        let mut m = Map::new();
        m.insert("download_limit_mib".into(), Json::from(self.download_limit_mib));
        m.insert("cache_mib".into(), Json::from(self.cache_mib));
        m.insert("server_scripts".into(), Json::Bool(self.server_scripts));
        Json::Object(m)
    }

    pub fn download_limit(self) -> u64 {
        u64::from(self.download_limit_mib) << 20
    }

    pub fn cache_bytes(self) -> u64 {
        u64::from(self.cache_mib) << 20
    }
}

pub fn size_label(mib: u32, loc: &mut crate::game::loc::Localization) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        loc.format("TXT_AVA_OPT_SIZE_GB", &[&(mib / 1024).to_string()])
    } else {
        loc.format("TXT_AVA_OPT_SIZE_MB", &[&mib.to_string()])
    }
}

pub fn step(v: &mut u32, steps: &[u32], next: bool) -> bool {
    let i = steps.iter().position(|s| s == v).unwrap_or(0);
    let to = if next { (i + 1).min(steps.len() - 1) } else { i.saturating_sub(1) };
    let moved = steps[to] != *v;
    *v = steps[to];
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_strictly_and_written_back() {
        let mut p = Vec::new();
        let s = ResourceSettings::from_json(
            Some(&serde_json::json!({"download_limit_mib": 512, "cache_mib": 16384, "server_scripts": false})),
            &mut p,
        );
        assert!(p.is_empty());
        assert_eq!(s, ResourceSettings { download_limit_mib: 512, cache_mib: 16384, server_scripts: false });
        assert_eq!(ResourceSettings::from_json(Some(&s.to_json()), &mut p), s);
        for bad in [
            serde_json::json!({"download_limit_mib": 500}),
            serde_json::json!({"cache_mib": -1}),
            serde_json::json!({"server_scripts": "no"}),
            serde_json::json!([1]),
        ] {
            let mut p = Vec::new();
            assert_eq!(ResourceSettings::from_json(Some(&bad), &mut p), ResourceSettings::default(), "{bad}");
            assert_eq!(p.len(), 1, "{bad}");
        }
        let mut v = 64;
        assert!(!step(&mut v, &DOWNLOAD_STEPS_MIB, false) && v == 64);
        assert!(step(&mut v, &DOWNLOAD_STEPS_MIB, true) && v == 128);
        let mut v = 8192;
        assert!(!step(&mut v, &DOWNLOAD_STEPS_MIB, true) && v == 8192);
    }
}
