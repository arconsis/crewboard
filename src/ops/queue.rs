//! `status --queue`: per dev agent the In Progress ticket and the Ready queue in
//! order, with counts, and one line when the queues are out of balance.
//!
//! The builders are pure: `status` reads the agents and the tickets and passes them in.

use super::agent_row::{AgentRow, caps_of_row, declared_tags};
use super::mine::select_mine;
use crate::rules::status::Status;
use crate::ticket::TicketRow;
use serde_json::{Value, json};

/// A queue is out of balance when one agent has at least this many more Ready tickets than
/// another agent that can take the same work.
pub const IMBALANCE_GAP: usize = 3;

/// The queue of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentQueue {
    pub name: String,
    pub state: String,
    pub liveness: String,
    /// Declared tags (not facts).
    pub tags: Vec<String>,
    /// The `arch:` fact, if known.
    pub arch: Option<String>,
    pub in_progress: Vec<TicketRow>,
    /// Ready tickets, next ticket first.
    pub ready: Vec<TicketRow>,
}

impl AgentQueue {
    /// Takes new work now: State `active` (or the legacy `working` of an old board), and alive.
    pub fn takes_work(&self) -> bool {
        matches!(self.state.as_str(), "active" | "working") && self.liveness == "alive"
    }
}

fn arch_of(r: &AgentRow) -> Option<String> {
    caps_of_row(r)
        .facts
        .iter()
        .find_map(|f| f.strip_prefix("arch:").map(str::to_string))
}

/// The queues of all approved dev agents that have not left, by name.
pub fn build(agents: &[AgentRow], rows: &[TicketRow]) -> Vec<AgentQueue> {
    let mut v: Vec<AgentQueue> = agents
        .iter()
        .filter(|a| {
            a.approved
                && !a.rejected
                && a.role.as_deref() == Some("dev")
                && a.state.as_deref() != Some("left")
        })
        .map(|a| {
            let mine = select_mine(rows, &a.name);
            let (in_progress, ready) = mine
                .into_iter()
                .partition(|r| r.in_status(Status::InProgress));
            AgentQueue {
                name: a.name.clone(),
                state: a.state.clone().unwrap_or_else(|| "?".into()),
                liveness: a.liveness.clone(),
                tags: declared_tags(a),
                arch: arch_of(a),
                in_progress,
                ready,
            }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Ready tickets that have no agent.
pub fn unassigned_ready(rows: &[TicketRow]) -> Vec<&TicketRow> {
    rows.iter()
        .filter(|r| r.in_status(Status::Ready) && r.agent.is_none())
        .collect()
}

/// `b` can take what `a` takes: same architecture and all declared tags of `a`.
fn can_take_same_work(a: &AgentQueue, b: &AgentQueue) -> bool {
    a.arch == b.arch && a.tags.iter().all(|t| b.tags.contains(t))
}

/// One line when an agent has [`IMBALANCE_GAP`] or more Ready tickets than another agent that
/// takes work now and can take the same work. The pair with the largest gap is named.
pub fn imbalance_note(queues: &[AgentQueue]) -> Option<String> {
    let mut best: Option<(usize, &AgentQueue, &AgentQueue)> = None;
    for a in queues {
        for b in queues {
            if a.name == b.name || !b.takes_work() {
                continue;
            }
            let (na, nb) = (a.ready.len(), b.ready.len());
            if na < nb + IMBALANCE_GAP || !can_take_same_work(a, b) {
                continue;
            }
            let gap = na - nb;
            if best.is_none_or(|(g, _, _)| gap > g) {
                best = Some((gap, a, b));
            }
        }
    }
    best.map(|(_, a, b)| {
        format!(
            "imbalance: {} has {} Ready tickets, {} has {} and can take the same work; consider `crewboard assign <n> --to {}`",
            a.name,
            a.ready.len(),
            b.name,
            b.ready.len(),
            b.name
        )
    })
}

fn order_text(r: &TicketRow) -> String {
    r.order.map_or("-".to_string(), |o| o.to_string())
}

/// The text of `status --queue`.
pub fn render(
    queues: &[AgentQueue],
    unassigned: &[&TicketRow],
    note: Option<&str>,
    board_has_order: bool,
) -> String {
    let mut s = String::from(
        "queues (In Progress, then Ready in order; the first Ready ticket is next):\n",
    );
    if !board_has_order {
        s.push_str("note: the board has no `Order` field yet; run `crewboard init`, or `assign --order` creates it. Tickets go by issue number until then.\n");
    }
    if queues.is_empty() {
        s.push_str("  no dev agent\n");
    }
    for q in queues {
        s.push_str(&format!(
            "\n{} [{}, {}]: {} In Progress, {} Ready\n",
            q.name,
            q.state,
            q.liveness,
            q.in_progress.len(),
            q.ready.len()
        ));
        for r in &q.in_progress {
            s.push_str(&format!("  now   {:<34}\n", r.key()));
        }
        for (i, r) in q.ready.iter().enumerate() {
            s.push_str(&format!(
                "  {:>2}.   {:<34} order {}\n",
                i + 1,
                r.key(),
                order_text(r)
            ));
        }
    }
    if !unassigned.is_empty() {
        s.push_str(&format!(
            "\nReady without an agent ({}): {}\n",
            unassigned.len(),
            unassigned
                .iter()
                .map(|r| r.key())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(n) = note {
        s.push_str(&format!("\n{n}\n"));
    }
    s
}

pub fn to_json(queues: &[AgentQueue], unassigned: &[&TicketRow], note: Option<&str>) -> Value {
    json!({
        "agents": queues.iter().map(|q| json!({
            "agent": q.name, "state": q.state, "liveness": q.liveness,
            "in_progress": q.in_progress.iter().map(TicketRow::key).collect::<Vec<_>>(),
            "ready": q.ready.iter().map(|r| json!({
                "ticket": r.key(), "order": r.order})).collect::<Vec<_>>(),
            "ready_count": q.ready.len(),
        })).collect::<Vec<_>>(),
        "ready_without_agent": unassigned.iter().map(|r| r.key()).collect::<Vec<_>>(),
        "imbalance": note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, state: &str, live: &str, caps: &str) -> AgentRow {
        AgentRow {
            name: name.into(),
            approved: true,
            rejected: false,
            trusted: true,
            role: Some("dev".into()),
            state: Some(state.into()),
            heartbeat: None,
            heartbeat_age: None,
            runtime: None,
            host: None,
            item_id: None,
            capabilities: Some(caps.into()),
            liveness: live.into(),
            busy: Vec::new(),
        }
    }

    fn row(n: u64, status: &str, agent: Option<&str>, order: Option<i64>) -> TicketRow {
        TicketRow {
            repo: "a/b".into(),
            issue: n,
            item_id: format!("PVTI_{n}"),
            status: Some(status.into()),
            agent: agent.map(str::to_string),
            order,
            updated: None,
        }
    }

    /// `n` Ready tickets for `agent`, numbered from `first`.
    fn ready(agent: &str, first: u64, n: u64) -> Vec<TicketRow> {
        (0..n)
            .map(|i| row(first + i, "Ready", Some(agent), Some(i as i64 + 1)))
            .collect()
    }

    #[test]
    fn a_queue_lists_in_progress_then_ready_in_order_with_counts() {
        let agents = vec![agent("a@x", "working", "alive", "arch:arm64")];
        let rows = vec![
            row(1, "In Progress", Some("a@x"), None),
            row(40, "Ready", Some("a@x"), Some(2)),
            row(38, "Ready", Some("a@x"), Some(1)),
            row(32, "Ready", Some("a@x"), Some(3)),
            row(5, "Review", Some("a@x"), None),
            row(6, "Ready", Some("b@x"), Some(1)),
            row(7, "Ready", None, None),
        ];
        let q = build(&agents, &rows);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].in_progress.len(), 1);
        let order: Vec<u64> = q[0].ready.iter().map(|r| r.issue).collect();
        assert_eq!(order, [38, 40, 32]);
        let un = unassigned_ready(&rows);
        let text = render(&q, &un, None, true);
        assert!(
            text.contains("a@x [working, alive]: 1 In Progress, 3 Ready"),
            "{text}"
        );
        let p38 = text.find("a/b#38").unwrap();
        assert!(
            p38 < text.find("a/b#40").unwrap()
                && text.find("a/b#40").unwrap() < text.find("a/b#32").unwrap()
        );
        assert!(text.contains("Ready without an agent (1): a/b#7"), "{text}");
        assert!(!text.contains("a/b#6 "), "another agent's ticket: {text}");
        assert!(!text.contains("Order` field"), "board has Order: {text}");
        // control: a board without Order says so
        assert!(render(&q, &un, None, false).contains("no `Order` field"));
    }

    #[test]
    fn only_approved_dev_agents_that_have_not_left_have_a_queue() {
        let mut coord = agent("c@x", "active", "alive", "");
        coord.role = Some("coordinator".into());
        let mut gone = agent("g@x", "left", "-", "");
        gone.approved = true;
        let mut pending = agent("p@x", "requested", "-", "");
        pending.approved = false;
        let dev = agent("d@x", "active", "alive", "");
        let names: Vec<String> = build(&[coord, gone, pending, dev], &[])
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert_eq!(names, ["d@x"]);
    }

    fn queues(a_ready: u64, b_ready: u64, b: AgentRow, a: AgentRow) -> Vec<AgentQueue> {
        let mut rows = ready(&a.name, 100, a_ready);
        rows.extend(ready(&b.name, 200, b_ready));
        build(&[a, b], &rows)
    }

    #[test]
    fn the_imbalance_note_appears_at_a_gap_of_three_and_names_the_agents() {
        let a = agent("a@x", "working", "alive", "arch:arm64 e2e");
        let b = agent("b@x", "active", "alive", "arch:arm64 e2e");
        let note = imbalance_note(&queues(6, 1, b.clone(), a.clone())).expect("gap 5");
        assert!(
            note.contains("a@x has 6") && note.contains("b@x has 1"),
            "{note}"
        );
        assert!(note.contains("--to b@x"), "{note}");
        // exactly 3 more is the limit
        assert!(imbalance_note(&queues(4, 1, b.clone(), a.clone())).is_some());
        // the note is one line
        assert_eq!(note.lines().count(), 1);
    }

    #[test]
    fn the_imbalance_note_does_not_appear_without_a_gap_or_a_fit() {
        let a = agent("a@x", "working", "alive", "arch:arm64 e2e");
        let b = agent("b@x", "active", "alive", "arch:arm64 e2e");
        // gap of 2
        assert!(imbalance_note(&queues(3, 1, b.clone(), a.clone())).is_none());
        // both equal
        assert!(imbalance_note(&queues(5, 5, b.clone(), a.clone())).is_none());
        // the idle agent lacks a tag of the busy one
        let b_less = agent("b@x", "active", "alive", "arch:arm64");
        assert!(imbalance_note(&queues(6, 0, b_less, a.clone())).is_none());
        // another architecture
        let b_x86 = agent("b@x", "active", "alive", "arch:x86_64 e2e");
        assert!(imbalance_note(&queues(6, 0, b_x86, a.clone())).is_none());
        // the idle agent is late, paused, or does not take work
        for (st, lv) in [("active", "late"), ("active", "dead"), ("paused", "alive")] {
            let b_off = agent("b@x", st, lv, "arch:arm64 e2e");
            assert!(
                imbalance_note(&queues(6, 0, b_off, a.clone())).is_none(),
                "{st} {lv}"
            );
        }
        // control: the same pair with a fitting, alive, active target does fire
        assert!(imbalance_note(&queues(6, 0, b, a)).is_some());
    }

    #[test]
    fn a_busy_agent_with_fewer_tags_is_compared_with_one_that_has_more() {
        // the idle agent has every tag of the busy one (and more): it can take the work
        let a = agent("a@x", "working", "alive", "arch:arm64 e2e");
        let b = agent("b@x", "active", "alive", "arch:arm64 e2e gpu");
        assert!(imbalance_note(&queues(6, 0, b, a)).is_some());
    }

    #[test]
    fn the_json_carries_the_counts_and_the_note() {
        let a = agent("a@x", "working", "alive", "arch:arm64");
        let b = agent("b@x", "active", "alive", "arch:arm64");
        let q = queues(5, 1, b, a);
        let note = imbalance_note(&q);
        let j = to_json(&q, &[], note.as_deref());
        assert_eq!(j["agents"][0]["ready_count"], 5);
        assert_eq!(j["agents"][1]["ready_count"], 1);
        assert_eq!(j["agents"][0]["ready"][0]["order"], 1);
        assert!(j["imbalance"].as_str().unwrap().starts_with("imbalance:"));
    }
}
