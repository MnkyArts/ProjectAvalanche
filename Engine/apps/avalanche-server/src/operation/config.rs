use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;
use std::path::PathBuf;

use serde::Deserialize;
use ssx_net_ops::Range as AddressRange;
use toml::Spanned;

use crate::config::ConfigError;

// PORT(choice): 32 ranges (a scraper or two, a monitoring network, both address families); tuning: the deployments
// operators run; the original has no dedicated server.
pub const MAX_METRICS_ALLOW: usize = 32;

// PORT(choice): 60 s — long enough for a run in its last minute to finish (SSX's race drops run about one to four
// minutes), short enough for systemd's stop timeout (90 s in our unit) and Docker's grace (90 s in our compose file)
// with the results' display and the goodbyes after it; tuning: the events' run times (the rival ghosts' durations) and
// the supervisors operators use; the original has no dedicated server.
pub const DEFAULT_ROUND_END_SECS: u32 = 60;
pub const ROUND_END_SECS: std::ops::RangeInclusive<i64> = 0..=600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationConfig {
    pub metrics_enabled: bool,
    pub metrics_allow: Vec<AddressRange>,
    pub round_end_secs: u32,
    pub icon: Option<PathBuf>,
}

impl Default for OperationConfig {
    fn default() -> Self {
        Self {
            metrics_enabled: false,
            metrics_allow: vec![
                AddressRange::address(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                AddressRange::address(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            ],
            round_end_secs: DEFAULT_ROUND_END_SECS,
            icon: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMetrics {
    enabled: Option<Spanned<bool>>,
    allow: Option<Spanned<Vec<Spanned<String>>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawStop {
    round_end: Option<Spanned<i64>>,
}

type At<'a> = &'a dyn Fn(Range<usize>, String) -> ConfigError;

pub(crate) fn parse_tables(
    metrics: Option<RawMetrics>,
    stop: Option<RawStop>,
    out: &mut OperationConfig,
    at: At<'_>,
) -> Result<(), ConfigError> {
    if let Some(m) = metrics {
        out.metrics_enabled = m.enabled.is_some_and(Spanned::into_inner);
        if let Some(list) = m.allow {
            if list.get_ref().len() > MAX_METRICS_ALLOW {
                return Err(at(list.span(), format!("allow: at most {MAX_METRICS_ALLOW} addresses or ranges")));
            }
            out.metrics_allow.clear();
            for r in list.into_inner() {
                let range = AddressRange::parse(r.get_ref()).map_err(|e| at(r.span(), format!("allow: {e}")))?;
                if out.metrics_allow.contains(&range) {
                    return Err(at(r.span(), format!("allow: {range} twice")));
                }
                out.metrics_allow.push(range);
            }
        }
    }
    if let Some(round_end) = stop.and_then(|s| s.round_end) {
        let v = *round_end.get_ref();
        out.round_end_secs = u32::try_from(v).ok().filter(|_| ROUND_END_SECS.contains(&v)).ok_or_else(|| {
            at(
                round_end.span(),
                format!("round_end = {v}: {} to {} seconds", ROUND_END_SECS.start(), ROUND_END_SECS.end()),
            )
        })?;
    }
    Ok(())
}

pub fn icon_file(config: &crate::config::Config) -> Option<PathBuf> {
    let folder = config.path.as_deref().and_then(std::path::Path::parent).unwrap_or(std::path::Path::new(""));
    config.operation.icon.as_ref().map(|icon| folder.join(icon))
}

pub(crate) fn parse_icon(icon: Spanned<String>, at: At<'_>) -> Result<PathBuf, ConfigError> {
    let text = icon.get_ref();
    let path = PathBuf::from(text);
    let plain = path.components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if text.is_empty() || text.len() > 255 || !plain || text.contains('\\') || text.chars().any(char::is_control) {
        return Err(at(
            icon.span(),
            format!(
                "icon = {}: a file beside server.toml or below its folder (no absolute path, no ..)",
                ssx_core::text::quoted_max(text, 64)
            ),
        ));
    }
    if !text.to_ascii_lowercase().ends_with(".png") {
        return Err(at(icon.span(), "icon: a .png file (96 × 96)".into()));
    }
    Ok(path)
}
