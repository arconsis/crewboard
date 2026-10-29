//! `done`: a dev agent delivers a ticket with a PR and its evidence (Review).
//!
//! Three steps: [`load_done`] reads the ticket, [`plan_done`] reads the PR and the CI and
//! decides ([`decide_done`] is the pure part, a [`DonePlan`] its result), [`act_done`] performs
//! the writes of the plan in order.

use super::in_progress::write_idle_when_free;
use super::local_gate::local_gate_for_done;
use super::merged_pr;
use super::pr_evidence::{
    PrFiles, check_pr_in_initiative, fetch_pr, fetch_pr_files_upto, judge_no_run_for, named_runs,
};
use super::{
    Me, facts, load_world_cached, me_of, needs_owner_lines, post_with, remember, unread_guard,
    work_write_ctx_of,
};
use crate::authority::Authority;
use crate::clock::{Clock, SystemClock};
use crate::domain::ci::evaluate::NamedRun;
use crate::error::CbResult;
use crate::ops::behind::{behind_by, done_warning};
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::owner_route::Route;
use crate::protocol::names::{Kind, Re, To};
use crate::rules::Actor;
use crate::rules::Refusal;
use crate::rules::evidence::{
    CiFacts, EvidenceInput, PrFacts, PrUrl, judge_evidence, parse_pr_url,
};
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::limits::check_text_field;
use crate::rules::protected::MAX_FILES;
use crate::rules::status::{Status, TicketFacts, check_status_transition};
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_loader::Ticket;
use crate::services::ticket_loader::load_ticket;
use crate::ticket::VMsg;
use crate::ticket::latest_done;
use crate::ticket::{EvidenceMsg, ci_none_declared, done_body, evidence_field, question_open};

pub struct DoneArgs<'a> {
    pub issue: &'a str,
    pub pr: &'a str,
    /// The SHA the dev worked on; the PR head must still be this commit.
    pub sha: Option<&'a str>,
    /// Post although a message for this agent is unread on the ticket
    /// (spec/events#unread-messages-on-a-ticket).
    pub anyway: bool,
    /// Deliver although events for this agent are unread (spec/events#event-checkpoints-and-done).
    pub ignore_events: bool,
    /// `--run`: CI runs named as part of the evidence (spec/ci#which-ci-runs-count).
    pub runs: &'a [String],
    /// `--local-run`: the local gate of the dev, for `ci_mode: local`
    /// (spec/ci#local-run-ci_mode-local).
    pub local_gate: Option<&'a str>,
}

/// The `ci-info:` text of a local-gate `done`: what the CI of the head said, as information.
fn ci_info_text(ci: &CiFacts) -> String {
    let t = match ci {
        CiFacts::Green { .. } => "green (information only)".to_string(),
        CiFacts::NotGreen(r) => format!("not green (information only): {}", r.join("; ")),
        CiFacts::NoCi(_) => "no CI run (information only)".to_string(),
        CiFacts::OnlyInformation { info, .. } => {
            format!("only runs that are information: {}", info.join(", "))
        }
        CiFacts::NotQueried(w) => format!("not readable (information only): {w}"),
    };
    let t = t.replace(['\n', '\r'], " ");
    match t.char_indices().nth(300) {
        Some((i, _)) => format!("{}...", &t[..i]),
        None => t,
    }
}

/// What `done` has loaded before it decides: the arguments, the agent and the ticket.
struct DoneLoad {
    repo: String,
    n: u64,
    pr_url: PrUrl,
    world: World,
    me: Me,
    t: Ticket,
    from: Status,
    /// The unread lines (events and messages) that `--override-unread` lets pass.
    unread_line: Option<String>,
}

/// The reads of `done` that come before any decision about the PR.
fn load_done(ac: &AgentCtx, a: &DoneArgs<'_>) -> CbResult<DoneLoad> {
    let pr_url = parse_pr_url(a.pr)?;
    let (repo, n) = parse_issue_arg(a.issue, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    // Events for this agent that `watch` has not delivered. Before the first write.
    let events_line = match crate::ops::watch::unread_for_done(ac, &world, (&repo, n))? {
        Some(unread) => crate::unread::events_verdict(&unread, a.ignore_events)?,
        None => None,
    };
    crate::ops::say::refresh_heartbeat(ac, &world);
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    t.check_agent_signed()?;
    // --override-unread also covers the unread messages of this ticket (the same messages).
    let unread_line = unread_guard(ac, &world, &t, a.anyway || events_line.is_some())?;
    let unread_line = match (events_line, unread_line) {
        (Some(e), Some(u)) => Some(format!("{e}\n{u}")),
        (e, u) => e.or(u),
    };
    let from = t.status()?;
    Ok(DoneLoad {
        repo,
        n,
        pr_url,
        world,
        me,
        t,
        from,
        unread_line,
    })
}

/// The checks that need no PR: writer, state, authority, `--local-run` and the repo of the PR.
struct DonePre {
    facts: TicketFacts,
    auth: Authority,
    local_gate: Option<String>,
    declared: bool,
}

fn precheck_done(ac: &AgentCtx, a: &DoneArgs<'_>, l: &DoneLoad) -> CbResult<DonePre> {
    // In Review, a done of the assignee with a new head is a re-delivery. The same
    // head stays `status-no-change`, before any read.
    let redelivery = l.from == Status::Review;
    let facts = facts(&l.t, &l.me, l.from);
    writer_precheck(
        l.from,
        l.me.actor,
        &facts,
        redelivery && new_head(&l.t, a.sha),
    )?;
    // spec/board#authority: AUTHORITY.md, read now. A dev who may not open PRs cannot deliver
    // evidence.
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    auth.check_open_pr()?;
    // spec/ci#local-run-ci_mode-local: `--local-run` follows `ci_mode`, before any write.
    let local_gate = local_gate_for_done(&auth, a.local_gate)?;
    let declared = ci_none_declared(&l.t.msgs, &l.t.info.body);
    // A PR of a repo outside the initiative is refused before any call to that repo.
    check_pr_in_initiative(&ac.ctx.cfg.repos, &l.pr_url)?;
    Ok(DonePre {
        facts,
        auth,
        local_gate,
        declared,
    })
}

/// On the owner route the owner merges a PR that holds only the patch. A closing
/// keyword in the title or the body of the PR would close the ticket then.
fn check_owner_route_keeps_ticket_open(
    route: &Route,
    pr: &PrFacts,
    ticket_repo: &str,
    issue: u64,
) -> Result<(), Refusal> {
    let closes = matches!(route, Route::Owner { .. })
        && crate::rules::text::closes_ticket(
            &format!("{}\n{}", pr.title, pr.body),
            &pr.repo,
            ticket_repo,
            issue,
        );
    if closes {
        return Err(crate::owner_route::closing_keyword_refusal(issue));
    }
    Ok(())
}

/// What `done` reads about the PR: head, files and the route.
struct DonePr {
    pr: PrFacts,
    base: String,
    adopted: Option<String>,
    files: PrFiles,
    route: Route,
    named: Vec<NamedRun>,
}

fn read_done_pr(ac: &AgentCtx, a: &DoneArgs<'_>, l: &DoneLoad, pre: &DonePre) -> CbResult<DonePr> {
    let named = named_runs(ac, &l.pr_url.repo, a.runs)?;
    let (pr, base) = fetch_pr(ac, &l.pr_url.repo, l.pr_url.number)?;
    // A push to the branch of a merged PR goes nowhere. Before any write.
    merged_pr::check_not_merged(ac, &pr)?;
    pre.auth.check_head_branch(&pr.head_ref)?;
    let adopted = crate::ticket::adopted_branch(&l.t.msgs, &l.pr_url.repo, l.pr_url.number);
    // spec/tickets#workflow-files-the-owner-route: workflow files cannot be pushed by the crew
    // token. Decide the route before any CI call, so a missing patch is named at once.
    // Read one file more than the limit, so that "more than that" is known. A cut list cannot
    // prove that no workflow file or protected path is in the PR: fail closed.
    let files = fetch_pr_files_upto(ac, &pr.repo, pr.number, MAX_FILES + 1)?;
    if files.over(MAX_FILES) {
        return Err(Refusal::new(
            crate::rules::protected::RULE,
            format!(
                "{} has more than {MAX_FILES} files; crewboard cannot prove that none is a workflow file or a protected path, so it cannot choose the route: split the PR into smaller PRs",
                format_args!("{}#{}", l.pr_url.repo, l.pr_url.number)
            ),
        )
        .into());
    }
    let route = crate::owner_route::decide(
        crate::owner_route::declared(&l.t.info.body),
        &files.names,
        l.n,
    )?;
    check_owner_route_keeps_ticket_open(&route, &pr, &l.repo, l.n)?;
    Ok(DonePr {
        pr,
        base,
        adopted,
        files,
        route,
        named,
    })
}

/// What `done` reads about CI and the head of the PR.
struct DoneCi {
    ci: CiFacts,
    no_run: Option<Result<String, String>>,
    /// The head SHA of the PR, read a second time after the CI facts.
    head_after: String,
    /// Commits the PR head is behind its base; `None` when unread (one compare read).
    behind: Option<u64>,
}

fn read_done_ci(ac: &AgentCtx, l: &DoneLoad, pre: &DonePre, p: &DonePr) -> CbResult<DoneCi> {
    let pr = &p.pr;
    let in_initiative = ac
        .ctx
        .cfg
        .repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&pr.repo));
    let ci = if in_initiative {
        let q = CiQuery {
            repo: &pr.repo,
            sha: &pr.head_sha,
            base: &p.base,
            declared: pre.declared,
            named: &p.named,
            ignore: &pre.auth.ci_ignore,
            ..CiQuery::default()
        };
        match fetch_ci(&|p| ac.ctx.client.get(p), &q).map(|(facts, _)| facts) {
            Ok(f) => f,
            // A local gate does not judge CI: a failed read is information, not a refusal.
            Err(e) if pre.local_gate.is_some() => CiFacts::NotQueried(e.to_string()),
            Err(e) => return Err(e),
        }
    } else {
        CiFacts::NoCi(vec![])
    };
    // spec/ci#files-that-every-workflow-ignores: only without any CI run and without `CI: none`.
    let no_run = if in_initiative && pre.local_gate.is_none() {
        judge_no_run_for(
            ac,
            &ci,
            pre.declared,
            pr,
            &pr.head_sha,
            &p.base,
            Some(&p.files),
        )?
    } else {
        None
    };
    let (after, _) = fetch_pr(ac, &l.pr_url.repo, l.pr_url.number)?;
    // A warning only, never a refusal: an unreadable compare is skipped.
    let behind = in_initiative
        .then(|| behind_by(&ac.ctx.client, &pr.repo, &p.base, &pr.head_sha))
        .flatten();
    Ok(DoneCi {
        ci,
        no_run,
        head_after: after.head_sha,
        behind,
    })
}

/// The `owner` route of a `done`: one question to the owner per patch and head.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnerStep {
    /// The question to post; `None`: it was sent for this patch and head already.
    question: Option<String>,
    /// The label `needs-owner` is set after this step.
    label_on: bool,
}

/// What `done` writes and what it answers. Plain data; [`act_done`] performs it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DonePlan {
    /// The body of the `done` message to the coordinator.
    body: String,
    owner: Option<OwnerStep>,
    /// The new value of the board field `Evidence`.
    evidence_field: String,
    /// The Status to write; `None` for a re-delivery in Review.
    status_write: Option<Status>,
    /// `Doing = idle` when the agent has no other ticket In Progress.
    idle_when_free: bool,
    /// The Status that the local cache learns.
    remember: Status,
    /// The answer up to the line that names the `done` message.
    head: String,
    unread_note: String,
    warning: String,
}

/// The facts [`decide_done`] needs. All read already; no network.
struct DoneFacts<'a> {
    ticket_key: String,
    ticket_repo: &'a str,
    issue: u64,
    msgs: &'a [VMsg],
    assignee: Option<&'a str>,
    agent: &'a str,
    actor: Actor,
    from: Status,
    facts: TicketFacts,
    /// The SHA of the latest `done` of the assignee.
    last_done_sha: Option<String>,
    claimed_sha: Option<&'a str>,
    pr: &'a PrFacts,
    route: &'a Route,
    files: &'a PrFiles,
    adopted: Option<&'a str>,
    ci: &'a CiFacts,
    no_run: Option<&'a Result<String, String>>,
    head_after: &'a str,
    /// The base branch of the PR and the commits the head is behind it.
    base: &'a str,
    behind: Option<u64>,
    declared: bool,
    local_gate: Option<&'a str>,
    linked_repos: &'a [String],
    branch_prefix: &'a str,
    authority_path: &'a str,
    /// `protected_paths` of AUTHORITY.md (additive globs).
    protected_paths: &'a [String],
    unread_line: Option<&'a str>,
}

/// Judge the evidence and decide every write of `done`. Pure.
fn decide_done(f: &DoneFacts<'_>) -> Result<DonePlan, Refusal> {
    let to = Status::Review;
    let ev = judge_evidence(&EvidenceInput {
        pr: f.pr,
        claimed_sha: f.claimed_sha,
        head_after: f.head_after,
        ci: f.ci,
        ci_none_declared: f.declared,
        linked_repos: f.linked_repos,
        ticket_repo: f.ticket_repo,
        ticket_issue: f.issue,
        agent: f.agent,
        branch_prefix: f.branch_prefix,
        owner_route: matches!(f.route, Route::Owner { .. }),
        no_run: f.no_run,
        local_gate: f.local_gate,
        // The PR is the one that a coordinator signed as adopted by this ticket.
        adopted_branch: f.adopted,
    })?;
    let msg = EvidenceMsg {
        pr: ev.pr_url.clone(),
        sha: ev.sha.clone(),
        ci: ev.ci.clone(),
        ci_sources: ev.ci_sources.clone(),
        ci_runs: ev.ci_runs.clone(),
        local_gate: ev.local_gate.clone(),
    };
    let field = evidence_field(&msg);
    check_text_field(&field)?;
    let redelivery = f.from == Status::Review;
    let deliver = f.facts.deliver(
        true,
        redelivery && is_new_head(f.last_done_sha.as_deref(), Some(&ev.sha)),
    );
    check_status_transition(Some(f.from), to, f.actor, &deliver)?;
    check_field_write(
        Item::Work,
        Field::Evidence,
        &work_write_ctx_of(f.actor, f.facts.actor_is_assignee),
    )?;
    let ci_info = if f.local_gate.is_some() {
        format!("ci-info: {}\n", ci_info_text(f.ci))
    } else {
        String::new()
    };
    Ok(DonePlan {
        body: done_body(&msg, &done_notes(f, &ci_info)),
        owner: owner_step(f, &msg.sha),
        evidence_field: field,
        status_write: (f.from != to).then_some(to),
        idle_when_free: f.from == Status::InProgress,
        remember: to,
        head: done_head(f, &msg, &ci_info),
        unread_note: crate::unread::note(f.unread_line),
        warning: format!(
            "{}{}",
            ci_warning(f.ci, f.local_gate.is_some()),
            f.behind.map_or(String::new(), |n| done_warning(f.base, n))
        ),
    })
}

/// The lines after the evidence in the `done` message. The parser reads only pr, sha, ci and
/// ci-sources, so they do not change the evidence.
fn done_notes(f: &DoneFacts<'_>, ci_info: &str) -> String {
    let note = match f.route {
        Route::Normal => String::new(),
        Route::Owner { patch } => format!("owner-route: {patch}\n"),
    };
    // A PR that changes a protected path is merged by the owner only. The line lets `tick` leave
    // the ticket without a read (spec/tickets#merge). No new request: the files are read above.
    let hits = crate::rules::protected::protected_hits(
        &f.files.all(),
        crate::rules::protected::Protect::new(f.authority_path, f.protected_paths),
    );
    let note = if hits.is_empty() {
        note
    } else {
        format!("{note}protected-paths: {}\n", hits.join(", "))
    };
    format!(
        "{note}{ci_info}{}",
        f.unread_line.map(|l| format!("{l}\n")).unwrap_or_default()
    )
}

/// ONE message to the owner per patch and head; a retry of `done` sends none. The label follows
/// the question: a retry that sends nothing sets it only while the earlier question is still
/// open (the owner may have answered it).
fn owner_step(f: &DoneFacts<'_>, sha: &str) -> Option<OwnerStep> {
    let Route::Owner { patch } = f.route else {
        return None;
    };
    let question = if crate::owner_route::already_sent(f.msgs, patch, sha) {
        None
    } else {
        Some(crate::owner_route::owner_message(
            f.ticket_repo,
            f.issue,
            &f.pr.head_ref,
            patch,
            sha,
        ))
    };
    let label_on = question.is_some() || question_open(f.msgs, f.assignee).is_some();
    Some(OwnerStep { question, label_on })
}

/// The answer of `done` up to the line that names the `done` message.
fn done_head(f: &DoneFacts<'_>, msg: &EvidenceMsg, ci_info: &str) -> String {
    let to = Status::Review;
    let sources_line = msg
        .ci_sources
        .as_ref()
        .map(|s| format!("ci-sources: {s}\n"))
        .unwrap_or_default()
        + &msg
            .ci_runs
            .as_ref()
            .map(|s| format!("ci-runs: {s}\n"))
            .unwrap_or_default()
        + &msg
            .local_gate
            .as_ref()
            .map(|s| {
                format!(
                    "local-gate: {s}\n{}\n{ci_info}",
                    crate::ticket::LOCAL_GATE_OWNER_LINE
                )
            })
            .unwrap_or_default();
    format!(
        "{} done: Status {}\npr: {}\nsha: {}\nci: {}\n{sources_line}",
        f.ticket_key,
        if f.from == to {
            format!("{} (re-delivery: the Status stays)", to.name())
        } else {
            to.name().to_string()
        },
        msg.pr,
        msg.sha,
        msg.ci,
    )
}

/// The warning line about CI that could not be read fully; none next to a local gate.
fn ci_warning(ci: &CiFacts, local_gate: bool) -> String {
    match ci {
        _ if local_gate => String::new(),
        CiFacts::NotQueried(why) => format!(
            "warning: CI could not be read ({why}); accepted as 'ci: none' because the ticket declares 'CI: none'\n"
        ),
        CiFacts::Green { sources, .. } if !sources.unreadable.is_empty() => format!(
            "warning: CI source(s) unreadable: {}; judged from {}\n",
            sources.unreadable.join(", "),
            sources.seen.join(", ")
        ),
        _ => String::new(),
    }
}

/// Read everything `done` needs and decide: no write.
fn plan_done(ac: &AgentCtx, a: &DoneArgs<'_>, l: &DoneLoad) -> CbResult<DonePlan> {
    let pre = precheck_done(ac, a, l)?;
    let p = read_done_pr(ac, a, l, &pre)?;
    let c = read_done_ci(ac, l, &pre, &p)?;
    let prefix = pre.auth.branch_prefix();
    Ok(decide_done(&DoneFacts {
        ticket_key: l.t.key(),
        ticket_repo: &l.repo,
        issue: l.n,
        msgs: &l.t.msgs,
        assignee: l.t.agent(),
        agent: &l.me.name,
        actor: l.me.actor,
        from: l.from,
        facts: pre.facts,
        last_done_sha: last_done_sha(&l.t),
        claimed_sha: a.sha,
        pr: &p.pr,
        route: &p.route,
        files: &p.files,
        adopted: p.adopted.as_deref(),
        ci: &c.ci,
        no_run: c.no_run.as_ref(),
        head_after: &c.head_after,
        base: &p.base,
        behind: c.behind,
        declared: pre.declared,
        local_gate: pre.local_gate.as_deref(),
        linked_repos: &ac.ctx.cfg.repos,
        branch_prefix: &prefix,
        authority_path: &ac.ctx.cfg.authority,
        protected_paths: &pre.auth.protected_paths,
        unread_line: l.unread_line.as_deref(),
    })?)
}

/// Perform the writes of a [`DonePlan`], in order, and return the answer.
fn act_done(ac: &AgentCtx, l: &DoneLoad, p: &DonePlan, clock: &dyn Clock) -> CbResult<String> {
    let t = &l.t;
    // The guard runs before the first write: a ticket that moved gets no message and no field.
    if p.status_write.is_some()
        && let Ok(from) = t.status()
    {
        super::check_status_unchanged(ac, &t.item_id, &t.key(), from, p.status_write)?;
    }
    let posted = post_with(ac, t, clock, To::Coordinator, Kind::Done, Re::None, &p.body)?;
    let owner_lines = match &p.owner {
        Some(o) => act_owner(ac, t, o, clock)?,
        None => String::new(),
    };
    ac.ctx
        .board
        .set_text(&ac.ctx.client, &t.item_id, "Evidence", &p.evidence_field)?;
    // A re-delivery in Review keeps the Status: no Status write.
    if let Some(to) = p.status_write {
        super::set_select_status(ac, &t.item_id, to)?;
    }
    // `done` on a split Blocked ticket is not the work in hand. The
    // agent may work on another ticket; its `Doing` stays. The agent State is not written.
    if p.idle_when_free {
        write_idle_when_free(ac, &l.world, t)?;
    }
    remember(ac, t, p.remember, t.agent());
    Ok(format!(
        "{}done message {} ({})\n{}{owner_lines}{}",
        p.head, posted.id, posted.url, p.unread_note, p.warning
    ))
}

/// The question to the owner (once) and the label `needs-owner`; the lines for the answer.
fn act_owner(ac: &AgentCtx, t: &Ticket, o: &OwnerStep, clock: &dyn Clock) -> CbResult<String> {
    let mut lines = String::new();
    match &o.question {
        None => lines.push_str("owner message sent already; not sent again\n"),
        Some(q) => {
            let sent = post_with(ac, t, clock, To::Owner, Kind::Question, Re::None, q)?;
            lines.push_str(&format!("owner message {} ({})\n", sent.id, sent.url));
        }
    }
    if o.label_on {
        lines.push_str(&needs_owner_lines(ac, &t.repo, t.issue, true));
    }
    Ok(lines)
}

/// `done <issue> --pr <url> [--sha <sha>]` (dev): load, plan, act.
pub fn done(ac: &AgentCtx, a: DoneArgs<'_>) -> CbResult<String> {
    let l = load_done(ac, &a)?;
    let plan = plan_done(ac, &a, &l)?;
    act_done(ac, &l, &plan, &SystemClock)
}

/// Is `sha` a new head for a `done` of the ticket's agent in Review? True when the
/// agent has no `done` yet or its latest `done` names another SHA. `None` (no `--sha`) counts
/// as new until the evidence names the head.
fn new_head(t: &Ticket, sha: Option<&str>) -> bool {
    is_new_head(last_done_sha(t).as_deref(), sha)
}

/// The SHA of the latest `done` of the ticket's agent.
fn last_done_sha(t: &Ticket) -> Option<String> {
    t.agent()
        .and_then(|a| latest_done(&t.msgs, a))
        .map(|(_, ev)| ev.sha)
}

/// [`new_head`] on the facts: `last` is the SHA of the latest `done`.
fn is_new_head(last: Option<&str>, sha: Option<&str>) -> bool {
    match (last, sha) {
        (Some(last), Some(sha)) => !last.eq_ignore_ascii_case(sha),
        _ => true,
    }
}

/// The writer and state check that needs no network. The evidence counts as complete, so the
/// check passes everything except the writer and the table, and a wrong writer is named first.
fn writer_precheck(
    from: Status,
    actor: Actor,
    facts: &TicketFacts,
    new_head: bool,
) -> CbResult<()> {
    let optimistic = facts.deliver(true, new_head);
    check_status_transition(Some(from), Status::Review, actor, &optimistic)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Role;

    #[test]
    fn the_ci_info_text_names_the_state_on_one_short_line() {
        assert_eq!(
            ci_info_text(&CiFacts::NotGreen(vec!["a\nb".into()])),
            "not green (information only): a b"
        );
        assert!(ci_info_text(&CiFacts::NoCi(vec![])).starts_with("no CI run"));
        assert!(ci_info_text(&CiFacts::NotQueried("HTTP 500".into())).contains("HTTP 500"));
        let long = ci_info_text(&CiFacts::NotGreen(vec!["é".repeat(500)]));
        assert!(
            long.ends_with("...") && long.chars().count() < 340,
            "{long}"
        );
    }

    #[test]
    fn the_writer_precheck_names_the_wrong_writer_before_any_fact() {
        let base = TicketFacts::default();
        let dev = Actor::Agent(Role::Dev);
        let coord = Actor::Agent(Role::Coordinator);
        // the coordinator cannot deliver work
        assert!(
            writer_precheck(Status::InProgress, coord, &base, false)
                .unwrap_err()
                .to_string()
                .contains("status-wrong-writer")
        );
        // a dev who is not the assignee cannot deliver, the assignee can
        let assignee = TicketFacts {
            actor_is_assignee: true,
            ..base
        };
        assert!(writer_precheck(Status::InProgress, dev, &assignee, false).is_ok());
        assert!(writer_precheck(Status::InProgress, dev, &base, false).is_err());
    }

    // ---- plan tests: the decisions of `done` without a fake GitHub ----

    use crate::rules::evidence::CiSources;
    use crate::ticket::Sender;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn pr_facts() -> PrFacts {
        PrFacts {
            repo: "a/b".into(),
            number: 7,
            state: "open".into(),
            merged: false,
            head_sha: SHA.into(),
            head_ref: "crew/hermes@b/12-fix-it".into(),
            title: "Work".into(),
            body: "Does it.\n\nCloses #12\n".into(),
        }
    }

    fn green() -> CiFacts {
        CiFacts::Green {
            run_url: "https://github.com/a/b/actions/runs/1".into(),
            sources: CiSources::default(),
        }
    }

    fn dev_ctx() -> TicketFacts {
        TicketFacts {
            actor_is_assignee: true,
            ..TicketFacts::default()
        }
    }

    fn files(names: &[&str]) -> PrFiles {
        PrFiles {
            names: names.iter().map(|s| s.to_string()).collect(),
            previous: vec![],
            truncated: false,
        }
    }

    /// The facts of a fresh delivery from In Progress; a test changes what it needs.
    struct Fix {
        pr: PrFacts,
        route: Route,
        files: PrFiles,
        ci: CiFacts,
        repos: Vec<String>,
        msgs: Vec<VMsg>,
    }

    fn fix() -> Fix {
        Fix {
            pr: pr_facts(),
            route: Route::Normal,
            files: files(&["src/lib.rs"]),
            ci: green(),
            repos: vec!["a/b".into()],
            msgs: vec![],
        }
    }

    fn facts_of(x: &Fix, from: Status) -> DoneFacts<'_> {
        DoneFacts {
            ticket_key: "a/b#12".into(),
            ticket_repo: "a/b",
            issue: 12,
            msgs: &x.msgs,
            assignee: Some("hermes@b"),
            agent: "hermes@b",
            actor: Actor::Agent(Role::Dev),
            from,
            facts: dev_ctx(),
            last_done_sha: None,
            claimed_sha: Some(SHA),
            pr: &x.pr,
            route: &x.route,
            files: &x.files,
            adopted: None,
            ci: &x.ci,
            no_run: None,
            head_after: SHA,
            base: "main",
            behind: None,
            declared: false,
            local_gate: None,
            linked_repos: &x.repos,
            branch_prefix: "crew/",
            authority_path: "AUTHORITY.md",
            protected_paths: &[],
            unread_line: None,
        }
    }

    fn rule_of(r: Result<DonePlan, Refusal>) -> &'static str {
        r.expect_err("must be refused").rule
    }

    fn question(body: &str, to: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: 1,
            created: 0,
            sender: Sender::Agent {
                name: "hermes@b".into(),
                role: Role::Dev,
            },
            to: to.into(),
            kind: Some(Kind::Question),
            re: Re::None,
            body: body.into(),
        }
    }

    #[test]
    fn a_fresh_delivery_plans_the_message_the_evidence_and_the_move_to_review() {
        let x = fix();
        let p = decide_done(&facts_of(&x, Status::InProgress)).unwrap();
        assert_eq!(p.status_write, Some(Status::Review));
        assert!(p.idle_when_free);
        assert_eq!(p.remember, Status::Review);
        assert!(p.owner.is_none());
        assert!(p.body.starts_with(&format!(
            "Done. Evidence follows.\npr: https://github.com/a/b/pull/7\nsha: {SHA}\nci: "
        )));
        assert!(p.evidence_field.contains(SHA));
        assert!(p.head.starts_with("a/b#12 done: Status Review\npr: "));
        assert_eq!(p.unread_note, "");
        assert_eq!(p.warning, "");
    }

    #[test]
    fn a_head_behind_the_base_adds_one_warning_line_and_no_refusal() {
        let x = fix();
        let mut f = facts_of(&x, Status::InProgress);
        assert_eq!(decide_done(&f).unwrap().warning, "");
        f.behind = Some(0);
        assert_eq!(decide_done(&f).unwrap().warning, "");
        f.behind = Some(3);
        let p = decide_done(&f).unwrap();
        assert_eq!(
            p.warning,
            "warning: your branch is 3 commits behind main; merge main and run done again if CI should test the latest code\n"
        );
        // the evidence and the status write are the same as without the warning
        f.behind = None;
        let q = decide_done(&f).unwrap();
        assert_eq!((&p.body, p.status_write), (&q.body, q.status_write));
    }

    #[test]
    fn a_split_ticket_in_blocked_delivers_without_going_idle() {
        let x = fix();
        let mut f = facts_of(&x, Status::Blocked);
        f.facts.split_off_proven = true;
        let p = decide_done(&f).unwrap();
        assert!(!p.idle_when_free && p.status_write == Some(Status::Review));
        // without the split proof the table refuses
        assert_eq!(
            rule_of(decide_done(&facts_of(&x, Status::Blocked))),
            "status-unknown-transition"
        );
    }

    #[test]
    fn the_same_head_in_review_is_status_no_change_and_a_new_head_is_a_redelivery() {
        let mut x = fix();
        x.pr.head_sha = OTHER_SHA.into();
        let mut f = facts_of(&x, Status::Review);
        f.claimed_sha = Some(OTHER_SHA);
        f.head_after = OTHER_SHA;
        f.last_done_sha = Some(OTHER_SHA.into());
        assert_eq!(rule_of(decide_done(&f)), "status-no-change");
        // another head: the Status stays, and the answer says so
        let mut f = facts_of(&x, Status::Review);
        f.claimed_sha = Some(OTHER_SHA);
        f.head_after = OTHER_SHA;
        f.last_done_sha = Some(SHA.into());
        let p = decide_done(&f).unwrap();
        assert_eq!(p.status_write, None);
        assert!(
            p.head
                .contains("Status Review (re-delivery: the Status stays)")
        );
    }

    #[test]
    fn a_wrong_writer_or_a_moved_head_or_red_ci_is_refused_in_the_plan() {
        let x = fix();
        let mut f = facts_of(&x, Status::InProgress);
        f.actor = Actor::Agent(Role::Coordinator);
        assert_eq!(rule_of(decide_done(&f)), "status-wrong-writer");
        let mut f = facts_of(&x, Status::InProgress);
        f.head_after = OTHER_SHA;
        assert_eq!(rule_of(decide_done(&f)), "evidence-head-moved");
        let mut x = fix();
        x.ci = CiFacts::NotGreen(vec!["red".into()]);
        assert_eq!(
            rule_of(decide_done(&facts_of(&x, Status::InProgress))),
            "evidence-ci-not-green"
        );
    }

    #[test]
    fn a_pr_outside_the_initiative_is_refused_by_name() {
        let pr = PrUrl {
            repo: "other/repo".into(),
            number: 3,
        };
        let r = check_pr_in_initiative(&["a/b".to_string(), "a/c".to_string()], &pr).unwrap_err();
        assert_eq!(r.rule, "evidence-pr-foreign-repo");
        assert!(r.message.contains("other/repo#3") && r.message.contains("a/b, a/c"));
        // the repo name has no case
        let pr = PrUrl {
            repo: "A/B".into(),
            number: 3,
        };
        assert!(check_pr_in_initiative(&["a/b".to_string()], &pr).is_ok());
    }

    #[test]
    fn a_closing_keyword_on_the_owner_route_is_refused_and_not_on_the_normal_route() {
        let owner = Route::Owner {
            patch: ".crewboard/patches/12.patch".into(),
        };
        let pr = pr_facts();
        let r = check_owner_route_keeps_ticket_open(&owner, &pr, "a/b", 12).unwrap_err();
        assert_eq!(r.rule, crate::owner_route::KEYWORD_RULE);
        assert!(check_owner_route_keeps_ticket_open(&Route::Normal, &pr, "a/b", 12).is_ok());
        let refs = PrFacts {
            body: "Refs #12".into(),
            ..pr
        };
        assert!(check_owner_route_keeps_ticket_open(&owner, &refs, "a/b", 12).is_ok());
    }

    #[test]
    fn the_owner_route_plans_one_question_per_patch_and_head_and_a_note_line() {
        let mut x = fix();
        x.route = Route::Owner {
            patch: ".crewboard/patches/12.patch".into(),
        };
        let p = decide_done(&facts_of(&x, Status::InProgress)).unwrap();
        let o = p.owner.expect("an owner step");
        let q = o.question.expect("a question");
        assert!(q.starts_with(&format!(
            "owner-route: .crewboard/patches/12.patch sha={SHA}"
        )));
        assert!(o.label_on);
        assert!(
            p.body
                .contains("owner-route: .crewboard/patches/12.patch\n")
        );
        // the question was sent for this head: none again, and the label only while it is open
        x.msgs = vec![question(&q, "owner")];
        let p = decide_done(&facts_of(&x, Status::InProgress)).unwrap();
        let o = p.owner.unwrap();
        assert_eq!((o.question, o.label_on), (None, true));
        x.msgs.push(VMsg {
            ts: String::new(),
            sender: Sender::Owner,
            kind: None,
            ..question("done it", "hermes@b")
        });
        let p = decide_done(&facts_of(&x, Status::InProgress)).unwrap();
        let o = p.owner.unwrap();
        assert_eq!((o.question, o.label_on), (None, false));
    }

    #[test]
    fn protected_paths_and_unread_lines_go_into_the_message_after_the_evidence() {
        let mut x = fix();
        x.files = files(&["src/lib.rs", "AUTHORITY.md"]);
        let mut f = facts_of(&x, Status::InProgress);
        f.unread_line = Some("1 unread message(s)");
        let p = decide_done(&f).unwrap();
        assert!(p.body.contains("protected-paths: AUTHORITY.md\n"));
        assert!(p.body.ends_with("1 unread message(s)\n"));
        assert_eq!(
            p.unread_note,
            "note: sent with --override-unread; 1 unread message(s)\n"
        );
    }

    #[test]
    fn a_local_gate_names_the_ci_as_information_and_gives_no_ci_warning() {
        let mut x = fix();
        x.ci = CiFacts::NotQueried("HTTP 500".into());
        let mut f = facts_of(&x, Status::InProgress);
        f.local_gate = Some("host-a: make test rc=0, log /tmp/g.log");
        let p = decide_done(&f).unwrap();
        assert!(
            p.body
                .contains("ci-info: not readable (information only): HTTP 500\n")
        );
        assert!(p.head.contains("local-gate: host-a: make test rc=0"));
        assert_eq!(p.warning, "");
        // without a local gate, an unreadable CI under `CI: none` is a warning line
        let mut f = facts_of(&x, Status::InProgress);
        f.declared = true;
        let p = decide_done(&f).unwrap();
        assert!(
            p.warning
                .starts_with("warning: CI could not be read (HTTP 500)")
        );
    }
}
