//! `ticket new` and `ticket adopt` (coordinator): put a ticket on the board
//! without assigning it, or with `--assign`.
//!
//! The signed info comment with the item id, `Kind`, `Status` and `Gate` are written the same
//! way as `assign` writes them, so `status` lists a parked ticket like an assigned one.
//!
//! Order: pure checks (title, body, Gate, secrets), the writer, the authority, the
//! assignment checks (so that a refused `--assign` creates nothing), then the writes.

use super::assign::assign;
use super::assign_checks::{CapOverride, check_assignable, check_capabilities};
use super::board_put::{Adoption, put_on_board};
use super::pr_evidence::fetch_pr;
use super::{Me, load_world_cached, me_of, work_write_ctx};
use crate::error::{CbError, CbResult};
use crate::github::GhError;
use crate::ops::say::prepare_body;
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::AgentName;
use crate::rules::evidence::parse_pr_url;
use crate::rules::fields::{Field, Item, check_field_write};
use crate::rules::ids;
use crate::rules::limits::check_text_field;
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::services::ticket_loader::{
    IssueInfo, check_repo, check_work_issue, fetch_issue, load_ticket, parse_issue, read_messages,
};
use crate::ticket::log::{Expected, fold_ticket, unsigned_end, unsigned_end_remedy};
use crate::ticket::{WORK_LABEL, body_field, find_item_id};

/// Arguments of `ticket new`.
pub struct NewArgs<'a> {
    pub title: &'a str,
    pub body_file: &'a str,
    /// Repo of the new issue (default: the main repo of the initiative).
    pub repo: Option<&'a str>,
    /// Assign to this agent after the ticket is on the board (default: park in Backlog).
    pub assign: Option<&'a str>,
    /// Adopt the open PR of a Done ticket (a PR URL). The signed creation info
    /// records `adopts-pr: <url> branch: <branch>`; `done` then accepts that branch.
    pub adopt_pr: Option<&'a str>,
    /// `--gate-ci`: the Gate is exactly `ci` (the headless `tick` merges such a ticket).
    pub gate_ci: bool,
}

/// The body for `--gate-ci`: its `Gate:` line is `Gate: ci`, the typed CI-only Gate that the
/// headless `tick` merges. A body without a Gate line gets one at the end. A body whose Gate is
/// already CI only (`ci`, or an old phrase such as `CI green`) has it replaced by `Gate: ci`; a
/// body with another Gate is refused, so a real condition is never dropped. Pure.
pub fn apply_gate_ci(body: &str) -> Result<String, CbError> {
    let own = crate::rules::text::body_fields(body, "Gate");
    if let Some(g) = own
        .iter()
        .find(|g| !crate::headless::Gate::parse(g).is_ci_only())
    {
        return Err(CbError::refused(
            "ticket-gate-conflict",
            format!(
                "--gate-ci writes `Gate: {}`, but the body has its own Gate line ('{g}'): remove that line, or drop --gate-ci",
                crate::headless::GATE_CI
            ),
        ));
    }
    let line = format!("Gate: {}", crate::headless::GATE_CI);
    let mut out = String::new();
    let mut put = false;
    for l in body.lines() {
        if crate::rules::text::body_fields(l, "Gate").is_empty() {
            out.push_str(l);
        } else if !put {
            out.push_str(&line);
            put = true;
        } else {
            continue;
        }
        out.push('\n');
    }
    if !put {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

fn adopt_refused(rule: &'static str, why: String) -> CbError {
    CbError::refused(rule, why)
}

/// The checks of `--continue-pr`, before anything is created: the PR is open and in
/// a repo of the initiative, its head branch is `<dev_branch_prefix><agent>/<n>-<slug>`, and
/// ticket `<n>` (in the repo of the PR) is Done or Dropped, or is back in Backlog or Ready and its
/// reassign notice names this PR. The status of ticket `<n>` is the one of its SIGNED LOG
/// (`fold_ticket`, as `status` and `audit` judge it), never the board field: any holder of the
/// crew token can write the board, and a board Dropped would hand the open PR of a running
/// ticket to a new one.
pub(super) fn check_adoption(ac: &AgentCtx, world: &World, url: &str) -> CbResult<Adoption> {
    let u = parse_pr_url(url)?;
    check_repo(&ac.ctx.cfg, &u.repo)?;
    let (pr, _) = fetch_pr(ac, &u.repo, u.number)?;
    if pr.state != "open" || pr.merged {
        return Err(adopt_refused(
            "adopt-pr-not-open",
            format!(
                "PR {}#{} is {}: a ticket adopts an OPEN PR only",
                pr.repo,
                pr.number,
                if pr.merged { "merged" } else { "closed" }
            ),
        ));
    }
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    let prefix = auth.branch_prefix();
    let Some((_, parent_n)) = crate::rules::evidence::branch_ticket(&prefix, &pr.head_ref) else {
        return Err(adopt_refused(
            "adopt-pr-bad-branch",
            format!(
                "the head branch '{}' of PR {}#{} is not {prefix}<agent>/<n>-<slug>: only a crew branch of an older ticket can be adopted",
                pr.head_ref, pr.repo, pr.number
            ),
        ));
    };
    auth.check_head_branch(&pr.head_ref)?;
    let parent = format!("{}#{parent_n}", pr.repo);
    let parent_done: Result<(), String> = match load_ticket(
        &ac.ctx.reader(),
        &world.trust,
        &pr.repo,
        parent_n,
    ) {
        // An ended ticket by its signed log: Done, or Dropped (a drop leaves its PR open).
        Ok(p) if log_ended(&fold_ticket(&p.msgs).status) => Ok(()),
        // The PR of a ticket that went back to the board: its reassign notice names the PR, so
        // the work is left by an earlier agent and the PR may move to a new ticket.
        Ok(p) if returned_with_pr(&p, &pr) => Ok(()),
        Ok(p) => {
            let log = fold_ticket(&p.msgs).status;
            let board = p.item.field("Status");
            // A board Done or Dropped without a signed end: `close --why` refuses an ended
            // board Status, so the way out starts with the owner's move.
            let way = if board
                .and_then(Status::parse)
                .is_some_and(|b| unsigned_end(b, &log))
            {
                unsigned_end_remedy(parent_n)
            } else {
                format!("end the ticket first: `crewboard close {parent_n} --why \"<text>\"`")
            };
            Err(format!(
                "{parent}, the ticket that branch '{}' names, is {} by its signed log (board: {}): a ticket adopts the PR of a DONE or DROPPED ticket, or the open PR that the reassign notice of a ticket in Backlog or Ready names; the board Status alone does not count ({way})",
                pr.head_ref,
                log.describe(),
                board.unwrap_or("no Status")
            ))
        }
        Err(CbError::Refused(r)) => Err(format!(
            "{parent}, the ticket that branch '{}' names, is not a ticket on the board ({r}): a ticket adopts the PR of a DONE or DROPPED ticket only",
            pr.head_ref
        )),
        // the branch names a ticket number that is no issue of the repo
        Err(CbError::Network(GhError::Http { status: 404, .. })) => Err(format!(
            "{parent}, the ticket that branch '{}' names, does not exist: a ticket adopts the PR of a DONE ticket only",
            pr.head_ref
        )),
        Err(e) => return Err(e),
    };
    if let Err(why) = parent_done {
        return Err(adopt_refused("adopt-pr-parent-not-done", why));
    }
    Ok(Adoption {
        url: format!("https://github.com/{}/pull/{}", pr.repo, pr.number),
        branch: pr.head_ref,
        parent,
        body: pr.body,
        repo: pr.repo,
    })
}

/// The signed log ends the ticket: Done or Dropped, and nothing else. Pure.
fn log_ended(status: &Expected) -> bool {
    matches!(status, Expected::Is(s) if s.ended())
}

/// The parent is back on the board (Backlog or Ready by its signed log) and the latest reassign
/// notice of the coordinator in its log names this open PR: an earlier agent left it
/// (spec/tickets#work-that-an-earlier-agent-left). Pure.
fn returned_with_pr(
    parent: &crate::services::ticket_loader::Ticket,
    pr: &crate::rules::evidence::PrFacts,
) -> bool {
    let back_status = |s: &Status| matches!(s, Status::Backlog | Status::Ready);
    let back = match fold_ticket(&parent.msgs).status {
        Expected::Is(s) => back_status(&s),
        Expected::OneOf(l) => !l.is_empty() && l.iter().all(back_status),
        Expected::Unknown => false,
    };
    let named = crate::ticket::earlier_work(&parent.msgs)
        .and_then(|e| e.pr)
        .and_then(|u| parse_pr_url(&u).ok());
    back && named.is_some_and(|n| n.repo.eq_ignore_ascii_case(&pr.repo) && n.number == pr.number)
}

/// The body template that `crewboard ticket new --template` prints. The same text is in
/// docs/design/spec/tickets.md (a test keeps both equal). `Gate:` is the line that `ticket new` and
/// `assign` require. The optional lines are HTML comments, so a template that is used as it is
/// does not set `Needs`, `Route` or `CI`.
pub const BODY_TEMPLATE: &str = "\
Goal: <one or two sentences>
Gate: <required: one line that can fail>
<!-- CI-only ticket: write the Gate line as: Gate: ci -->
Scope: <what is in scope, and what is not>
Pointers: <files, issues, earlier PRs>
<!-- optional: Needs: x86_64, x86-builds -->
<!-- optional: Route: owner-workflow -->
<!-- optional: CI: none -->
";

/// The pure checks of a ticket text: the title, and the `Gate:` line (spec/tickets#ticket-content)
/// that must be there before anything is created. Returns the Gate.
pub fn check_new_ticket(title: &str, body: &str) -> Result<String, CbError> {
    if title.trim().is_empty() {
        return Err(CbError::refused(
            "ticket-title-missing",
            "ticket new needs --title",
        ));
    }
    if title.chars().count() > 256 {
        return Err(CbError::refused(
            "ticket-title-too-long",
            "the title is longer than 256 characters",
        ));
    }
    let gate = body_field(body, "Gate").ok_or_else(|| {
        CbError::refused(
            ids::STATUS_GATE_MISSING,
            "the ticket body has no 'Gate:' line; see `crewboard ticket new --template`",
        )
    })?;
    check_text_field(&gate)?;
    Ok(gate)
}

/// The common steps of `new` and `adopt`: the ticket is an open `crew:work` issue with a Gate
/// line; make sure it is on the board with `Kind`, `Status` (Backlog if unset) and `Gate`.
/// Idempotent: a ticket that is on the board already keeps its Status.
pub(super) fn adopt_info(
    ac: &AgentCtx,
    world: &World,
    me: &Me,
    info: IssueInfo,
    repo: &str,
    adoption: Option<&Adoption>,
) -> CbResult<(String, String)> {
    let n = info.number;
    // writer first: only the coordinator (or the owner) puts tickets on the board
    check_status_transition(None, Status::Backlog, me.actor, &Transition::Create)?;
    check_work_issue(&info, repo)?;
    if !info.open {
        return Err(CbError::refused(
            "ticket-closed",
            format!("{repo}#{n} is closed"),
        ));
    }
    let gate = check_new_ticket("-", &info.body).map_err(|e| match e {
        CbError::Refused(r) if r.rule == ids::STATUS_GATE_MISSING => CbError::refused(
            ids::STATUS_GATE_MISSING,
            format!(
                "{repo}#{n} has no 'Gate:' line in its body (docs/design/spec/tickets.md#ticket-content)"
            ),
        ),
        other => other,
    })?;
    let (_, msgs) = read_messages(&ac.ctx.reader(), &world.trust, repo, n)?;
    let mut out = String::new();
    let item_id = match find_item_id(&msgs) {
        Some(id) => {
            out.push_str(&format!("{repo}#{n} is on the board already ({id})\n"));
            id
        }
        None => put_on_board(ac, me, &info, repo, adoption, &mut out)?,
    };
    let item = ac.ctx.board.read_item(&ac.ctx.client, &item_id)?;
    let wc = work_write_ctx(me, false);
    if item.field("Kind").is_none() {
        check_field_write(Item::Work, Field::Kind, &wc)?;
        ac.ctx
            .board
            .set_select(&ac.ctx.client, &item_id, "Kind", "Work")?;
    }
    let status = item.field("Status").and_then(Status::parse_board);
    if status.is_none() {
        check_status_transition(None, Status::Backlog, me.actor, &Transition::Create)?;
        ac.ctx
            .board
            .set_select(&ac.ctx.client, &item_id, "Status", Status::Backlog.name())?;
        out.push_str("Status set to Backlog\n");
    }
    if item.field("Gate") != Some(gate.as_str()) {
        check_field_write(Item::Work, Field::Gate, &wc)?;
        ac.ctx
            .board
            .set_text(&ac.ctx.client, &item_id, "Gate", &gate)?;
        out.push_str("Gate field set from the body\n");
    }
    let mut cache = ac.load_cache();
    cache
        .index
        .tickets
        .insert(format!("{repo}#{n}"), item_id.clone());
    ac.save_cache(&mut cache);
    Ok((item_id, out))
}

/// Refuse a `--assign` before anything is created: the agent must be assignable and meet the
/// needs that the ticket text states.
pub(super) fn precheck_assign(
    ac: &AgentCtx,
    world: &World,
    agent: &str,
    title: &str,
    body: &str,
) -> CbResult<()> {
    AgentName::parse(agent)?;
    check_assignable(ac, world, agent)?;
    let stub = IssueInfo {
        node_id: String::new(),
        number: 0,
        title: title.to_string(),
        body: body.to_string(),
        open: true,
        labels: vec![WORK_LABEL.to_string()],
        is_pr: false,
    };
    check_capabilities(world, &stub, agent, CapOverride::default())?;
    Ok(())
}

/// `ticket new --title T --body-file F [--repo R] [--backlog | --assign A]`.
pub fn ticket_new(ac: &mut AgentCtx, a: NewArgs<'_>) -> CbResult<String> {
    let mut body = prepare_body(Some(a.body_file), None)?;
    if a.gate_ci {
        body = apply_gate_ci(&body)?;
    }
    check_new_ticket(a.title, &body)?;
    let repo = a
        .repo
        .map_or_else(|| ac.ctx.main_repo().to_string(), str::to_string);
    check_repo(&ac.ctx.cfg, &repo)?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    check_status_transition(None, Status::Backlog, me.actor, &Transition::Create)?;
    crate::ctx::load_authority(&ac.ctx)?.check_create_ticket()?;
    // The checks of the adopted PR run before anything is created.
    let adoption = a
        .adopt_pr
        .map(|u| check_adoption(ac, &world, u))
        .transpose()?;
    if let Some(agent) = a.assign {
        precheck_assign(ac, &world, agent, a.title, &body)?;
    }
    let r = ac.ctx.client.post(
        &format!("repos/{repo}/issues"),
        &serde_json::json!({"title": a.title.trim(), "body": body, "labels": [WORK_LABEL]}),
    )?;
    let info = parse_issue(&r.body)?;
    let n = info.number;
    let title = a.title.trim();
    let mut out = format!("{repo}#{n} created ({title})\n");
    let (_item, lines) = adopt_info(ac, &world, &me, info, &repo, adoption.as_ref())?;
    out.push_str(&lines);
    if let Some(ad) = &adoption {
        out.push_str(&format!(
            "adopts-pr: {} branch: {} (open PR of Done ticket {})\n",
            ad.url, ad.branch, ad.parent
        ));
        if !crate::rules::text::closes_ticket(&ad.body, &ad.repo, &repo, n) {
            out.push_str(&format!(
                "note: the body of the PR does not close {repo}#{n} yet: the dev agent edits it to say 'Closes #{n}' before `done {n} --pr {}` (`done` refuses otherwise)\n",
                ad.url
            ));
        }
    }
    match a.assign {
        Some(agent) => out.push_str(&assign(
            ac,
            &format!("{repo}#{n}"),
            Some(agent),
            CapOverride::default(),
            None,
        )?),
        None => out.push_str("parked in Backlog; `crewboard assign` moves it to Ready\n"),
    }
    Ok(out)
}

/// `ticket adopt <issue> [--assign A]`: an issue that exists already.
pub fn ticket_adopt(
    ac: &mut AgentCtx,
    issue_arg: &str,
    assign_to: Option<&str>,
) -> CbResult<String> {
    let (repo, n) = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    check_repo(&ac.ctx.cfg, &repo)?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let info = fetch_issue(&ac.ctx.client, &repo, n)?;
    if let Some(agent) = assign_to {
        precheck_assign(ac, &world, agent, &info.title, &info.body)?;
    }
    let (_item, mut out) = adopt_info(ac, &world, &me, info, &repo, None)?;
    match assign_to {
        Some(agent) => out.push_str(&assign(
            ac,
            &format!("{repo}#{n}"),
            Some(agent),
            CapOverride::default(),
            None,
        )?),
        None => {
            out.push_str("kept in its Status; `crewboard assign` moves a Backlog ticket to Ready\n")
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_ci_writes_the_typed_gate_and_never_drops_another_condition() {
        // no Gate line: one is added
        let b = apply_gate_ci("Goal: x\nScope: y\n").unwrap();
        assert_eq!(b, "Goal: x\nScope: y\n\nGate: ci\n");
        assert_eq!(check_new_ticket("T", &b).unwrap(), "ci");
        assert_eq!(
            crate::headless::Gate::parse("ci"),
            crate::headless::Gate::Ci
        );
        // a CI only Gate in any letter case is replaced in place, a second one is dropped
        let b = apply_gate_ci("Goal: x\n**Gate:** CI\nScope: y\n- gate: ci\n").unwrap();
        assert_eq!(b, "Goal: x\nGate: ci\nScope: y\n");
        let b = apply_gate_ci("Goal: x\nGate: CI\n").unwrap();
        assert_eq!(b, "Goal: x\nGate: ci\n");
        // another Gate is refused
        for g in [
            "cargo test passes",
            "CI green and a manual test",
            "CI is green",
            "<required: one line that can fail>",
        ] {
            let e = apply_gate_ci(&format!("Goal: x\nGate: {g}\n")).unwrap_err();
            assert!(e.to_string().contains("ticket-gate-conflict"), "{e}");
            assert!(e.to_string().contains(g), "{e}");
        }
    }

    #[test]
    fn a_ticket_needs_a_title_and_a_gate_line() {
        // pass
        assert_eq!(
            check_new_ticket("T", "Goal.\n\n**Gate:** cargo test passes\n").unwrap(),
            "cargo test passes"
        );
        // refusals: no Gate line, an empty Gate, no title, a Gate over 1024 bytes
        let e = check_new_ticket("T", "Goal only.\n").unwrap_err();
        assert!(e.to_string().contains("status-gate-missing"), "{e}");
        assert!(check_new_ticket("T", "Gate:\n").is_err());
        let e = check_new_ticket("  ", "Gate: x\n").unwrap_err();
        assert!(e.to_string().contains("ticket-title-missing"), "{e}");
        let long = format!("Gate: {}\n", "x".repeat(1100));
        assert!(check_new_ticket("T", &long).is_err());
    }
}
