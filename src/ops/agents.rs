//! `agents [--can <requirement>...] [--json]` (spec/profiles#requirements-and-matching): the active
//! agents that meet the requirements, ordered for an assignment: fewest open tickets first, then
//! the cheaper tier.

use super::profile::caps_of;
use super::{AgentCtx, Ctx, Global, load_world, new_client, resolve_board_config};
use crate::capabilities::{Caps, is_met, parse_requirement, tier_rank};
use crate::clock::now_epoch;
use crate::error::{CbError, CbResult};
use crate::owner_cmd::role_name;
use crate::rules::status::Status;
use crate::services::ticket_index::{discover_tickets, read_ticket_rows};
use crate::state::StateCache;
use crate::state::TicketIndex;
use crate::ticket::TicketRow;
use serde_json::json;

/// One agent as the command sees it. Built from the board in [`run`]; the selection and the
/// order are pure functions of these rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub role: String,
    /// `State` of the agent item.
    pub state: String,
    /// `alive`, `late`, `dead` or `-`.
    pub liveness: String,
    pub heartbeat_age: Option<i64>,
    pub load: usize,
    pub caps: Caps,
    pub capabilities: String,
}

/// Why an agent is not in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// `State` is not `active` (or the legacy `working`) (requested, approved, paused, draining, left).
    NotActive(String),
    /// Active, but the heartbeat is late or the agent is dead.
    NotAlive(String),
    Lacks(Vec<String>),
}

impl Skip {
    pub fn text(&self) -> String {
        match self {
            Skip::NotActive(s) => format!("state {s}"),
            Skip::NotAlive(l) => format!("liveness {l}"),
            Skip::Lacks(m) => format!("lacks {}", m.join(", ")),
        }
    }
}

/// Open tickets of an agent: every ticket on the board with this `Agent` that is not `Done`.
pub fn load_of(rows: &[TicketRow], agent: &str) -> usize {
    rows.iter()
        .filter(|r| r.is_agent(agent))
        .filter(|r| r.status.is_some() && !r.in_status(Status::Done))
        .count()
}

/// Parse the `--can` values: each may hold several requirements separated by commas or spaces.
pub fn parse_can(values: &[String]) -> CbResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        for t in v.split(|c: char| c == ',' || c.is_whitespace()) {
            if t.is_empty() {
                continue;
            }
            let r = parse_requirement(t)?;
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    Ok(out)
}

/// Split the candidates into the ordered list and the skipped ones with their reason. The
/// order is fewest open tickets, then the cheaper tier (`low`, `mid`, `high`, other names, no
/// tier), then the name.
pub fn select(
    mut all: Vec<Candidate>,
    needs: &[String],
) -> (Vec<Candidate>, Vec<(Candidate, Skip)>) {
    let mut listed = Vec::new();
    let mut skipped = Vec::new();
    for c in all.drain(..) {
        if !matches!(c.state.as_str(), "active" | "working") {
            let s = Skip::NotActive(c.state.clone());
            skipped.push((c, s));
        } else if matches!(c.liveness.as_str(), "late" | "dead") {
            let s = Skip::NotAlive(c.liveness.clone());
            skipped.push((c, s));
        } else {
            let miss: Vec<String> = needs
                .iter()
                .filter(|n| !is_met(n, &c.caps))
                .cloned()
                .collect();
            if miss.is_empty() {
                listed.push(c);
            } else {
                skipped.push((c, Skip::Lacks(miss)));
            }
        }
    }
    listed.sort_by(|a, b| {
        (a.load, tier_rank(a.caps.tier.as_deref()), &a.name).cmp(&(
            b.load,
            tier_rank(b.caps.tier.as_deref()),
            &b.name,
        ))
    });
    (listed, skipped)
}

fn age(a: Option<i64>) -> String {
    a.map_or("-".to_string(), crate::clock::fmt_age)
}

fn tags_of(c: &Caps) -> String {
    if c.declared.is_empty() {
        "-".to_string()
    } else {
        c.declared.iter().cloned().collect::<Vec<_>>().join(",")
    }
}

pub fn render_text(
    listed: &[Candidate],
    skipped: &[(Candidate, Skip)],
    needs: &[String],
    initiative: &str,
) -> String {
    let mut s = format!(
        "agents of {initiative}{}:\n",
        if needs.is_empty() {
            String::new()
        } else {
            format!(" that can: {}", needs.join(", "))
        }
    );
    if listed.is_empty() {
        s.push_str("  (none)\n");
    }
    for c in listed {
        s.push_str(&format!(
            "  {:<22} {:<11} {:<8} hb {:<5} load {:<2} tier {:<6} tags {}\n",
            c.name,
            c.role,
            c.state,
            age(c.heartbeat_age),
            c.load,
            c.caps.tier.as_deref().unwrap_or("-"),
            tags_of(&c.caps)
        ));
    }
    if !skipped.is_empty() {
        s.push_str(&format!("not listed ({}):\n", skipped.len()));
        for (c, why) in skipped {
            s.push_str(&format!("  {:<22} {}\n", c.name, why.text()));
        }
    }
    s
}

pub fn render_json(
    listed: &[Candidate],
    skipped: &[(Candidate, Skip)],
    needs: &[String],
    initiative: &str,
) -> String {
    let one = |c: &Candidate| {
        json!({
            "name": c.name, "role": c.role, "state": c.state, "liveness": c.liveness,
            "heartbeat_age_s": c.heartbeat_age, "load": c.load, "tier": c.caps.tier,
            "tags": c.caps.declared, "facts": c.caps.facts, "tools_cut": c.caps.tools_cut,
            "capabilities": c.capabilities,
        })
    };
    json!({
        "initiative": initiative,
        "can": needs,
        "agents": listed.iter().map(one).collect::<Vec<_>>(),
        "not_listed": skipped.iter().map(|(c, why)| json!({"name": c.name, "why": why.text()})).collect::<Vec<_>>(),
    })
    .to_string()
        + "\n"
}

pub fn run(g: &Global, can: &[String], json_out: bool) -> CbResult<String> {
    // Requirements first: a typo must not read the board.
    let needs = parse_can(can)?;
    let (ctx, mut cache, extra) = match AgentCtx::load(g) {
        Ok(ac) => {
            let c = ac.load_cache();
            let extra = vec![ac.agent.item_id.clone()];
            (ac.ctx, c, extra)
        }
        Err(_) => {
            let cfg = resolve_board_config(g)?;
            (
                Ctx::load(new_client()?, cfg, g.dir()?)?,
                StateCache::default(),
                vec![],
            )
        }
    };
    let world = load_world(&ctx, &extra, &mut cache)?;
    let now = now_epoch();
    let mut tmp = StateCache {
        index: TicketIndex {
            tickets: cache.index.tickets.clone(),
            ..TicketIndex::default()
        },
        ..StateCache::default()
    };
    discover_tickets(&ctx.reader(), &world.trust, &mut tmp.index)?;
    let trows = read_ticket_rows(&ctx.reader(), &mut tmp.index.tickets)?;
    let mut all = Vec::new();
    for e in world.reg.agents.values().filter(|e| e.approved()) {
        let Some(t) = world.trust.get(&e.name) else {
            continue; // approved but not trusted: `status` shows why
        };
        let item = world.items.get(&t.item_id);
        let (_, hb, live) = crate::ctx::agent_liveness(&world, &e.name, now, ctx.hb_interval);
        all.push(Candidate {
            name: e.name.clone(),
            role: role_name(t.role).to_string(),
            state: item
                .and_then(|i| i.field("State"))
                .unwrap_or("?")
                .to_string(),
            liveness: live.word().to_string(),
            heartbeat_age: hb.map(|h| now - h),
            load: load_of(&trows, &e.name),
            caps: caps_of(item),
            capabilities: item
                .and_then(|i| i.field(super::profile::FIELD))
                .unwrap_or("")
                .to_string(),
        });
    }
    if all.is_empty() && !world.trust.problems.is_empty() {
        return Err(CbError::config(format!(
            "no agent is trusted: {}",
            world.trust.problems.join("; ")
        )));
    }
    let (listed, skipped) = select(all, &needs);
    Ok(if json_out {
        render_json(&listed, &skipped, &needs, &ctx.cfg.initiative)
    } else {
        render_text(&listed, &skipped, &needs, &ctx.cfg.initiative)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, state: &str, live: &str, load: usize, field: &str) -> Candidate {
        Candidate {
            name: name.into(),
            role: "dev".into(),
            state: state.into(),
            liveness: live.into(),
            heartbeat_age: Some(30),
            load,
            caps: Caps::parse(field),
            capabilities: field.into(),
        }
    }

    fn names(v: &[Candidate]) -> Vec<&str> {
        v.iter().map(|c| c.name.as_str()).collect()
    }

    fn pool() -> Vec<Candidate> {
        vec![
            cand(
                "a@x",
                "active",
                "alive",
                2,
                "x86-builds e2e tier:high arch:x86_64",
            ),
            cand(
                "b@x",
                "active",
                "alive",
                0,
                "x86-builds tier:high arch:x86_64",
            ),
            cand(
                "c@x",
                "working",
                "alive",
                0,
                "x86-builds tier:low arch:x86_64",
            ),
            cand("d@x", "active", "alive", 0, "gpu arch:arm64"),
            cand("e@x", "paused", "alive", 0, "x86-builds arch:x86_64"),
            cand("f@x", "active", "late", 0, "x86-builds arch:x86_64"),
            cand("g@x", "active", "dead", 0, "x86-builds arch:x86_64"),
            cand("h@x", "left", "-", 0, "x86-builds arch:x86_64"),
            cand("i@x", "active", "alive", 0, "x86-builds arch:x86_64"),
        ]
    }

    #[test]
    fn requirements_filter_and_each_reason_for_a_skip_is_named() {
        let needs = parse_can(&["x86_64,x86-builds".to_string()]).unwrap();
        let (listed, skipped) = select(pool(), &needs);
        assert_eq!(names(&listed), ["c@x", "b@x", "i@x", "a@x"]);
        let why = |n: &str| skipped.iter().find(|(c, _)| c.name == n).unwrap().1.clone();
        assert_eq!(
            why("d@x"),
            Skip::Lacks(vec!["arch:x86_64".into(), "x86-builds".into()])
        );
        assert_eq!(why("e@x"), Skip::NotActive("paused".into()));
        assert_eq!(why("f@x"), Skip::NotAlive("late".into()));
        assert_eq!(why("g@x"), Skip::NotAlive("dead".into()));
        assert_eq!(why("h@x"), Skip::NotActive("left".into()));
        assert_eq!(
            listed.len() + skipped.len(),
            9,
            "every agent is accounted for"
        );
    }

    #[test]
    fn the_order_is_load_then_cheaper_tier_then_name() {
        let (listed, _) = select(pool(), &[]);
        // load 0: c (low), b (high), d (no needs; no tier), i (no tier): low < high < none;
        // d and i have no tier, so they sort by name; load 2 last
        assert_eq!(names(&listed), ["c@x", "b@x", "d@x", "i@x", "a@x"]);
        // a lower load beats a cheaper tier
        let (l, _) = select(
            vec![
                cand("cheap@x", "active", "alive", 3, "tier:low"),
                cand("free@x", "active", "alive", 1, "tier:high"),
            ],
            &[],
        );
        assert_eq!(names(&l), ["free@x", "cheap@x"]);
    }

    #[test]
    fn no_requirement_lists_every_active_agent_and_an_impossible_one_lists_none() {
        let (all, _) = select(pool(), &[]);
        assert_eq!(all.len(), 5);
        let needs = parse_can(&["gpu".to_string(), "x86_64".to_string()]).unwrap();
        let (none, skipped) = select(pool(), &needs);
        assert!(none.is_empty());
        assert_eq!(skipped.len(), 9);
        assert!(render_text(&none, &skipped, &needs, "t").contains("(none)"));
    }

    #[test]
    fn can_values_are_split_validated_and_deduplicated() {
        assert_eq!(
            parse_can(&["x86_64, e2e".into(), "e2e".into(), "AMD64".into()]).unwrap(),
            ["arch:x86_64", "e2e"]
        );
        assert!(parse_can(&[]).unwrap().is_empty());
        for bad in ["Not A Tag!", "foo:bar", "tool:"] {
            assert_eq!(
                parse_can(&[bad.to_string()]).unwrap_err().exit(),
                crate::exit::Exit::Refused,
                "{bad}"
            );
        }
    }

    #[test]
    fn load_counts_open_tickets_of_the_agent_only() {
        let row = |agent: Option<&str>, status: Option<&str>| TicketRow {
            repo: "a/b".into(),
            issue: 1,
            item_id: "PVTI_x".into(),
            status: status.map(Into::into),
            agent: agent.map(str::to_string),
            order: None,
            updated: None,
        };
        let rows = vec![
            row(Some("a@x"), Some("In Progress")),
            row(Some("a@x"), Some("Review")),
            row(Some("a@x"), Some("Done")),
            row(Some("a@x"), None),
            row(Some("b@x"), Some("Ready")),
            row(None, Some("Backlog")),
        ];
        assert_eq!(
            load_of(&rows, "a@x"),
            2,
            "Done and a missing status do not count"
        );
        assert_eq!(load_of(&rows, "b@x"), 1);
        assert_eq!(load_of(&rows, "nobody@x"), 0);
    }

    #[test]
    fn the_json_has_one_object_per_listed_agent_with_the_asked_columns() {
        let (l, s) = select(pool(), &parse_can(&["e2e".to_string()]).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&render_json(&l, &s, &["e2e".into()], "t")).unwrap();
        assert!(v["agents"].is_array());
        assert_eq!(v["agents"].as_array().unwrap().len(), 1);
        let a = &v["agents"][0];
        assert_eq!(a["name"], "a@x");
        for k in [
            "role",
            "state",
            "liveness",
            "heartbeat_age_s",
            "load",
            "tier",
            "tags",
        ] {
            assert!(a.get(k).is_some(), "{k}");
        }
        assert_eq!(a["tier"], "high");
        assert_eq!(a["load"], 2);
        assert_eq!(v["not_listed"].as_array().unwrap().len(), 8);
    }
}
