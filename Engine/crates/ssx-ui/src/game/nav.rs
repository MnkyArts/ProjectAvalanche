use std::collections::{HashMap, VecDeque};

use anyhow::{Context, Result, bail};
use serde_json::Value as Json;

use crate::fs::FileSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub name: String,
    pub args: Vec<String>,
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.args.join(", "))
    }
}

#[derive(Clone, Debug)]
struct Transition {
    event: Option<String>,
    targets: Vec<usize>,
    actions: Vec<Action>,
}

type Selected = (Option<usize>, Vec<usize>, Vec<Action>);

#[derive(Clone, Debug)]
struct State {
    name: String,
    parallel: bool,
    parent: Option<usize>,
    children: Vec<usize>,
    initial: Option<(Vec<usize>, Vec<Action>)>,
    on_enter: Vec<Action>,
    on_exit: Vec<Action>,
    transitions: Vec<Transition>,
}

#[derive(Clone, Debug)]
enum Step {
    NextInternal,
    Exit(usize),
    Enter(usize, bool),
    Action(Action),
    SortConfig,
    EventDone,
}

#[derive(Clone, Debug, Default)]
pub struct Nav {
    states: Vec<State>,
    by_name: HashMap<String, usize>,
    config: Vec<usize>,
    external: VecDeque<String>,
    internal: VecDeque<String>,
    started: bool,
    busy: bool,
    todo: VecDeque<Step>,
    awaiting: Option<Action>,
    pub log: Vec<String>,
    pub drop_key: u32,
}

fn actions(v: Option<&Json>) -> Vec<Action> {
    let Some(a) = v.and_then(Json::as_array) else { return Vec::new() };
    a.iter()
        .filter_map(|x| {
            let x = x.as_array()?;
            let name = x.first()?.as_str()?.to_string();
            let args = x
                .get(1)
                .and_then(Json::as_array)
                .map(|a| a.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect())
                .unwrap_or_default();
            Some(Action { name, args })
        })
        .collect()
}

impl Nav {
    pub const PATH: &str = "data/common/fe/ssxgame.nav";

    pub fn load(files: &dyn FileSource) -> Result<Self> {
        let data = files.read(Self::PATH).with_context(|| format!("{} not found", Self::PATH))?;
        let j: Json = serde_json::from_slice(&data).context("parsing ssxgame.nav")?;
        Self::from_json(&j)
    }

    pub fn from_json(root: &Json) -> Result<Self> {
        let mut nav = Nav::default();
        let mut raw = Vec::new();
        nav.build(root, None, &mut raw)?;
        let resolve = |names: Option<&Json>, by_name: &HashMap<String, usize>| -> Result<Vec<usize>> {
            let mut out = Vec::new();
            for t in names.and_then(Json::as_array).into_iter().flatten() {
                let n = t.as_str().unwrap_or_default();
                out.push(*by_name.get(n).with_context(|| format!("nav: no state {n}"))?);
            }
            Ok(out)
        };
        for (i, j) in raw.iter().enumerate() {
            if let Some(init) = j.get("initial") {
                let targets = resolve(init.get("targets"), &nav.by_name)?;
                nav.states[i].initial = Some((targets, actions(init.get("actions"))));
            }
            for t in j.get("transitions").and_then(Json::as_array).into_iter().flatten() {
                let tr = Transition {
                    event: t.get("event").and_then(Json::as_str).map(str::to_string),
                    targets: resolve(t.get("targets"), &nav.by_name)?,
                    actions: actions(t.get("actions")),
                };
                nav.states[i].transitions.push(tr);
            }
        }
        Ok(nav)
    }

    fn build<'a>(&mut self, j: &'a Json, parent: Option<usize>, raw: &mut Vec<&'a Json>) -> Result<usize> {
        let name = j.get("name").and_then(Json::as_str).context("nav: a state without a name")?.to_string();
        let kind = j.get("type").and_then(Json::as_str).unwrap_or("state");
        if !matches!(kind, "state" | "parallel") {
            bail!("nav: state {name} has type {kind} (external states are not supported)");
        }
        if self.by_name.contains_key(&name) {
            bail!("nav: two states named {name}");
        }
        let id = self.states.len();
        self.by_name.insert(name.clone(), id);
        self.states.push(State {
            name,
            parallel: kind == "parallel",
            parent,
            children: Vec::new(),
            initial: None,
            on_enter: actions(j.get("onEnter")),
            on_exit: actions(j.get("onExit")),
            transitions: Vec::new(),
        });
        raw.push(j);
        for c in j.get("states").and_then(Json::as_array).into_iter().flatten() {
            let k = self.build(c, Some(id), raw)?;
            self.states[id].children.push(k);
        }
        Ok(id)
    }

    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn event_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> =
            self.states.iter().flat_map(|s| s.transitions.iter().filter_map(|t| t.event.as_deref())).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.by_name.get(name).is_some_and(|i| self.config.contains(i))
    }

    pub fn active_leaves(&self) -> Vec<&str> {
        self.config
            .iter()
            .filter(|&&s| self.states[s].children.is_empty())
            .map(|&s| self.states[s].name.as_str())
            .collect()
    }

    pub fn raise(&mut self, event: &str) {
        self.external.push_back(event.to_string());
    }

    pub fn next_action(&mut self) -> Option<Action> {
        loop {
            if self.awaiting.is_some() {
                return None;
            }
            let Some(step) = self.todo.pop_front() else {
                if !self.started && !self.states.is_empty() {
                    self.started = true;
                    self.busy = true;
                    let plan = self.microstep_plan(&[(None, vec![0], Vec::new())]);
                    self.todo.extend(plan);
                    self.todo.extend([Step::NextInternal, Step::EventDone]);
                    continue;
                }
                let e = self.external.pop_front()?;
                self.busy = true;
                self.internal.push_back(e);
                self.todo.extend([Step::NextInternal, Step::EventDone]);
                continue;
            };
            match step {
                Step::NextInternal => {
                    let Some(e) = self.internal.pop_front() else { continue };
                    let ts = self.select(&e);
                    self.log.push(format!("{e}: {} transitions", ts.len()));
                    self.todo.push_front(Step::NextInternal);
                    if !ts.is_empty() {
                        for st in self.microstep_plan(&ts).into_iter().rev() {
                            self.todo.push_front(st);
                        }
                    }
                }
                Step::Exit(s) => {
                    self.config.retain(|&c| c != s);
                    for a in self.states[s].on_exit.clone().into_iter().rev() {
                        self.todo.push_front(Step::Action(a));
                    }
                }
                Step::Enter(s, default) => {
                    if !self.config.contains(&s) {
                        self.config.push(s);
                    }
                    let mut acts = self.states[s].on_enter.clone();
                    if default {
                        acts.extend(self.states[s].initial.as_ref().map(|(_, a)| a.clone()).unwrap_or_default());
                    }
                    for a in acts.into_iter().rev() {
                        self.todo.push_front(Step::Action(a));
                    }
                }
                Step::SortConfig => self.config.sort_unstable(),
                Step::EventDone => self.busy = false,
                Step::Action(a) => {
                    if a.name == "invoke" {
                        if let Some(e) = a.args.first() {
                            self.internal.push_back(e.clone());
                        }
                        continue;
                    }
                    self.awaiting = Some(a.clone());
                    return Some(a);
                }
            }
        }
    }

    pub fn complete(&mut self) {
        self.awaiting = None;
    }

    pub fn awaiting(&self) -> Option<&Action> {
        self.awaiting.as_ref()
    }

    pub fn busy(&self) -> bool {
        self.busy || !self.todo.is_empty() || self.awaiting.is_some()
    }

    pub fn run(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        while let Some(a) = self.next_action() {
            out.push(a);
            self.complete();
        }
        out
    }

    fn proper_ancestors(&self, s: usize, root: Option<usize>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut p = self.states[s].parent;
        while p != root {
            let Some(x) = p else { break };
            out.push(x);
            p = self.states[x].parent;
        }
        out
    }

    fn is_proper_descendant(&self, s: usize, anc: usize) -> bool {
        let mut p = self.states[s].parent;
        while let Some(x) = p {
            if x == anc {
                return true;
            }
            p = self.states[x].parent;
        }
        false
    }

    fn is_descendant(&self, s: usize, anc: Option<usize>) -> bool {
        match anc {
            None => true,
            Some(a) => s == a || self.is_proper_descendant(s, a),
        }
    }

    fn lca(&self, source: usize, targets: &[usize]) -> Option<usize> {
        self.proper_ancestors(source, None)
            .into_iter()
            .find(|&a| targets.iter().all(|&t| self.is_proper_descendant(t, a)))
    }

    fn select(&self, event: &str) -> Vec<Selected> {
        let mut out: Vec<Selected> = Vec::new();
        let mut taken: Vec<(usize, usize)> = Vec::new();
        for &s in &self.config {
            if !self.states[s].children.is_empty() {
                continue;
            }
            let preempted = out.iter().any(|(src, targets, _)| {
                let Some(src) = *src else { return false };
                !targets.is_empty() && self.is_descendant(s, self.lca(src, targets))
            });
            if preempted {
                continue;
            }
            let chain = std::iter::once(s).chain(self.proper_ancestors(s, None));
            'anc: for a in chain {
                for (k, t) in self.states[a].transitions.iter().enumerate() {
                    if t.event.as_deref() == Some(event) {
                        if !taken.contains(&(a, k)) {
                            taken.push((a, k));
                            out.push((Some(a), t.targets.clone(), t.actions.clone()));
                        }
                        break 'anc;
                    }
                }
            }
        }
        out
    }

    fn add_states_to_enter(&self, s: usize, root: Option<usize>, enter: &mut Vec<usize>, default: &mut Vec<usize>) {
        if !enter.contains(&s) {
            enter.push(s);
        }
        let st = &self.states[s];
        if st.parallel {
            for &c in &st.children {
                self.add_states_to_enter(c, Some(s), enter, default);
            }
        } else if !st.children.is_empty() {
            if !default.contains(&s) {
                default.push(s);
            }
            let initial = st.initial.as_ref().map_or_else(|| vec![st.children[0]], |(t, _)| t.clone());
            for i in initial {
                self.add_states_to_enter(i, Some(s), enter, default);
            }
        }
        for a in self.proper_ancestors(s, root) {
            if !enter.contains(&a) {
                enter.push(a);
            }
            if self.states[a].parallel {
                for &c in &self.states[a].children.clone() {
                    if !enter.iter().any(|&e| self.is_descendant(e, Some(c))) {
                        self.add_states_to_enter(c, Some(a), enter, default);
                    }
                }
            }
        }
    }

    fn microstep_plan(&self, ts: &[Selected]) -> Vec<Step> {
        let mut exit: Vec<usize> = Vec::new();
        let mut enter: Vec<usize> = Vec::new();
        let mut default: Vec<usize> = Vec::new();
        let mut transition_actions = Vec::new();
        for (source, targets, acts) in ts {
            let mut lca = *source;
            if let Some(src) = source
                && !targets.is_empty()
            {
                lca = self.lca(*src, targets);
                for &c in &self.config {
                    let below = match lca {
                        Some(l) => self.is_proper_descendant(c, l),
                        None => true,
                    };
                    if below && !exit.contains(&c) {
                        exit.push(c);
                    }
                }
            }
            transition_actions.extend(acts.iter().cloned());
            for &t in targets {
                self.add_states_to_enter(t, lca, &mut enter, &mut default);
            }
        }
        exit.sort_unstable_by(|a, b| b.cmp(a));
        enter.sort_unstable();
        let mut plan: Vec<Step> = exit.into_iter().map(Step::Exit).collect();
        plan.extend(transition_actions.into_iter().map(Step::Action));
        plan.extend(enter.into_iter().map(|s| Step::Enter(s, default.contains(&s))));
        plan.push(Step::SortConfig);
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harel_rules() {
        let j: Json = serde_json::from_str(
            r#"{"name":"root","type":"parallel","states":[
                {"name":"a","initial":{"targets":["a_off"]},"states":[
                    {"name":"a_on","onEnter":[["loadView",["x"]]],"onExit":[["unloadView",["x"]]],
                     "transitions":[{"event":"a_disable","targets":["a_off"]}]},
                    {"name":"a_off","transitions":[{"event":"a_enable","targets":["a_on"]}]}]},
                {"name":"b","states":[
                    {"name":"b1","onEnter":[["invoke",["a_enable"]]],
                     "transitions":[{"event":"go","targets":["b2"],"actions":[["say",["t"]]]},
                                    {"event":"ping","targets":[],"actions":[["pong",[]]]}]},
                    {"name":"b2","onEnter":[["enter2",[]]]}]}]}"#,
        )
        .unwrap();
        let mut n = Nav::from_json(&j).unwrap();
        let out = n.run();
        assert_eq!(out, vec![Action { name: "loadView".into(), args: vec!["x".into()] }]);
        assert!(n.is_active("a_on") && n.is_active("b1"));
        n.raise("ping");
        assert_eq!(n.run().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["pong"]);
        n.raise("go");
        assert_eq!(n.run().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["say", "enter2"]);
        assert!(n.is_active("b2") && !n.is_active("b1") && n.is_active("a_on"));
        n.raise("a_disable");
        assert_eq!(n.run().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["unloadView"]);
        assert_eq!(n.active_leaves(), ["a_off", "b2"]);
    }

    #[test]
    fn an_action_waits_for_its_continuation() {
        let j: Json = serde_json::from_str(
            r#"{"name":"root","states":[
                {"name":"a","transitions":[{"event":"go","targets":["b"],"actions":[["wait",[]],["after_wait",[]]]}]},
                {"name":"b","onEnter":[["done",[]]],
                 "transitions":[{"event":"leave","targets":["a"],"actions":[["left",[]]]}]}]}"#,
        )
        .unwrap();
        let mut eager = Nav::from_json(&j).unwrap();
        eager.run();
        eager.raise("go");
        eager.raise("leave");
        let names = |v: Vec<Action>| v.into_iter().map(|a| a.name).collect::<Vec<_>>();
        assert_eq!(names(eager.run()), ["wait", "after_wait", "done", "left"]);

        let mut n = Nav::from_json(&j).unwrap();
        assert_eq!(n.next_action(), None, "the start runs no action");
        assert!(!n.busy() && n.is_active("a"));
        n.raise("go");
        assert_eq!(n.next_action().map(|a| a.name), Some("wait".into()));
        n.raise("leave");
        assert_eq!(n.next_action(), None, "waits for the continuation");
        assert!(n.busy() && !n.is_active("a") && !n.is_active("b"), "a exited, b not entered yet");
        n.complete();
        assert_eq!(n.next_action().map(|a| a.name), Some("after_wait".into()));
        n.complete();
        assert_eq!(n.next_action().map(|a| a.name), Some("done".into()));
        assert!(n.is_active("b"), "b is in the configuration before its onEnter");
        n.complete();
        assert_eq!(n.next_action().map(|a| a.name), Some("left".into()), "the queued event runs after the first");
        n.complete();
        assert_eq!(n.next_action(), None);
        assert!(!n.busy() && n.is_active("a"));
    }

    #[test]
    fn invoke_runs_before_the_next_external_event() {
        let j: Json = serde_json::from_str(
            r#"{"name":"root","type":"parallel","states":[
                {"name":"x","states":[{"name":"x1","transitions":[{"event":"go","targets":["x2"],
                    "actions":[["invoke",["inner"]],["first",[]]]}]},{"name":"x2"}]},
                {"name":"y","transitions":[{"event":"inner","actions":[["second",[]]]},
                    {"event":"outer","actions":[["third",[]]]}]}]}"#,
        )
        .unwrap();
        let mut n = Nav::from_json(&j).unwrap();
        n.run();
        n.raise("go");
        n.raise("outer");
        let mut got = Vec::new();
        while let Some(a) = n.next_action() {
            got.push(a.name);
            n.complete();
        }
        assert_eq!(got, ["first", "second", "third"]);
    }
}
