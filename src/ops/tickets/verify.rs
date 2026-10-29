//! `verify`: the coordinator checks the evidence of a delivered ticket.
//!
//! Three steps: [`load_verify`] reads the ticket, [`plan_verify`] reads the PR and the CI and
//! decides ([`verify_failures`] and [`decide_verify`] are the pure parts, a [`VerifyPlan`] their
//! result), [`act_verify`] performs the writes of the plan in order.

use super::in_progress::in_progress_key;
use super::local_gate::local_gate_for_verify;
use super::order;
use super::pr_evidence::{evidence_runs, fetch_pr, judge_no_run_for, pr_files_for_protection};
use super::{
    Me, check_verify_ticket, load_world_cached, me_of, needs_owner_lines, post_with, remember,
    to_agent, work_write_ctx_of,
};
use crate::authority::Authority;
use crate::clock::{Clock, SystemClock};
use crate::domain::ci::evaluate::CiRuns;
use crate::error::CbResult;
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::{Kind, Re};
use crate::rules::Actor;
use crate::rules::Refusal;
use crate::rules::evidence::{CiFacts, PrFacts, PrUrl, parse_pr_url};
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::protected::PrFileList;
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::{
    EvidenceMsg, ci_none_declared, is_local_gate, latest_done, question_open, verify_body,
};

/// `verify <issue> [--body ...]` (coordinator): spec/tickets#verification-by-the-coordinator steps
/// 1 and 2; the gate itself (step 3) is the model's work, and `--body` records what it found.
/// `verify --fail` needs `--body`, the finding of the coordinator's own Gate
/// check. Pure; the CLI runs it before any read.
pub fn check_verify_fail_note(gate_failed: bool, note: &str) -> Result<(), Refusal> {
    if gate_failed && note.trim().is_empty() {
        return Err(Refusal::new(
            "verify-fail-needs-note",
            "verify --fail needs --body \"<what the Gate check found>\": the dev reads it as the failure",
        ));
    }
    Ok(())
}

/// What `verify` has loaded before it decides: the agent and the ticket.
struct VerifyLoad {
    n: u64,
    world: World,
    me: Me,
    t: Ticket,
    from: Status,
}

/// The reads of `verify` that come before any decision about the evidence.
fn load_verify(ac: &AgentCtx, issue_arg: &str) -> CbResult<VerifyLoad> {
    let (repo, n) = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    t.check_agent_signed()?;
    let from = t.status()?;
    Ok(VerifyLoad {
        n,
        world,
        me,
        t,
        from,
    })
}

/// The arguments of `verify` that the checks need.
struct VerifyArgs<'a> {
    note: &'a str,
    runs: &'a [String],
    gate_failed: bool,
    local_gate: Option<&'a str>,
}

/// The checks that need no PR: the ticket, the evidence of the `done`, `--local-run`.
struct VerifyPre {
    assignee: String,
    /// The comment of the `done` that this verify answers.
    done_comment: u64,
    ev: EvidenceMsg,
    coordinator_gate: Option<String>,
    ev_local: bool,
    /// The evidence is a local gate and `ci_mode` is `local`: no CI is judged.
    local_verify: bool,
    auth: Authority,
    pr_url: PrUrl,
    named: CiRuns,
}

/// The latest `done` of the assignee, or the refusal `verify-no-evidence`. Pure.
fn latest_evidence<'a>(
    key: &str,
    assignee: Option<&str>,
    msgs: &'a [crate::ticket::VMsg],
) -> Result<(String, &'a crate::ticket::VMsg, EvidenceMsg), Refusal> {
    let assignee = assignee
        .ok_or_else(|| Refusal::new("verify-no-evidence", format!("{key} has no assignee")))?;
    let (done_msg, ev) = latest_done(msgs, assignee).ok_or_else(|| {
        Refusal::new(
            "verify-no-evidence",
            format!("{key} has no signed done message of {assignee} with pr, sha and ci"),
        )
    })?;
    Ok((assignee.to_string(), done_msg, ev))
}

fn precheck_verify(ac: &AgentCtx, l: &VerifyLoad, va: &VerifyArgs<'_>) -> CbResult<VerifyPre> {
    let t = &l.t;
    check_verify_ticket(ac, t, l.from, &l.me, va.gate_failed)?;
    let (assignee, done_msg, ev) = latest_evidence(&t.key(), t.agent(), &t.msgs)?;
    // spec/ci#local-run-ci_mode-local: `ci_mode` from AUTHORITY.md on the default branch (never
    // of the PR branch). `--local-run` is checked before any write.
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    let ev_local = is_local_gate(&ev);
    let coordinator_gate = local_gate_for_verify(&auth, va.local_gate, ev_local, va.gate_failed)?;
    let local_verify = ev_local && auth.ci_mode.is_local();
    let mut pr_url = parse_pr_url(&ev.pr)?;
    super::pr_evidence::follow_former_name(
        &mut pr_url,
        &done_msg.ts,
        ac.ctx.main_repo(),
        &auth.former_repos,
    );
    let named = if local_verify {
        CiRuns::new(Vec::new(), Vec::new())
    } else {
        evidence_runs(ac, &pr_url.repo, &ev, va.runs)?
    };
    Ok(VerifyPre {
        assignee,
        done_comment: done_msg.comment_id,
        ev,
        coordinator_gate,
        ev_local,
        local_verify,
        auth,
        pr_url,
        named,
    })
}

/// Rule `protected-path-owner-merge`: a verify PASS is refused for a PR that changes a protected
/// path. `files` is `None` for `--fail`, which stays allowed. Pure.
fn check_pass_allowed(
    files: Option<&PrFileList>,
    pr_url: &str,
    key: &str,
    guard: crate::rules::protected::Protect<'_>,
) -> Result<(), Refusal> {
    match files {
        Some(f) => crate::rules::protected::check_unprotected(pr_url, key, f, guard),
        None => Ok(()),
    }
}

/// What `verify` reads about CI (not for a local gate).
struct VerifyCi {
    facts: CiFacts,
    findings: Vec<Refusal>,
    no_run: Option<Result<String, String>>,
}

/// What `verify` reads about the PR and its CI.
struct VerifyRead {
    pr: PrFacts,
    ci: Option<VerifyCi>,
}

fn read_verify(
    ac: &AgentCtx,
    l: &VerifyLoad,
    pre: &VerifyPre,
    va: &VerifyArgs<'_>,
) -> CbResult<VerifyRead> {
    let ev = &pre.ev;
    let (pr, base) = fetch_pr(ac, &pre.pr_url.repo, pre.pr_url.number)?;
    // Rule `protected-path-owner-merge`: a PR that changes AUTHORITY.md, .crewboard.toml or a
    // workflow is for the owner: a verify PASS is refused (and nothing is posted). `--fail` stays
    // allowed: the coordinator may still judge the work wrong and send the ticket back. Before
    // the CI read.
    let files = if va.gate_failed {
        None
    } else {
        Some(pr_files_for_protection(
            ac,
            &pr.repo,
            pr.number,
            &l.t.key(),
        )?)
    };
    let guard =
        crate::rules::protected::Protect::new(&ac.ctx.cfg.authority, &pre.auth.protected_paths);
    check_pass_allowed(files.as_ref(), &ev.pr, &l.t.key(), guard)?;
    // A local gate has no CI to judge (its CI is information only, and not read here).
    if pre.local_verify {
        return Ok(VerifyRead { pr, ci: None });
    }
    // CI of the head that the evidence names (a moved head is already a failure).
    let declared = ci_none_declared(&l.t.msgs, &l.t.info.body);
    let ci_none = ev.ci == "none";
    // The named runs of the evidence are judged again, like `done` judged them.
    // spec/ci#jobs-that-ci_ignore-skips: `ci_ignore` of AUTHORITY.md on the default branch,
    // never of the PR branch.
    let q = CiQuery {
        repo: &pr.repo,
        sha: &ev.sha,
        base: &base,
        declared: ci_none && declared,
        named: &pre.named.extra,
        evidence: &pre.named.evidence,
        ignore: &pre.auth.ci_ignore,
    };
    let (facts, findings) = fetch_ci(&|p| ac.ctx.client.get(p), &q)?;
    // spec/ci#files-that-every-workflow-ignores: the same judgement as `done`, for the head of
    // the evidence. Reads only when no CI run exists.
    let no_run = judge_no_run_for(ac, &facts, ci_none && declared, &pr, &ev.sha, &base, None)?;
    Ok(VerifyRead {
        pr,
        ci: Some(VerifyCi {
            facts,
            findings,
            no_run,
        }),
    })
}

/// The facts [`verify_failures`] and [`decide_verify`] need. All read already; no network.
struct VerifyFacts<'a> {
    ticket_key: String,
    assignee: &'a str,
    done_comment: u64,
    ev: &'a EvidenceMsg,
    coordinator_gate: Option<&'a str>,
    ev_local: bool,
    local_verify: bool,
    declared: bool,
    pr: &'a PrFacts,
    ci: Option<&'a VerifyCi>,
    linked_repos: &'a [String],
    gate_failed: bool,
    note: &'a str,
    from: Status,
    actor: Actor,
    /// The ticket has the label `needs-owner`.
    labelled_needs_owner: bool,
    /// A question to the owner is still open on the ticket.
    question_open: bool,
}

/// The failures of the evidence and the fresh CI text that a pass keeps.
#[derive(Debug, Default, PartialEq, Eq)]
struct VerifyFindings {
    failures: Vec<String>,
    fresh_ci: Option<String>,
    fresh_sources: Option<String>,
    fresh_runs: Option<String>,
}

/// Judge the evidence of the `done` against the PR and the CI now. Pure. `prefix` gives the
/// dev branch prefix for the hint of a missing CI run, and is called only for that hint.
fn verify_failures(f: &VerifyFacts<'_>, prefix: &dyn Fn() -> String) -> VerifyFindings {
    let (ev, pr) = (f.ev, f.pr);
    let mut out = VerifyFindings::default();
    let failures = &mut out.failures;
    if !f
        .linked_repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&pr.repo))
    {
        failures.push(format!(
            "PR {}#{} is not in a repo of the initiative",
            pr.repo, pr.number
        ));
    }
    if pr.state == "closed" && !pr.merged {
        failures.push("the PR is closed without a merge".into());
    }
    if !pr.head_sha.eq_ignore_ascii_case(&ev.sha) {
        failures.push(format!(
            "PR head moved: the evidence says {}, the PR head is {} now",
            ev.sha, pr.head_sha
        ));
    }
    let ci_none = ev.ci == "none";
    if ci_none && !f.declared {
        failures.push(
            "the evidence says 'ci: none' but the ticket body does not declare 'CI: none'".into(),
        );
    }
    if f.ev_local
        && !ev
            .local_gate
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        failures.push("the done message has no local-gate line [local-gate-missing]".into());
    }
    if f.ev_local && !f.local_verify {
        failures.push(
            "the evidence is a local gate, but AUTHORITY.md ci_mode is ci: it needs CI. The dev agent runs `crewboard done` again with a CI run of the head [local-gate-not-enabled]".into(),
        );
    }
    if let Some(ci) = f.ci.filter(|_| !f.local_verify) {
        ci_failures(f, ci, prefix, &mut out);
    }
    if f.gate_failed {
        out.failures
            .push(format!("Gate check by the coordinator: {}", f.note.trim()));
    }
    out
}

/// The failures that the CI facts of the evidence head give, and the fresh CI text of a pass.
fn ci_failures(
    f: &VerifyFacts<'_>,
    ci: &VerifyCi,
    prefix: &dyn Fn() -> String,
    out: &mut VerifyFindings,
) {
    let ev = f.ev;
    let ci_none_declared = ev.ci == "none" && f.declared;
    for r in &ci.findings {
        out.failures.push(format!("{} [{}]", r.message, r.rule));
    }
    match &ci.facts {
        CiFacts::Green { sources, .. } => {
            out.fresh_runs = sources.runs_text();
            // A newer green run is fine; the CI is green for the SHA. The sources that were
            // read now go into the result (a token may have changed since `done`).
            out.fresh_sources = Some(sources.text());
        }
        CiFacts::NoCi(_) | CiFacts::OnlyInformation { .. } if ci_none_declared => {}
        CiFacts::NotQueried(_) if ci_none_declared => {}
        CiFacts::OnlyInformation { info, .. } => out.failures.push(format!(
            "no counted CI run for {}: only runs that are information (not push or pull_request): {}. Name a run that is part of the Gate with `--run <id>`. {}",
            ev.sha,
            info.join(", "),
            crate::rules::evidence::no_ci_cause(&prefix())
        )),
        CiFacts::NoCi(_) => match &ci.no_run {
            Some(Ok(text)) => out.fresh_ci = Some(text.clone()),
            other => out.failures.push(format!(
                "no CI run for {}.{} {}",
                ev.sha,
                match other {
                    Some(Err(why)) => format!(" Not accepted as paths-ignored: {why}."),
                    _ => String::new(),
                },
                crate::rules::evidence::no_ci_cause(&prefix())
            )),
        },
        CiFacts::NotGreen(r) => out
            .failures
            .push(format!("CI of {} is not green: {}", ev.sha, r.join("; "))),
        CiFacts::NotQueried(w) => out.failures.push(format!(
            "CI cannot be read ({w}) and the ticket does not declare 'CI: none' [evidence-ci-unreadable]"
        )),
    }
}

/// The move of a failed verify: back to In Progress, or to Ready (first in the
/// queue) when the agent works on another ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FailMove {
    to: Status,
    /// The `Order` to write, for the move to Ready.
    order: Option<i64>,
    /// The line of the answer.
    line: String,
}

/// What `verify` writes and what it answers. Plain data; [`act_verify`] performs it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifyPlan {
    pass: bool,
    /// The agent that gets the report.
    assignee: String,
    /// The body of the signed report to the assignee.
    body: String,
    /// The `done` message that the report answers.
    done_comment: u64,
    /// The first line of the answer, before the line that names the report.
    first_line: String,
    /// The lines after the report line: the coordinator gate, the failures, the next step.
    rest: String,
    /// Only for a failed verify: the move of the ticket.
    fail_move: Option<FailMove>,
    /// The label `needs-owner` goes (a failed verify only).
    clear_needs_owner: bool,
}

/// Decide the report and the writes of `verify` from the findings. Pure. `busy` is the other
/// ticket of the assignee that is In Progress and the `Order` that puts this ticket first in
/// the queue; read only for a failed verify.
fn decide_verify(
    f: &VerifyFacts<'_>,
    findings: VerifyFindings,
    busy: Option<(String, i64)>,
) -> Result<VerifyPlan, Refusal> {
    let pass = findings.failures.is_empty();
    let ev_now = EvidenceMsg {
        ci: findings.fresh_ci.unwrap_or_else(|| f.ev.ci.clone()),
        ci_sources: findings.fresh_sources.or_else(|| f.ev.ci_sources.clone()),
        ci_runs: findings.fresh_runs.or_else(|| f.ev.ci_runs.clone()),
        ..f.ev.clone()
    };
    let case = busy.as_ref().map(|(other, order)| {
        format!(
            "verify failed; {} works on {other}; queued as next (Ready, Order {order})",
            f.assignee
        )
    });
    let body = verify_body(
        pass,
        &ev_now,
        &f.pr.head_sha,
        &findings.failures,
        // a failed Gate check carries the note in its `failed:` line already
        if f.gate_failed { "" } else { f.note },
        case.as_deref(),
        f.coordinator_gate,
    );
    let mut rest = String::new();
    if let Some(g) = f.coordinator_gate {
        rest.push_str(&format!("local-gate (coordinator): {g}\n"));
    }
    for fail in &findings.failures {
        rest.push_str(&format!("failed: {fail}\n"));
    }
    let fail_move = if pass {
        rest.push_str(&format!(
            "next: crewboard close {} --verified (after you checked the Gate)\n",
            f.ticket_key
        ));
        None
    } else {
        Some(fail_move(f, busy.as_ref())?)
    };
    Ok(VerifyPlan {
        pass,
        assignee: f.assignee.to_string(),
        body,
        done_comment: f.done_comment,
        first_line: format!(
            "verify {} for {}: {}\n",
            if pass { "PASS" } else { "FAIL" },
            f.ticket_key,
            if pass {
                "evidence still holds"
            } else {
                "evidence does not hold"
            }
        ),
        rest,
        fail_move,
        clear_needs_owner: !pass && f.labelled_needs_owner && !f.question_open,
    })
}

/// The move after a failed verify, checked against the status table and the field rules.
fn fail_move(f: &VerifyFacts<'_>, busy: Option<&(String, i64)>) -> Result<FailMove, Refusal> {
    let ctx = Transition::AfterVerify {
        verify: Some(false),
        assignee_busy: busy.is_some(),
    };
    match busy {
        Some((other, order)) => {
            // the agent works on `other`: Ready for the same agent, first in its queue
            check_field_write(Item::Work, Field::Order, &work_write_ctx_of(f.actor, false))?;
            check_status_transition(Some(f.from), Status::Ready, f.actor, &ctx)?;
            Ok(FailMove {
                to: Status::Ready,
                order: Some(*order),
                line: format!(
                    "Status moved to Ready, Order {order} (first in the queue of {}): it works on {other}; `take` follows after `done`\n",
                    f.assignee
                ),
            })
        }
        None => {
            check_status_transition(Some(f.from), Status::InProgress, f.actor, &ctx)?;
            Ok(FailMove {
                to: Status::InProgress,
                order: None,
                line: "Status moved back to In Progress\n".into(),
            })
        }
    }
}

/// Read everything `verify` needs and decide: no write.
fn plan_verify(ac: &AgentCtx, l: &VerifyLoad, va: &VerifyArgs<'_>) -> CbResult<VerifyPlan> {
    let t = &l.t;
    let pre = precheck_verify(ac, l, va)?;
    let read = read_verify(ac, l, &pre, va)?;
    let facts = VerifyFacts {
        ticket_key: t.key(),
        assignee: &pre.assignee,
        done_comment: pre.done_comment,
        ev: &pre.ev,
        coordinator_gate: pre.coordinator_gate.as_deref(),
        ev_local: pre.ev_local,
        local_verify: pre.local_verify,
        declared: ci_none_declared(&t.msgs, &t.info.body),
        pr: &read.pr,
        ci: read.ci.as_ref(),
        linked_repos: &ac.ctx.cfg.repos,
        gate_failed: va.gate_failed,
        note: va.note,
        from: l.from,
        actor: l.me.actor,
        labelled_needs_owner: t
            .info
            .labels
            .iter()
            .any(|x| x == crate::labels::NEEDS_OWNER),
        question_open: question_open(&t.msgs, t.agent()).is_some(),
    };
    let findings = verify_failures(&facts, &|| {
        crate::ctx::load_authority_quiet(&ac.ctx)
            .unwrap_or_default()
            .branch_prefix()
    });
    // A failed verify needs to know if the agent works on another ticket. This is
    // the ONE added read: the board rows of the assignee's tickets (`nodes(ids:)`), only on a fail.
    let busy = if findings.failures.is_empty() {
        None
    } else {
        let rows = crate::ops::lifecycle::rows_of(ac, &l.world, &pre.assignee)?;
        in_progress_key(&rows, &pre.assignee, &t.item_id)
            .map(|other| (other, order::queue_front_order(&rows, &pre.assignee)))
    };
    Ok(decide_verify(&facts, findings, busy)?)
}

/// Perform the writes of a [`VerifyPlan`], in order, and return the answer.
fn act_verify(
    ac: &mut AgentCtx,
    l: &VerifyLoad,
    p: &VerifyPlan,
    clock: &dyn Clock,
) -> CbResult<String> {
    let t = &l.t;
    // The guard runs before the first write: a ticket that moved gets no result message.
    if let Some(m) = &p.fail_move
        && let Ok(from) = t.status()
    {
        super::check_status_unchanged(ac, &t.item_id, &t.key(), from, Some(m.to))?;
    }
    let posted = post_with(
        ac,
        t,
        clock,
        to_agent(&p.assignee)?,
        Kind::Report,
        Re::Comment {
            issue: l.n,
            comment: p.done_comment,
        },
        &p.body,
    )?;
    let mut out = format!(
        "{}result message {} ({})\n{}",
        p.first_line, posted.id, posted.url, p.rest
    );
    if let Some(m) = &p.fail_move {
        super::set_select_status(ac, &t.item_id, m.to)?;
        if let Some(order) = m.order {
            order::write_order(ac, &t.item_id, order)?;
        }
        remember(ac, t, m.to, t.agent());
        out.push_str(&m.line);
        if p.clear_needs_owner {
            out.push_str(&needs_owner_lines(ac, &t.repo, t.issue, false));
        }
    }
    Ok(out)
}

/// `verify <issue> [--body ...] [--run <id>] [--fail]` (coordinator). `gate_failed` (`--fail`): the coordinator's own Gate check failed. The note is then a failure, so the
/// verify fails with the writes of any failed verify, also when the evidence holds.
pub fn verify(
    ac: &mut AgentCtx,
    issue_arg: &str,
    note: &str,
    runs: &[String],
    gate_failed: bool,
    local_gate: Option<&str>,
) -> CbResult<String> {
    let l = load_verify(ac, issue_arg)?;
    let va = VerifyArgs {
        note,
        runs,
        gate_failed,
        local_gate,
    };
    let plan = plan_verify(ac, &l, &va)?;
    act_verify(ac, &l, &plan, &SystemClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_fail_needs_a_note_and_plain_verify_needs_none() {
        // refusals: --fail without a note, or with only blanks
        for note in ["", "   "] {
            assert_eq!(
                check_verify_fail_note(true, note).unwrap_err().rule,
                "verify-fail-needs-note"
            );
        }
        // pass: --fail with a note; a plain verify with or without a note
        assert!(check_verify_fail_note(true, "Gate part 2 is missing").is_ok());
        assert!(check_verify_fail_note(false, "").is_ok());
        assert!(check_verify_fail_note(false, "reran the tests").is_ok());
    }

    // ---- plan tests: the decisions of `verify` without a fake GitHub ----

    use crate::protocol::names::Re as ReName;
    use crate::rules::Role;
    use crate::rules::evidence::CiSources;
    use crate::ticket::{Sender, VMsg};

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
    const PR_URL: &str = "https://github.com/a/b/pull/7";
    const COORD: Actor = Actor::Agent(Role::Coordinator);

    fn ev() -> EvidenceMsg {
        EvidenceMsg {
            pr: PR_URL.into(),
            sha: SHA.into(),
            ci: "https://github.com/a/b/actions/runs/1".into(),
            ci_sources: None,
            ci_runs: None,
            local_gate: None,
        }
    }

    fn pr() -> PrFacts {
        PrFacts {
            repo: "a/b".into(),
            number: 7,
            state: "open".into(),
            merged: false,
            head_sha: SHA.into(),
            head_ref: "crew/hermes@b/12-fix-it".into(),
            title: "Work".into(),
            body: "Closes #12".into(),
        }
    }

    fn green() -> VerifyCi {
        VerifyCi {
            facts: CiFacts::Green {
                run_url: "https://github.com/a/b/actions/runs/1".into(),
                sources: CiSources {
                    seen: vec!["workflow-runs".into()],
                    counted: vec!["1 (push, success)".into()],
                    ..CiSources::default()
                },
            },
            findings: vec![],
            no_run: None,
        }
    }

    struct Fix {
        ev: EvidenceMsg,
        pr: PrFacts,
        ci: Option<VerifyCi>,
        repos: Vec<String>,
    }

    fn fix() -> Fix {
        Fix {
            ev: ev(),
            pr: pr(),
            ci: Some(green()),
            repos: vec!["a/b".into()],
        }
    }

    fn facts_of(x: &Fix) -> VerifyFacts<'_> {
        VerifyFacts {
            ticket_key: "a/b#12".into(),
            assignee: "hermes@b",
            done_comment: 55,
            ev: &x.ev,
            coordinator_gate: None,
            ev_local: false,
            local_verify: false,
            declared: false,
            pr: &x.pr,
            ci: x.ci.as_ref(),
            linked_repos: &x.repos,
            gate_failed: false,
            note: "",
            from: Status::Review,
            actor: COORD,
            labelled_needs_owner: false,
            question_open: false,
        }
    }

    fn failures(x: &Fix) -> Vec<String> {
        verify_failures(&facts_of(x), &|| "crew/".to_string()).failures
    }

    fn msg(kind: Kind, to: &str, body: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: 9,
            created: 0,
            sender: Sender::Agent {
                name: "hermes@b".into(),
                role: Role::Dev,
            },
            to: to.into(),
            kind: Some(kind),
            re: ReName::None,
            body: body.into(),
        }
    }

    #[test]
    fn a_ticket_without_an_assignee_or_without_a_done_has_no_evidence() {
        let r = latest_evidence("a/b#12", None, &[]).unwrap_err();
        assert_eq!(r.rule, "verify-no-evidence");
        assert!(r.message.contains("has no assignee"));
        let r = latest_evidence("a/b#12", Some("hermes@b"), &[]).unwrap_err();
        assert_eq!(r.rule, "verify-no-evidence");
        assert!(r.message.contains("no signed done message of hermes@b"));
        // a signed done of the assignee is the evidence
        let done = msg(
            Kind::Done,
            "coordinator",
            &format!("Done.\npr: {PR_URL}\nsha: {SHA}\nci: none\n"),
        );
        let msgs = [done];
        let (who, m, e) = latest_evidence("a/b#12", Some("hermes@b"), &msgs).unwrap();
        assert_eq!(
            (who.as_str(), m.comment_id, e.sha.as_str()),
            ("hermes@b", 9, SHA)
        );
    }

    #[test]
    fn evidence_that_holds_has_no_failure_and_keeps_the_fresh_ci_text() {
        let x = fix();
        let f = verify_failures(&facts_of(&x), &|| "crew/".to_string());
        assert!(f.failures.is_empty());
        assert_eq!(f.fresh_sources.as_deref(), Some("workflow-runs"));
        assert_eq!(f.fresh_runs.as_deref(), Some("counted: 1 (push, success)"));
        assert_eq!(f.fresh_ci, None);
    }

    #[test]
    fn a_foreign_repo_a_closed_pr_and_a_moved_head_are_failures_in_this_order() {
        let mut x = fix();
        x.repos = vec!["a/c".into()];
        x.pr.state = "closed".into();
        x.pr.head_sha = OTHER.into();
        let f = failures(&x);
        assert_eq!(f.len(), 3, "{f:?}");
        assert!(f[0].contains("is not in a repo of the initiative"));
        assert_eq!(f[1], "the PR is closed without a merge");
        assert!(f[2].starts_with("PR head moved: the evidence says"));
        // a merged PR is not "closed without a merge"
        x.pr.merged = true;
        assert_eq!(failures(&x).len(), 2);
    }

    #[test]
    fn ci_none_needs_the_declaration_and_then_needs_no_run() {
        let mut x = fix();
        x.ev.ci = "none".into();
        x.ci = Some(VerifyCi {
            facts: CiFacts::NoCi(vec![]),
            findings: vec![],
            no_run: None,
        });
        let f = failures(&x);
        assert!(f[0].contains("does not declare 'CI: none'"), "{f:?}");
        assert!(f[1].starts_with("no CI run for "), "{f:?}");
        let mut facts = facts_of(&x);
        facts.declared = true;
        assert!(
            verify_failures(&facts, &|| String::new())
                .failures
                .is_empty()
        );
    }

    #[test]
    fn a_head_without_a_run_names_the_cause_and_a_paths_ignored_head_is_accepted() {
        let mut x = fix();
        x.ci = Some(VerifyCi {
            facts: CiFacts::NoCi(vec![]),
            findings: vec![],
            no_run: Some(Err("a workflow file changed".into())),
        });
        let f = failures(&x);
        assert_eq!(f.len(), 1);
        assert!(f[0].contains("Not accepted as paths-ignored: a workflow file changed."));
        assert!(f[0].contains("push to crew/**"));
        x.ci = Some(VerifyCi {
            facts: CiFacts::NoCi(vec![]),
            findings: vec![],
            no_run: Some(Ok("none (all files paths-ignored: docs)".into())),
        });
        let r = verify_failures(&facts_of(&x), &|| "crew/".to_string());
        assert!(r.failures.is_empty());
        assert_eq!(
            r.fresh_ci.as_deref(),
            Some("none (all files paths-ignored: docs)")
        );
    }

    #[test]
    fn red_unreadable_and_information_only_ci_are_failures_and_findings_come_first() {
        let mut x = fix();
        x.ci = Some(VerifyCi {
            facts: CiFacts::NotGreen(vec!["build".into(), "test".into()]),
            findings: vec![Refusal::new("evidence-run-missing", "run 9 is gone")],
            no_run: None,
        });
        let f = failures(&x);
        assert_eq!(f[0], "run 9 is gone [evidence-run-missing]");
        assert!(f[1].ends_with("is not green: build; test"));
        x.ci = Some(VerifyCi {
            facts: CiFacts::NotQueried("HTTP 403".into()),
            findings: vec![],
            no_run: None,
        });
        assert!(failures(&x)[0].ends_with("[evidence-ci-unreadable]"));
        x.ci = Some(VerifyCi {
            facts: CiFacts::OnlyInformation {
                info: vec!["9 (schedule)".into()],
                unreadable: vec![],
            },
            findings: vec![],
            no_run: None,
        });
        let f = failures(&x);
        assert!(f[0].contains("only runs that are information"), "{f:?}");
        assert!(f[0].contains("--run <id>"));
    }

    #[test]
    fn a_local_gate_without_its_line_or_in_ci_mode_is_a_failure_and_reads_no_ci() {
        let mut x = fix();
        x.ev.ci = "local-gate".into();
        x.ci = None;
        let mut f = facts_of(&x);
        f.ev_local = true;
        f.local_verify = true;
        let r = verify_failures(&f, &|| String::new());
        assert_eq!(r.failures.len(), 1);
        assert!(r.failures[0].ends_with("[local-gate-missing]"));
        x.ev.local_gate = Some("host-a: make test rc=0".into());
        let mut f = facts_of(&x);
        f.ev_local = true;
        f.local_verify = false;
        let r = verify_failures(&f, &|| "crew/".to_string());
        assert!(
            r.failures
                .iter()
                .any(|m| m.ends_with("[local-gate-not-enabled]"))
        );
    }

    #[test]
    fn a_failed_gate_check_is_the_last_failure_and_fails_green_evidence() {
        let x = fix();
        let mut f = facts_of(&x);
        f.gate_failed = true;
        f.note = "  Gate part 2 is missing ";
        let r = verify_failures(&f, &|| String::new());
        assert_eq!(
            r.failures,
            ["Gate check by the coordinator: Gate part 2 is missing"]
        );
    }

    #[test]
    fn a_pass_is_refused_for_a_protected_path_and_a_fail_is_not() {
        use crate::rules::protected::Protect;
        let hit = PrFileList {
            names: vec!["AUTHORITY.md".into()],
            previous: vec![],
            truncated: false,
        };
        let r = check_pass_allowed(
            Some(&hit),
            PR_URL,
            "a/b#12",
            Protect::new("AUTHORITY.md", &[]),
        )
        .unwrap_err();
        assert_eq!(r.rule, crate::rules::protected::RULE);
        // --fail reads no file list and stays allowed
        assert!(
            check_pass_allowed(None, PR_URL, "a/b#12", Protect::new("AUTHORITY.md", &[])).is_ok()
        );
        let clean = PrFileList {
            names: vec!["src/lib.rs".into()],
            previous: vec![],
            truncated: false,
        };
        assert!(
            check_pass_allowed(
                Some(&clean),
                PR_URL,
                "a/b#12",
                Protect::new("AUTHORITY.md", &[])
            )
            .is_ok()
        );
        // a list that is cut cannot prove it
        let cut = PrFileList {
            truncated: true,
            ..clean
        };
        assert!(
            check_pass_allowed(
                Some(&cut),
                PR_URL,
                "a/b#12",
                Protect::new("AUTHORITY.md", &[])
            )
            .is_err()
        );
    }

    #[test]
    fn a_pass_posts_pass_and_writes_nothing_else() {
        let x = fix();
        let f = facts_of(&x);
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert!(p.pass && p.fail_move.is_none() && !p.clear_needs_owner);
        assert_eq!(p.assignee, "hermes@b");
        assert_eq!(p.done_comment, 55);
        assert_eq!(
            p.first_line,
            "verify PASS for a/b#12: evidence still holds\n"
        );
        assert_eq!(
            p.rest,
            "next: crewboard close a/b#12 --verified (after you checked the Gate)\n"
        );
        assert!(p.body.starts_with("verify: pass\npr: "));
    }

    #[test]
    fn a_fail_of_an_idle_agent_moves_the_ticket_back_to_in_progress() {
        let mut x = fix();
        x.pr.head_sha = OTHER.into();
        let mut f = facts_of(&x);
        f.labelled_needs_owner = true;
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert!(!p.pass);
        let m = p.fail_move.clone().unwrap();
        assert_eq!((m.to, m.order), (Status::InProgress, None));
        assert_eq!(m.line, "Status moved back to In Progress\n");
        assert!(p.rest.starts_with("failed: PR head moved"));
        assert!(p.body.starts_with("verify: fail\n"));
        // the label goes when no question is open, and stays while one is
        assert!(p.clear_needs_owner);
        f.question_open = true;
        let findings = verify_failures(&f, &|| String::new());
        assert!(!decide_verify(&f, findings, None).unwrap().clear_needs_owner);
    }

    #[test]
    fn a_fail_of_a_busy_agent_queues_the_ticket_first_in_ready() {
        let mut x = fix();
        x.pr.state = "closed".into();
        let f = facts_of(&x);
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, Some(("a/b#3".into(), 4))).unwrap();
        let m = p.fail_move.unwrap();
        assert_eq!((m.to, m.order), (Status::Ready, Some(4)));
        assert!(
            m.line
                .starts_with("Status moved to Ready, Order 4 (first in the queue of hermes@b)")
        );
        assert!(p.body.contains(
            "case: verify failed; hermes@b works on a/b#3; queued as next (Ready, Order 4)"
        ));
    }

    #[test]
    fn the_move_after_a_failed_verify_is_checked_against_the_table_in_the_plan() {
        let mut x = fix();
        x.pr.state = "closed".into();
        let mut f = facts_of(&x);
        f.actor = Actor::Agent(Role::Dev);
        let findings = verify_failures(&f, &|| String::new());
        let r = decide_verify(&f, findings, None).unwrap_err();
        assert_eq!(r.rule, "status-wrong-writer");
    }
}
