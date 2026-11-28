//! `verify`: the coordinator checks the evidence of a delivered ticket.
//!
//! Three steps: [`load_verify`] reads the ticket, [`plan_verify`] reads the PR and the CI and
//! decides ([`verify_failures`] and [`decide_verify`] are the pure parts, a [`VerifyPlan`] their
//! result), [`act_verify`] performs the writes of the plan in order.
//!
//! The round limit (spec/tickets#round-limit): with `max_rounds` in AUTHORITY.md, the failed
//! verify that spends the last round moves the ticket to Blocked with a question to the owner
//! ([`round_stop`]). The headless `tick` runs this code, so it does the same.

use super::in_progress::in_progress_key;
use super::local_gate::local_gate_for_verify;
use super::order;
use super::pr_evidence::{evidence_runs, fetch_pr, judge_no_run_for, pr_files_for_protection};
use super::run_checks::RunArgs;
use super::{
    Loaded, Me, check_verify_ticket, load_signed, needs_owner_lines, post_with, remember, to_agent,
    work_write_ctx_of,
};
use crate::authority::Authority;
use crate::base::ids;
use crate::clock::{Clock, SystemClock};
use crate::domain::ci::evaluate::CiRuns;
use crate::error::{CbError, CbResult};
use crate::headless::UAct;
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::acts::{
    EvidenceMsg, FailClass, VerifyFailure, counts_as_round, failed_line, is_local_gate,
    round_limit_case, round_limit_case_of, round_limit_question, verify_body,
};
use crate::protocol::names::{Kind, Re, To};
use crate::rules::Actor;
use crate::rules::Refusal;
use crate::rules::evidence::{CiFacts, PrFacts, PrUrl, parse_pr_url};
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::protected::PrFileList;
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_loader::Ticket;
use crate::ticket::{
    VMsg, ci_none_declared, fold_rounds, latest_done, question_open, verify_report,
};

/// `verify <issue> [--body ...]` (coordinator): spec/tickets#verification-by-the-coordinator steps
/// 1 and 2; the gate itself (step 3) is the model's work, and `--body` records what it found.
/// `verify --fail` needs `--body`, the finding of the coordinator's own Gate
/// check. Pure; the CLI runs it before any read.
pub fn check_verify_fail_note(gate_failed: bool, note: &str) -> Result<(), Refusal> {
    if gate_failed && note.trim().is_empty() {
        return Err(Refusal::new(
            ids::VERIFY_FAIL_NEEDS_NOTE,
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
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let Loaded { world, me, t, from } = load_signed(ac, &tref)?;
    Ok(VerifyLoad {
        n: tref.issue,
        world,
        me,
        t,
        from,
    })
}

/// The arguments of `verify` that the checks need.
#[derive(Clone, Copy)]
struct VerifyArgs<'a> {
    note: &'a str,
    runs: &'a [String],
    gate_failed: bool,
    local_gate: Option<&'a str>,
    /// `--run-check`: the tool runs these checks and writes the local-run line.
    run: Option<&'a RunArgs<'a>>,
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
        .ok_or_else(|| Refusal::new(ids::VERIFY_NO_EVIDENCE, format!("{key} has no assignee")))?;
    let (done_msg, ev) = latest_done(msgs, assignee).ok_or_else(|| {
        Refusal::new(
            ids::VERIFY_NO_EVIDENCE,
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
    let coordinator_gate = local_gate_for_verify(
        &auth,
        va.local_gate,
        ev_local,
        va.gate_failed,
        va.run.is_some(),
    )?;
    // an unknown name is refused before any read of the PR
    if let Some(run) = va.run {
        super::run_checks::pick(&auth.checks, run.names)?;
    }
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
/// path. Rule `guarded-path-changed`: and for a PR that changes a path of `ticket_guard`, the
/// signed guard of the ticket (spec/tickets#guarded-paths). `files` is `None` for `--fail`,
/// which stays allowed. Pure.
fn check_pass_allowed(
    files: Option<&PrFileList>,
    pr_url: &str,
    key: &str,
    guard: crate::rules::protected::Protect<'_>,
    ticket_guard: &[String],
) -> Result<(), Refusal> {
    match files {
        Some(f) => {
            crate::rules::protected::check_unprotected(pr_url, key, f, guard)?;
            crate::rules::guard::check_untouched(pr_url, key, f, ticket_guard)
        }
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
    let (pr, pr_base) =
        super::pr_evidence::fetch_pr_based(ac, &pre.pr_url.repo, pre.pr_url.number)?;
    // Rule `protected-path-owner-merge`: a PR that changes AUTHORITY.md, .crewboard.toml or a
    // workflow is for the owner: a verify PASS is refused (and nothing is posted). `--fail` stays
    // allowed: the coordinator may still judge the work wrong and send the ticket back. Before
    // the CI read.
    let files = if va.gate_failed {
        None
    } else {
        Some(pr_files_for_protection(ac, &pr, &pr_base, &l.t.key())?)
    };
    let base = pr_base.branch;
    let guard =
        crate::rules::protected::Protect::new(&ac.ctx.cfg.authority, &pre.auth.protected_paths);
    check_pass_allowed(
        files.as_ref(),
        &ev.pr,
        &l.t.key(),
        guard,
        &crate::ticket::signed_guard(&l.t.msgs),
    )?;
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
    /// The round limit; `None` without `max_rounds` in AUTHORITY.md.
    rounds: Option<RoundFacts>,
}

/// What the round limit reads (spec/tickets#round-limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoundFacts {
    /// `max_rounds` of AUTHORITY.md.
    limit: u32,
    /// The rounds that the ticket has spent ([`crate::ticket::Rounds::spent`]); this verify adds
    /// its own.
    prior: usize,
    /// The question of the round limit is open on the ticket already.
    question_open: bool,
}

/// A failed verify that spends the last round of the ticket: the ticket goes to the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoundStop {
    limit: u32,
    /// The rounds with this one.
    rounds: u32,
    /// Post the question to the owner (not when the question of the round limit is open).
    ask: bool,
}

/// Does this verify stop the ticket at the round limit? It does when the limit is set, the fail
/// counts as a round, and it is the last round or one after it (a limit that the owner lowered
/// stops the ticket at the next fail that counts, never before). Pure.
fn round_stop(f: &VerifyFacts<'_>, failures: &[VerifyFailure]) -> Option<RoundStop> {
    let r = f.rounds?;
    if failures.is_empty() {
        return None;
    }
    let mut classes: Vec<FailClass> = failures.iter().map(VerifyFailure::class).collect();
    classes.sort();
    classes.dedup();
    if !counts_as_round(&classes) {
        return None;
    }
    let rounds = u32::try_from(r.prior).unwrap_or(u32::MAX).saturating_add(1);
    (rounds >= r.limit).then_some(RoundStop {
        limit: r.limit,
        rounds,
        ask: !r.question_open,
    })
}

/// The failures of the evidence and the fresh CI text that a pass keeps.
#[derive(Debug, Default, PartialEq, Eq)]
struct VerifyFindings {
    /// Typed, so that every `failed:` line has a class (`protocol::acts::verify_fail`).
    failures: Vec<VerifyFailure>,
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
        failures.push(VerifyFailure::PrOutsideInitiative {
            repo: pr.repo.clone(),
            number: pr.number,
        });
    }
    if pr.state == "closed" && !pr.merged {
        failures.push(VerifyFailure::PrClosed);
    }
    if !pr.head_sha.eq_ignore_ascii_case(&ev.sha) {
        failures.push(VerifyFailure::HeadMoved {
            evidence_sha: ev.sha.clone(),
            head_now: pr.head_sha.clone(),
        });
    }
    let ci_none = ev.ci == "none";
    if ci_none && !f.declared {
        failures.push(VerifyFailure::CiNoneUndeclared);
    }
    if f.ev_local
        && !ev
            .local_gate
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
    {
        failures.push(VerifyFailure::LocalGateMissing);
    }
    if f.ev_local && !f.local_verify {
        failures.push(VerifyFailure::LocalGateNotEnabled);
    }
    if let Some(ci) = f.ci.filter(|_| !f.local_verify) {
        ci_failures(f, ci, prefix, &mut out);
    }
    if f.gate_failed {
        out.failures.push(VerifyFailure::GateCheck {
            note: f.note.trim().to_string(),
        });
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
        out.failures.push(VerifyFailure::CiFinding {
            message: r.message.clone(),
            rule: r.rule.to_string(),
        });
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
        CiFacts::OnlyInformation { info, .. } => out.failures.push(VerifyFailure::NoCountedCiRun {
            sha: ev.sha.clone(),
            info: info.clone(),
            cause: crate::rules::evidence::no_ci_cause(&prefix()),
        }),
        CiFacts::NoCi(_) => match &ci.no_run {
            Some(Ok(text)) => out.fresh_ci = Some(text.clone()),
            other => out.failures.push(VerifyFailure::NoCiRun {
                sha: ev.sha.clone(),
                not_accepted: match other {
                    Some(Err(why)) => Some(why.clone()),
                    _ => None,
                },
                cause: crate::rules::evidence::no_ci_cause(&prefix()),
            }),
        },
        CiFacts::NotGreen(r) => out.failures.push(VerifyFailure::CiNotGreen {
            sha: ev.sha.clone(),
            red: r.clone(),
        }),
        CiFacts::NotQueried(w) => out
            .failures
            .push(VerifyFailure::CiUnreadable { why: w.clone() }),
    }
}

/// The move of a failed verify: back to In Progress, or to Ready (first in the
/// queue) when the agent works on another ticket. At the round limit: to Blocked.
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
    /// The round limit stops the ticket: the question to the owner and the label come before
    /// the Status write.
    round_stop: Option<RoundStop>,
}

/// Decide the report and the writes of `verify` from the findings. Pure. `busy` is the other
/// ticket of the assignee that is In Progress and the `Order` that puts this ticket first in
/// the queue; read only for a failed verify, and not at the round limit.
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
    let stop = round_stop(f, &findings.failures);
    let case = match stop {
        Some(s) => Some(round_limit_case(s.rounds, s.limit)),
        None => busy.as_ref().map(|(other, order)| {
            format!(
                "verify failed; {} works on {other}; queued as next (Ready, Order {order})",
                f.assignee
            )
        }),
    };
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
        rest.push_str(&failed_line(fail));
    }
    let fail_move = if pass {
        rest.push_str(&format!(
            "next: crewboard merge {} (after you checked the Gate; when the owner merges, crewboard close {})\n",
            f.ticket_key, f.ticket_key
        ));
        None
    } else {
        Some(match stop {
            Some(s) => stop_move(f, s)?,
            None => fail_move(f, busy.as_ref())?,
        })
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
        clear_needs_owner: !pass && stop.is_none() && f.labelled_needs_owner && !f.question_open,
        round_stop: stop,
    })
}

/// The move of the failed verify that spends the last round: to Blocked, checked against the
/// status table. No `Order` is written.
fn stop_move(f: &VerifyFacts<'_>, s: RoundStop) -> Result<FailMove, Refusal> {
    let ctx = Transition::AfterVerify {
        verify: Some(false),
        assignee_busy: false,
        rounds_spent: true,
    };
    check_status_transition(Some(f.from), Status::Blocked, f.actor, &ctx)?;
    Ok(FailMove {
        to: Status::Blocked,
        order: None,
        line: stop_line(s.rounds, s.limit, f.assignee),
    })
}

/// The line of the answer for a ticket that the round limit stopped.
fn stop_line(rounds: u32, limit: u32, assignee: &str) -> String {
    format!(
        "Status moved to Blocked: the round limit stopped the ticket ({rounds} failed verifies, max_rounds {limit}). It does not go back to {assignee}. The owner decides: any comment of the owner on the ticket releases it, so the decision belongs in that comment. `crewboard unblock` and `reassign` release it too; `close --why` drops it. For a split, release the ticket first, then run `ticket split`\n"
    )
}

/// A stop at the round limit that an earlier `verify` did not finish: the verify result is in
/// the log, and a later write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StopFinish {
    limit: u32,
    rounds: u32,
    /// The comment of the verify result that stopped the ticket.
    report: u64,
    /// The question of the round limit is not open: post it.
    ask: bool,
}

/// Is a stop at the round limit left to finish? It is when the ticket is in Review, the round
/// limit stopped it with no release since, and the latest verify result for the latest `done` is
/// the result that stopped it. Then a `verify` posts no new result (the judgment is in the log)
/// and does only the writes that are missing. Pure.
fn stop_to_finish(
    msgs: &[VMsg],
    assignee: Option<&str>,
    issue: u64,
    from: Status,
) -> Option<StopFinish> {
    if from != Status::Review {
        return None;
    }
    let rounds = fold_rounds(msgs);
    if !rounds.stopped {
        return None;
    }
    let (done, _) = latest_done(msgs, assignee?)?;
    let (report, pass) = verify_report(msgs, done, issue)?;
    let (spent, limit) = round_limit_case_of(&report.body).filter(|_| !pass)?;
    Some(StopFinish {
        limit,
        rounds: spent,
        report: report.comment_id,
        ask: !rounds.question_open,
    })
}

/// Does `verify` only finish a stop at the round limit on this ticket ([`stop_to_finish`])? The
/// headless `tick` asks, because it runs no `verify` for a `done` that has a result.
pub(super) fn stop_unfinished(t: &Ticket) -> bool {
    t.status()
        .ok()
        .and_then(|from| stop_to_finish(&t.msgs, t.agent(), t.issue, from))
        .is_some()
}

/// Finish the stop of the loaded ticket when one is left to finish and the caller is a
/// coordinator; `None` when there is none, and then nothing was written.
fn finish_if_stopped(ac: &AgentCtx, l: &VerifyLoad) -> CbResult<Option<String>> {
    if !l.me.actor.is_coordinator() {
        return Ok(None);
    }
    match stop_to_finish(&l.t.msgs, l.t.agent(), l.n, l.from) {
        Some(f) => finish_stop(ac, l, f, &SystemClock).map(Some),
        None => Ok(None),
    }
}

/// The entry of the headless `tick` for a stop that is not finished: it reads the ticket again
/// and only finishes the stop. `None`: nothing is left to finish (the stop ended meanwhile), and
/// nothing was written. It never judges evidence, so it cannot pass a ticket.
pub(super) fn finish_stop_only(ac: &AgentCtx, issue_arg: &str) -> CbResult<Option<String>> {
    let l = load_verify(ac, issue_arg)?;
    finish_if_stopped(ac, &l)
}

/// The writes of a stop after the verify result: the question to the owner unless it is open,
/// the label `needs-owner`, then Status Blocked. The question comes before the Status write, and
/// the label right after the question: nothing adds a missing label later, so a failed write
/// must not leave an open question without it. Returns the lines of the answer.
fn stop_writes(
    ac: &AgentCtx,
    t: &Ticket,
    clock: &dyn Clock,
    s: RoundStop,
    report_url: &str,
) -> CbResult<String> {
    let mut out = String::new();
    if s.ask {
        let q = post_with(
            ac,
            t,
            clock,
            To::Owner,
            Kind::Question,
            Re::None,
            &round_limit_question(s.rounds, s.limit, report_url),
        )?;
        out.push_str(&format!(
            "round limit: question to the owner {} ({})\n",
            q.id, q.url
        ));
    } else {
        out.push_str("round limit: the question to the owner is open already\n");
    }
    out.push_str(&needs_owner_lines(ac, &t.repo, t.issue, true));
    super::set_select_status(ac, &t.item_id, Status::Blocked)?;
    remember(ac, t, Status::Blocked, t.agent());
    Ok(out)
}

/// Finish a stop at the round limit ([`stop_to_finish`]): no new verify result, only the writes
/// that are missing.
fn finish_stop(
    ac: &AgentCtx,
    l: &VerifyLoad,
    f: StopFinish,
    clock: &dyn Clock,
) -> CbResult<String> {
    let t = &l.t;
    let ctx = Transition::AfterVerify {
        verify: Some(false),
        assignee_busy: false,
        rounds_spent: true,
    };
    check_status_transition(Some(l.from), Status::Blocked, l.me.actor, &ctx)?;
    super::check_status_unchanged(ac, &t.item_id, &t.key(), l.from, Some(Status::Blocked))?;
    let report_url = t
        .comments
        .iter()
        .find(|c| c.id == f.report)
        .map(|c| c.url.clone())
        .unwrap_or_default();
    let stop = RoundStop {
        limit: f.limit,
        rounds: f.rounds,
        ask: f.ask,
    };
    let mut out = format!(
        "verify for {}: the round limit stopped this ticket in an earlier run (result message {}, {report_url}). No new result message; this run finishes the stop\n",
        t.key(),
        f.report
    );
    out.push_str(&stop_writes(ac, t, clock, stop, &report_url)?);
    out.push_str(&stop_line(
        f.rounds,
        f.limit,
        t.agent().unwrap_or("its agent"),
    ));
    Ok(out)
}

/// The move after a failed verify, checked against the status table and the field rules.
fn fail_move(f: &VerifyFacts<'_>, busy: Option<&(String, i64)>) -> Result<FailMove, Refusal> {
    let ctx = Transition::AfterVerify {
        verify: Some(false),
        assignee_busy: busy.is_some(),
        rounds_spent: false,
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
    let mut pre = precheck_verify(ac, l, va)?;
    let mut read = read_verify(ac, l, &pre, va)?;
    // `--run-check`: after every refusal of `verify` that needs no run (the writer, the status,
    // the protected paths), and only for a head that is the commit of the evidence. The line
    // belongs to the `done` that this command loaded: a `done` that arrives while the checks
    // run is not answered by it. A head that moved in that time is the failure of every verify.
    // A check that failed in an unattended run: the failure text of this verify.
    let mut failed_check: Option<String> = None;
    if let Some(run) = va.run
        && read.pr.head_sha.eq_ignore_ascii_case(&pre.ev.sha)
    {
        // A closed or merged PR gets no run, and so no pass that says "the tool ran the checks".
        if read.pr.state == "closed" {
            return Err(Refusal::new(
                ids::CHECK_NOT_RUN,
                format!(
                    "the checks did not run: the PR of {} is closed or merged. Nothing was written",
                    t.key()
                ),
            )
            .into());
        }
        let checks = super::run_checks::pick(&pre.auth.checks, run.names)?;
        let after_fail = format!(
            "\nNothing was written. To send the ticket back: crewboard verify {} --fail --body \"<what failed>\"",
            l.n
        );
        let ticket_guard = crate::ticket::signed_guard(&t.msgs);
        let target = super::run_checks::Target {
            key: &t.key(),
            sha: &pre.ev.sha,
            repo: &pre.pr_url.repo,
            pr: pre.pr_url.number,
            protected_paths: &pre.auth.protected_paths,
            guard: &ticket_guard,
            env: &pre.auth.check_env,
            timeout: pre.auth.check_timeout,
            after_fail: &after_fail,
        };
        match super::run_checks::run(ac, &target, &checks, run) {
            Ok(line) => pre.coordinator_gate = Some(line),
            // unattended: the failed check is the result of this verify, for the `done` that
            // was tested. The first line is the fact; the lines after it are output of the PR.
            Err(CbError::Refused(r)) if run.unattended && r.rule == ids::CHECK_FAILED => {
                let fact = r.message.lines().next().unwrap_or("a check failed");
                // `fact` starts with "check <name> ": the note starts with TOOL_CHECK_NOTE, which
                // `retro` reads as the tag `check-<name>`
                let note = format!(
                    "{}{}",
                    crate::protocol::acts::TOOL_CHECK_NOTE,
                    fact.strip_prefix("check ").unwrap_or(fact)
                );
                crate::ops::say::check_body_text(&note)?;
                failed_check = Some(note);
            }
            Err(e) => return Err(e),
        }
        read.pr = fetch_pr(ac, &pre.pr_url.repo, pre.pr_url.number)?.0;
    }
    let gate_failed = va.gate_failed || failed_check.is_some();
    let note = failed_check.as_deref().unwrap_or(va.note);
    let rounds = fold_rounds(&t.msgs);
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
        gate_failed,
        note,
        from: l.from,
        actor: l.me.actor,
        labelled_needs_owner: t
            .info
            .labels
            .iter()
            .any(|x| x == crate::labels::NEEDS_OWNER),
        question_open: question_open(&t.msgs, t.agent()).is_some(),
        rounds: pre.auth.max_rounds.map(|limit| RoundFacts {
            limit,
            prior: rounds.spent(),
            question_open: rounds.question_open,
        }),
    };
    let findings = verify_failures(&facts, &|| {
        crate::ctx::load_authority_quiet(&ac.ctx)
            .unwrap_or_default()
            .branch_prefix()
    });
    // A failed verify needs to know if the agent works on another ticket. This is
    // the ONE added read: the board rows of the assignee's tickets (`nodes(ids:)`), only on a fail.
    // At the round limit the ticket goes to the owner, so nobody reads the queue.
    let busy = if findings.failures.is_empty() || round_stop(&facts, &findings.failures).is_some() {
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
    if let (Some(s), Some(m)) = (p.round_stop, &p.fail_move) {
        // A write that fails here leaves a stop that the next `verify` finishes
        // ([`stop_to_finish`]): the result is in the log, so that run posts no second one.
        out.push_str(&stop_writes(ac, t, clock, s, &posted.url)?);
        out.push_str(&m.line);
        return Ok(out);
    }
    if let Some(m) = &p.fail_move {
        super::set_select_status(ac, &t.item_id, m.to)?;
        if let Some(order) = m.order {
            order::write_order(ac, &t.item_id, order, &work_write_ctx_of(l.me.actor, false))?;
        }
        remember(ac, t, m.to, t.agent());
        out.push_str(&m.line);
        if p.clear_needs_owner {
            out.push_str(&needs_owner_lines(ac, &t.repo, t.issue, false));
        }
    }
    Ok(out)
}

/// Who runs `verify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// The command `verify` of a person or a model.
    Command,
    /// The headless `tick`, in its own process: its work on a typed Gate is no judgment of a
    /// model, so it is not a verdict of the unattended coordinator (no stamp, no count).
    Tick,
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
    run: Option<&RunArgs<'_>>,
) -> CbResult<String> {
    let va = VerifyArgs {
        note,
        runs,
        gate_failed,
        local_gate,
        run,
    };
    verify_as(ac, Origin::Command, issue_arg, &va)
}

/// The `verify` of the headless `tick`: a passing verify of a ticket whose Gate is `ci`, or the
/// checks of a `checks:` Gate that the tool ran (`run`).
pub(super) fn verify_by_tick(
    ac: &mut AgentCtx,
    issue_arg: &str,
    note: &str,
    run: Option<&RunArgs<'_>>,
) -> CbResult<String> {
    let va = VerifyArgs {
        note,
        runs: &[],
        gate_failed: false,
        local_gate: None,
        run,
    };
    verify_as(ac, Origin::Tick, issue_arg, &va)
}

/// Is this `verify` no judgment: a plain verify of a ticket whose signed Gate is the typed `ci`
/// and that does not declare `CI: none`? It only reads the evidence and the CI again. A failed
/// Gate check (`--fail`) is a judgment on every Gate, and a ticket with `CI: none` has no CI
/// that could decide.
fn is_ci_only_verify(t: &Ticket, va: &VerifyArgs<'_>) -> bool {
    !va.gate_failed
        && crate::ticket::signed_gate(&t.msgs)
            .is_some_and(|g| crate::headless::Gate::parse(&g).is_ci_only())
        && !ci_none_declared(&t.msgs, &t.info.body)
}

/// The checks of the command `verify` in a headless config dir
/// (spec/agents#unattended-coordinator); an attended agent passes without a check.
///
/// - A verify that is no judgment ([`is_ci_only_verify`]) needs no mode. It takes no `--body`:
///   a free text on such a verify would be a message of a model that nothing counts.
/// - Any other verify is a judgment: the mode of AUTHORITY.md and a verdict of the day
///   (`admit`), a `--body` that records what was judged, and the signed log of the ticket
///   (`check_ticket`).
fn check_headless_verify(ac: &AgentCtx, l: &VerifyLoad, va: &VerifyArgs<'_>) -> CbResult<()> {
    crate::ops::unattended::check_no_stamp_line(ac, va.note)?;
    if !ac.ctx.cd.is_headless()? {
        return Ok(());
    }
    if is_ci_only_verify(&l.t, va) {
        if !va.note.trim().is_empty() {
            return Err(crate::headless::unattended_refusal(&format!(
                "the Gate of {} is `ci`, and a verify of such a ticket takes no --body in a headless config dir: it only reads the evidence and the CI again",
                l.t.key()
            ))
            .into());
        }
        return Ok(());
    }
    crate::ops::unattended::admit(ac, &l.world, UAct::Verdict)?;
    if va.note.trim().is_empty() {
        return Err(crate::headless::unattended_refusal(
            "a verdict of the unattended coordinator needs --body \"<what you checked and what you found>\": the owner reads it as the reason",
        )
        .into());
    }
    crate::ops::unattended::check_ticket(ac, UAct::Verdict, &l.t, None)
}

fn verify_as(
    ac: &mut AgentCtx,
    origin: Origin,
    issue_arg: &str,
    va: &VerifyArgs<'_>,
) -> CbResult<String> {
    let l = load_verify(ac, issue_arg)?;
    // Before the stop below: a stopped ticket is for the owner, and the `tick` finishes its
    // stop.
    if origin == Origin::Command {
        check_headless_verify(ac, &l, va)?;
    }
    // A stop at the round limit that is not finished comes first: before the check of an open
    // question to the owner (the question of the stop may be the open one) and before any read
    // of the PR. It writes no judgment, so a headless coordinator does it too.
    if let Some(out) = finish_if_stopped(ac, &l)? {
        return Ok(out);
    }
    let plan = plan_verify(ac, &l, va)?;
    // The unattended coordinator takes its messages from the budget before the first write: the
    // verify result, and with it the question to the owner when this verify stops the ticket at
    // the round limit. Nothing happens for any other caller.
    if origin == Origin::Command {
        let question = plan.round_stop.is_some_and(|s| s.ask);
        crate::ops::unattended::reserve_verdict(ac, &l.t.tref(), question)?;
    }
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

    use crate::rules::Role;
    use crate::rules::evidence::CiSources;
    use crate::ticket::VMsg;

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
            rounds: None,
        }
    }

    /// The facts of the round limit `limit` with `prior` rounds spent.
    fn rounds(limit: u32, prior: usize) -> Option<RoundFacts> {
        Some(RoundFacts {
            limit,
            prior,
            question_open: false,
        })
    }

    fn texts(f: &[VerifyFailure]) -> Vec<String> {
        f.iter().map(ToString::to_string).collect()
    }

    fn failures(x: &Fix) -> Vec<String> {
        texts(&verify_failures(&facts_of(x), &|| "crew/".to_string()).failures)
    }

    fn msg(kind: Kind, to: &str, body: &str) -> VMsg {
        let dev = crate::testkit::agent("hermes@b", Role::Dev);
        crate::testkit::msg(9, dev, to, Some(kind), body)
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
            findings: vec![Refusal::new(ids::EVIDENCE_CI_MISSING, "run 9 is gone")],
            no_run: None,
        });
        let f = failures(&x);
        assert_eq!(f[0], "run 9 is gone [evidence-ci-missing]");
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
        assert_eq!(r.failures, [VerifyFailure::LocalGateMissing]);
        x.ev.local_gate = Some("host-a: make test rc=0".into());
        let mut f = facts_of(&x);
        f.ev_local = true;
        f.local_verify = false;
        let r = verify_failures(&f, &|| "crew/".to_string());
        assert!(
            texts(&r.failures)
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
            texts(&r.failures),
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
            &[],
        )
        .unwrap_err();
        assert_eq!(r.rule, ids::PROTECTED_PATH_OWNER_MERGE);
        // --fail reads no file list and stays allowed
        assert!(
            check_pass_allowed(
                None,
                PR_URL,
                "a/b#12",
                Protect::new("AUTHORITY.md", &[]),
                &[]
            )
            .is_ok()
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
                Protect::new("AUTHORITY.md", &[]),
                &[]
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
                Protect::new("AUTHORITY.md", &[]),
                &[]
            )
            .is_err()
        );
        // the signed guard of the ticket: a pass is refused for a guarded file, a fail is not
        let guard = vec!["src/**".to_string()];
        let clean = PrFileList {
            names: vec!["src/lib.rs".into()],
            previous: vec![],
            truncated: false,
        };
        let auth = Protect::new("AUTHORITY.md", &[]);
        let r = check_pass_allowed(Some(&clean), PR_URL, "a/b#12", auth, &guard).unwrap_err();
        assert_eq!(r.rule, ids::GUARDED_PATH_CHANGED);
        assert!(check_pass_allowed(None, PR_URL, "a/b#12", auth, &guard).is_ok());
        let other = vec!["tests/**".to_string()];
        assert!(check_pass_allowed(Some(&clean), PR_URL, "a/b#12", auth, &other).is_ok());
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
            "next: crewboard merge a/b#12 (after you checked the Gate; when the owner merges, crewboard close a/b#12)\n"
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

    /// The plan of a failed Gate check with the facts of the round limit.
    fn gate_fail_plan(r: Option<RoundFacts>, busy: Option<(String, i64)>) -> VerifyPlan {
        let x = fix();
        let mut f = facts_of(&x);
        f.gate_failed = true;
        f.note = "Gate part 2 is missing";
        f.rounds = r;
        let findings = verify_failures(&f, &|| String::new());
        decide_verify(&f, findings, busy).unwrap()
    }

    #[test]
    fn a_fail_below_the_round_limit_moves_as_without_a_limit() {
        // round 2 of 3: back to the agent, no case line, no stop
        let p = gate_fail_plan(rounds(3, 1), None);
        assert_eq!(p.round_stop, None);
        assert_eq!(p.fail_move.unwrap().to, Status::InProgress);
        assert!(!p.body.contains("case:"), "{}", p.body);
        // no key: the same plan
        assert_eq!(
            gate_fail_plan(None, None),
            gate_fail_plan(rounds(3, 1), None)
        );
        // a busy agent below the limit gets the ticket in Ready
        let p = gate_fail_plan(rounds(3, 1), Some(("a/b#3".into(), 4)));
        assert_eq!(p.fail_move.unwrap().to, Status::Ready);
    }

    #[test]
    fn the_fail_that_spends_the_last_round_moves_the_ticket_to_blocked_for_the_owner() {
        // round 3 of 3
        let p = gate_fail_plan(rounds(3, 2), None);
        assert_eq!(
            p.round_stop,
            Some(RoundStop {
                limit: 3,
                rounds: 3,
                ask: true
            })
        );
        let m = p.fail_move.clone().unwrap();
        assert_eq!((m.to, m.order), (Status::Blocked, None));
        assert!(
            m.line.starts_with(
                "Status moved to Blocked: the round limit stopped the ticket (3 failed verifies, max_rounds 3)"
            ),
            "{}",
            m.line
        );
        assert!(m.line.contains("The owner decides"), "{}", m.line);
        assert!(
            m.line
                .contains("For a split, release the ticket first, then run `ticket split`"),
            "{}",
            m.line
        );
        assert!(
            p.body
                .contains("case: round limit 3 of 3; the owner decides\n"),
            "{}",
            p.body
        );
        assert!(crate::protocol::acts::is_round_limit_fail(&p.body));
        assert!(p.body.starts_with("verify: fail\n"));
        // a busy agent changes nothing at the limit: no Ready, no Order, no queue line
        let busy = gate_fail_plan(rounds(3, 2), Some(("a/b#3".into(), 4)));
        assert_eq!(busy.fail_move, p.fail_move);
        assert!(!busy.body.contains("queued as next"), "{}", busy.body);
        // `max_rounds: 1`: the first fail that counts goes to the owner
        let first = gate_fail_plan(rounds(1, 0), None);
        assert_eq!(first.round_stop.map(|s| s.rounds), Some(1));
        // a limit that the owner lowered below the count stops at this fail
        let lowered = gate_fail_plan(rounds(2, 4), None);
        assert_eq!(
            lowered.round_stop.map(|s| (s.rounds, s.limit)),
            Some((5, 2))
        );
        assert!(lowered.body.contains("case: round limit 5 of 2;"));
    }

    #[test]
    fn at_the_round_limit_an_open_round_question_is_not_asked_again_and_the_label_stays() {
        let x = fix();
        let mut f = facts_of(&x);
        f.gate_failed = true;
        f.note = "Gate part 2 is missing";
        f.labelled_needs_owner = true;
        f.question_open = true;
        f.rounds = Some(RoundFacts {
            limit: 2,
            prior: 2,
            question_open: true,
        });
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert_eq!(p.round_stop.map(|s| s.ask), Some(false));
        assert_eq!(p.fail_move.unwrap().to, Status::Blocked);
        assert!(!p.clear_needs_owner);
        // another open question (of the assignee) does not replace the question of the limit
        f.rounds = Some(RoundFacts {
            limit: 2,
            prior: 2,
            question_open: false,
        });
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert_eq!(p.round_stop.map(|s| s.ask), Some(true));
        // the label never goes at the limit, also when no question was open
        f.question_open = false;
        let findings = verify_failures(&f, &|| String::new());
        assert!(!decide_verify(&f, findings, None).unwrap().clear_needs_owner);
    }

    #[test]
    fn a_stop_that_is_not_finished_needs_only_the_missing_writes() {
        use crate::protocol::acts::{round_limit_case, round_limit_question, withdraw_body};
        use crate::protocol::names::Re;
        let coord = || crate::testkit::agent("claude@a", Role::Coordinator);
        let done = |id: u64| {
            crate::testkit::msg(
                id,
                crate::testkit::agent("hermes@b", Role::Dev),
                "coordinator",
                Some(Kind::Done),
                &format!("Done.\npr: {PR_URL}\nsha: {SHA}\nci: none\n"),
            )
        };
        let report = |id: u64, done: u64, case: Option<&str>| VMsg {
            re: Re::Comment {
                issue: 12,
                comment: done,
            },
            ..crate::testkit::msg(
                id,
                coord(),
                "hermes@b",
                Some(Kind::Report),
                &format!(
                    "verify: fail\nfailed: Gate check by the coordinator: x\n{}",
                    case.map(|c| format!("case: {c}\n")).unwrap_or_default()
                ),
            )
        };
        let question = |id: u64| {
            crate::testkit::msg(
                id,
                coord(),
                "owner",
                Some(Kind::Question),
                &round_limit_question(3, 2, "u"),
            )
        };
        let case = round_limit_case(3, 2);
        let dev = Some("hermes@b");
        let finish = |msgs: &[VMsg], from| stop_to_finish(msgs, dev, 12, from);
        // the result is posted, the question is not: ask, with the numbers of the result
        let msgs = vec![done(1), report(2, 1, Some(&case))];
        assert_eq!(
            finish(&msgs, Status::Review),
            Some(StopFinish {
                limit: 2,
                rounds: 3,
                report: 2,
                ask: true
            })
        );
        // the question is posted too: only the label and the Status are left
        let mut asked = msgs.clone();
        asked.push(question(3));
        assert_eq!(finish(&asked, Status::Review).map(|f| f.ask), Some(false));
        // nothing to finish: the ticket is Blocked already, or the stop was released
        assert_eq!(finish(&asked, Status::Blocked), None);
        let mut released = asked.clone();
        released.push(crate::testkit::msg(
            4,
            coord(),
            "hermes@b",
            Some(Kind::Answer),
            &withdraw_body("go on"),
        ));
        assert_eq!(finish(&released, Status::Review), None);
        // a plain fail, a newer `done` without a result, and no assignee: a normal verify
        assert_eq!(finish(&[done(1), report(2, 1, None)], Status::Review), None);
        let mut newer = msgs.clone();
        newer.push(done(5));
        assert_eq!(finish(&newer, Status::Review), None);
        assert_eq!(stop_to_finish(&msgs, None, 12, Status::Review), None);
        // the result of another issue number does not answer this `done`
        assert_eq!(stop_to_finish(&msgs, dev, 13, Status::Review), None);
    }

    #[test]
    fn a_head_moved_only_fail_and_a_pass_never_stop_at_the_round_limit() {
        // head moved only, at the limit: the move of today
        let mut x = fix();
        x.pr.head_sha = OTHER.into();
        let mut f = facts_of(&x);
        f.rounds = rounds(1, 5);
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert_eq!(p.round_stop, None);
        assert_eq!(p.fail_move.unwrap().to, Status::InProgress);
        // head moved and a failed Gate check: a round
        f.gate_failed = true;
        f.note = "wrong";
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert!(p.round_stop.is_some());
        // a pass at the limit is a pass
        let x = fix();
        let mut f = facts_of(&x);
        f.rounds = rounds(1, 5);
        let findings = verify_failures(&f, &|| String::new());
        let p = decide_verify(&f, findings, None).unwrap();
        assert!(p.pass && p.round_stop.is_none() && p.fail_move.is_none());
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
        // and so is the move to Blocked at the round limit
        f.rounds = rounds(1, 0);
        let findings = verify_failures(&f, &|| String::new());
        let r = decide_verify(&f, findings, None).unwrap_err();
        assert_eq!(r.rule, "status-wrong-writer");
    }
}
