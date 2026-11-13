use ssx_net_join::browse::State;
use ssx_net_proto::{GhostStyle, Rewinds};

use super::rows::{mode_name, selected};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd};
use crate::game::loc::Localization;

// PORT(choice): a server's drop shows the panel's drop fields only (title, peak, event type, difficulty, requirements,
// warning, time of day), not the player's medal, price or lock (multiplayer plays every drop and changes no career);
// the port's screen; the original has no server browser.
pub fn refresh(fe: &mut FrontEnd, env: &mut FeEnv) {
    let drop = selected(&fe.ava.browser).and_then(|e| e.summary()).filter(|s| s.map.is_none()).map(|s| s.drop);
    let same = match (&fe.ava.browser.panel, &drop) {
        (Some((d, _)), Some(n)) => d == n,
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    fe.ava.browser.panel = drop.map(|d| {
        let key = fe.career.event_named(&d).map(|e| e.key);
        let panel = key.and_then(|k| crate::game::fe::globe::panel::drop_fields(fe, env.loc, k));
        (d, panel)
    });
}

pub fn drop_panel(fe: &FrontEnd, cx: &mut Ctx) -> Value {
    match &fe.ava.browser.panel {
        Some((_, Some(p))) => crate::game::fe::globe::panel::object(cx, p),
        _ => Value::Null,
    }
}

fn rules(loc: &mut Localization, i: &ssx_net_proto::info::ServerInfo) -> String {
    let rewinds = match Rewinds::parse(&i.rewinds) {
        Some(Rewinds::On) => loc.text("TXT_AVA_BROWSER_REWINDS_ON"),
        Some(Rewinds::Off) => loc.text("TXT_AVA_BROWSER_REWINDS_OFF"),
        Some(Rewinds::Limit(n)) => loc.format("TXT_AVA_BROWSER_REWINDS_N", &[&n.to_string()]),
        None => i.rewinds.clone(),
    };
    let ghosts = loc.text(if i.ghosts { "TXT_AVA_BROWSER_GHOSTS_ON" } else { "TXT_AVA_BROWSER_GHOSTS_OFF" });
    let look = loc.text(match GhostStyle::parse(&i.ghost_style) {
        Some(GhostStyle::Hologram) => "TXT_AVA_BROWSER_LOOK_HOLOGRAM",
        _ => "TXT_AVA_BROWSER_LOOK_CHARACTER",
    });
    let password = loc.text(if i.password { "TXT_AVA_BROWSER_PASSWORD_YES" } else { "TXT_AVA_BROWSER_PASSWORD_NO" });
    let mode = match &i.map {
        Some(m) => super::rows::map_mode_name(loc, m),
        None => mode_name(loc, &i.mode),
    };
    loc.format("TXT_AVA_BROWSER_RULES", &[&mode, &rewinds, &ghosts, &look, &password])
}

fn resources(
    loc: &mut Localization,
    e: &ssx_net_join::browse::Entry,
    r: &ssx_net_proto::resources::ResourceSummary,
) -> String {
    let policy = loc.text(match r.client_mods.as_str() {
        "any" => "TXT_AVA_BROWSER_POLICY_ANY",
        "none" => "TXT_AVA_BROWSER_POLICY_NONE",
        _ => "TXT_AVA_BROWSER_POLICY_PRESENTATION",
    });
    let scripts = loc.text(if r.client_scripts { "TXT_AVA_BROWSER_SCRIPTS_ON" } else { "TXT_AVA_BROWSER_SCRIPTS_OFF" });
    if r.count == 0 {
        return loc.format("TXT_AVA_BROWSER_RESOURCES_NONE", &[&policy, &scripts]);
    }
    let download = super::rows::download_cell(loc, e, r);
    loc.format("TXT_AVA_BROWSER_RESOURCES", &[&r.count.to_string(), &download, &policy, &scripts])
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    let o = Value::Object(cx.new_object());
    let shown = |t: &str| crate::game::fe::ava::shown(t, fe.ava.glyphs.as_ref());
    let set = |cx: &mut Ctx, k: &str, v: &str| {
        let _ = cx.set_member(&o, k, Value::str(&shown(v)));
    };
    let set_lines = |cx: &mut Ctx, k: &str, lines: &[String]| {
        let _ = cx.set_member(&o, k, Value::str(&shown_lines(lines, fe.ava.glyphs.as_ref())));
    };
    let Some(e) = selected(&fe.ava.browser) else {
        let _ = cx.set_member(&o, "bShown", Value::Bool(false));
        return o;
    };
    let _ = cx.set_member(&o, "bShown", Value::Bool(true));
    set(cx, "name", &e.name());
    let mut address = e.address.to_string();
    let (region, tags) = match (&e.state, &e.listed) {
        (State::Info(i), _) => (i.region.as_str(), i.tags.as_slice()),
        (_, Some(l)) => (l.server.region.as_str(), l.server.tags.as_slice()),
        _ => ("", &[][..]),
    };
    if !region.is_empty() {
        address.push_str(&format!(" \u{2022} {region}"));
    }
    if !tags.is_empty() {
        address.push_str(&format!(" \u{2022} {}", tags.join(", ")));
    }
    set(cx, "address", &address);
    let icon = fe.ava.browser.icons.path(e, cx);
    let _ = cx.set_member(&o, "strIcon", Value::str(&icon));
    let (about, rules_text) = match &e.state {
        State::Info(i) => {
            let mut about = match &i.map {
                Some(m) => {
                    let head = loc.format("TXT_AVA_BROWSER_MAP", &[&m.title, &m.id]);
                    if i.description.is_empty() { vec![head] } else { vec![head, i.description.clone()] }
                }
                None if fe.career.event_named(&i.drop).is_none() => {
                    vec![loc.format("TXT_AVA_BROWSER_DROP_MISSING", &[&i.drop])]
                }
                None => vec![i.description.clone()],
            };
            if let Some(why) = super::resources_refusal(fe, loc, e) {
                about = vec![why];
            }
            (about, vec![rules(loc, i), resources(loc, e, &i.resources)])
        }
        State::Pending => (vec![loc.text("TXT_AVA_BROWSER_ASKING_LONG")], Vec::new()),
        State::Failed(why) => (vec![loc.format("TXT_AVA_BROWSER_FAILED", &[why])], Vec::new()),
        State::Incompatible { why, .. } => (vec![loc.format("TXT_AVA_BROWSER_CANNOT_JOIN", &[why])], Vec::new()),
    };
    set_lines(cx, "about", &about);
    set_lines(cx, "rules", &rules_text);
    let mut listed: &[String] = &[];
    let players = match (&e.players, e.summary()) {
        (Some(names), Some(i)) => {
            let head = loc.format("TXT_AVA_BROWSER_PLAYERS", &[&i.players.to_string(), &i.max_players.to_string()]);
            listed = names;
            if names.is_empty() { head } else { loc.format("TXT_AVA_BROWSER_PLAYERS_HEAD", &[&head]) }
        }
        (None, Some(_)) if !e.fake => match &e.players_why {
            Some(why) => loc.format("TXT_AVA_BROWSER_PLAYERS_FAILED", &[why]),
            None => loc.text("TXT_AVA_BROWSER_PLAYERS_ASKING"),
        },
        _ => String::new(),
    };
    set(cx, "players", &players);
    let names = listed.iter().map(|n| Value::str(&shown(n))).collect();
    let names = Value::Object(cx.new_array(names));
    let _ = cx.set_member(&o, "playerNames", names);
    set(cx, "playersMore", &loc.text("TXT_AVA_BROWSER_MORE_PLAYERS"));
    o
}

fn shown_lines(lines: &[String], glyphs: Option<&std::collections::BTreeSet<char>>) -> String {
    lines.iter().map(|l| crate::game::fe::ava::shown(l, glyphs)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::shown_lines;
    use crate::game::fe::ava::shown;

    #[test]
    fn the_panes_lines_keep_their_breaks_and_a_servers_text_none() {
        let lines = ["Map: Neon Gully (neon:map/gully)".to_string(), "two\nlines".to_string()];
        let out = shown_lines(&lines, None);
        assert_eq!(out, format!("{}\n{}", shown(&lines[0], None), shown(&lines[1], None)));
        assert_eq!(out.matches('\n').count(), 1, "only the pane's own break: {out:?}");
        assert!(shown(&lines[1], None).contains('?'));
        assert_eq!(shown_lines(&[], None), "");
    }
}
