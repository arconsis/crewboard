//! CI results as events: `ci-finished` and `ci-none-seen` ([`ci_scan`]), and the seen map.

use super::*;

/// The result word of a `ci-finished` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiResult {
    Green,
    Red,
    None,
}

impl CiResult {
    pub fn word(self) -> &'static str {
        match self {
            CiResult::Green => "green",
            CiResult::Red => "red",
            CiResult::None => "none",
        }
    }
}

/// The case word in the seen map: `<item id>|ci-finished` -> head SHA.
pub const CI_FINISHED: &str = "ci-finished";
/// `<item id>|ci-none-seen` -> head SHA: the first answer "no CI" for a head. `none` comes with
/// the second one, one throttle period later, because GitHub needs some seconds after a push
/// to register the runs of a head.
pub const CI_NONE_SEEN: &str = "ci-none-seen";

/// The CI of a PR head of an In Progress ticket has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiFinished {
    /// `owner/repo#n` of the ticket.
    pub ticket: String,
    pub repo: String,
    pub issue: u64,
    pub item_id: String,
    pub sha: String,
    /// `owner/repo#n` of the PR.
    pub pr: String,
    pub result: CiResult,
    /// Why the CI is red (the reasons of 8.3); empty otherwise.
    pub reasons: Vec<String>,
}

impl CiFinished {
    /// The URL of the PR, from `owner/repo#n`.
    fn pr_url(&self) -> String {
        match self.pr.split_once('#') {
            Some((repo, n)) => format!("https://github.com/{repo}/pull/{n}"),
            None => self.pr.clone(),
        }
    }

    /// The text of the event for the dev agent.
    pub fn event_body(&self) -> String {
        let head = format!(
            "ci-finished: {}\nticket: {}\npr: {}\nhead: {}\n",
            self.result.word(),
            self.ticket,
            self.pr,
            self.sha
        );
        let short: String = self.sha.chars().take(7).collect();
        match self.result {
            CiResult::Green => format!(
                "{head}The CI of PR {} is green on head {short}.\n\
                 next: crewboard done {} --pr {} --sha {}\n\
                 If more work is open, push it or run `block` and say why.\n",
                self.pr,
                self.issue,
                self.pr_url(),
                self.sha
            ),
            CiResult::Red => {
                let why: String = self
                    .reasons
                    .iter()
                    .take(5)
                    .map(|r| format!("- {r}\n"))
                    .collect();
                format!(
                    "{head}The CI of PR {} is red on head {short}:\n{why}\
                     Fix the cause and push. Do not run `done` on this head. The next head SHA gives a new event.\n",
                    self.pr
                )
            }
            CiResult::None => format!(
                "{head}No CI ran for head {short} of PR {}.\n\
                 If the ticket declares `CI: none`, run `done` now. Else check the PR: a crew branch needs a workflow that runs for it.\n",
                self.pr
            ),
        }
    }
}

/// What [`ci_scan`] found.
#[derive(Debug, Default)]
pub struct CiScan {
    /// GitHub was asked for the PR list of an In Progress ticket in this run.
    pub read: bool,
    pub events: Vec<CiFinished>,
    /// `(item id, head SHA)` where the first "no CI" answer came: store them in the seen map.
    pub none_seen: Vec<(String, String)>,
}

/// `ci-finished` for the In Progress tickets of the dev agent of `inp.scope`
/// (spec/events#stale-work): one event per PR head SHA. Only `Scope::Dev`; the reads are those of
/// the late waits (`probe` caches them), and a head whose event was given is not read. A head whose
/// CI is not finished, and a head whose CI cannot be read, give nothing.
pub fn ci_scan(inp: &Inputs<'_>, probe: &dyn Probe) -> CiScan {
    let mut out = CiScan::default();
    let Scope::Dev(me) = inp.scope else {
        return out;
    };
    if !inp.github {
        return out;
    }
    let mut sorted: Vec<&TicketRow> = inp.rows.iter().collect();
    sorted.sort_by_key(|r| r.key());
    for r in sorted {
        if !r.in_status(Status::InProgress) || r.agent.as_deref() != Some(me) {
            continue;
        }
        if probe.ci_off() {
            return out;
        }
        let prefix = probe.branch_prefix();
        let want = format!("{prefix}{me}/{}-", r.issue);
        out.read = true;
        let Some(pr) = probe.open_pr(&r.repo, &want).flatten() else {
            continue;
        };
        let seen = |case: &str| {
            inp.seen
                .get(&format!("{}|{case}", r.item_id))
                .map(String::as_str)
        };
        if seen(CI_FINISHED) == Some(pr.head_sha.as_str()) {
            continue;
        }
        let (result, reasons) = match probe.ci_state(&r.repo, &pr) {
            None | Some(CiState::Pending) => continue,
            Some(CiState::Green(_)) => (CiResult::Green, Vec::new()),
            Some(CiState::Red(why)) => (CiResult::Red, why),
            Some(CiState::NoCi) => {
                if seen(CI_NONE_SEEN) != Some(pr.head_sha.as_str()) {
                    out.none_seen.push((r.item_id.clone(), pr.head_sha.clone()));
                    continue;
                }
                (CiResult::None, Vec::new())
            }
        };
        out.events.push(CiFinished {
            ticket: r.key(),
            repo: r.repo.clone(),
            issue: r.issue,
            item_id: r.item_id.clone(),
            pr: format!("{}#{}", r.repo, pr.number),
            sha: pr.head_sha,
            result,
            reasons,
        });
    }
    out
}

/// The case word of a seen key (`<item id>|<case>` or `<item id>|<case>|<ns>`) names the status in
/// which the entry can hold.
fn status_of_case(case: &str) -> Status {
    match case {
        "review-not-verified" | "owner-merge" => Status::Review,
        "take" => Status::Ready,
        "answer" | "owner" => Status::Blocked,
        _ => Status::InProgress,
    }
}

/// Drop the seen entries of tickets that left the status in which the case can hold, so a
/// later case of the same ticket is a new event.
pub fn prune_seen(seen: &mut BTreeMap<String, String>, rows: &[TicketRow]) {
    seen.retain(|key, _| {
        let mut parts = key.split('|');
        let (Some(item), Some(case)) = (parts.next(), parts.next()) else {
            return false;
        };
        let status = status_of_case(case);
        rows.iter()
            .any(|r| r.item_id == item && r.status_board() == Some(status))
    });
}

/// When did the CI of `sha` turn green? The latest completion among the counted workflow
/// runs, the successful check runs and the successful commit statuses in the answers that
/// were read for the CI judgement. `None` if no answer names a completion time (nothing is
/// guessed).
pub fn green_since_of(bodies: &[Value], sha: &str) -> Option<i64> {
    let ts = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .and_then(crate::clock::parse_ts)
    };
    let own = |v: &Value| {
        v.get("head_sha")
            .and_then(Value::as_str)
            .is_some_and(|h| h.eq_ignore_ascii_case(sha))
    };
    let mut best: Option<i64> = None;
    let mut take = |t: Option<i64>| {
        if let Some(t) = t {
            best = Some(best.map_or(t, |b| b.max(t)));
        }
    };
    for b in bodies {
        if let Some(runs) = b.get("workflow_runs").and_then(Value::as_array) {
            for r in runs.iter().filter(|r| own(r)) {
                let counted = r
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(|e| crate::domain::ci::evaluate::COUNTED_EVENTS.contains(&e));
                if counted && r.get("conclusion").and_then(Value::as_str) == Some("success") {
                    take(ts(r, "updated_at"));
                }
            }
        }
        if let Some(checks) = b.get("check_runs").and_then(Value::as_array) {
            for c in checks.iter().filter(|c| own(c)) {
                if c.get("conclusion").and_then(Value::as_str) == Some("success") {
                    take(ts(c, "completed_at"));
                }
            }
        }
        // The list of commit statuses is a plain array.
        if let Some(list) = b.as_array() {
            for s in list {
                if s.get("state").and_then(Value::as_str) == Some("success") {
                    take(ts(s, "updated_at"));
                }
            }
        }
    }
    best
}
