//! `ticket split` (coordinator): move the open part of an In Progress
//! or Blocked ticket to a new ticket for the same agent, so that the finished part
//! can merge.
//!
//! One ticket is one PR, and `merge` needs a verified ticket. A ticket with several
//! independent causes would block its finished part. The split keeps the old ticket in its
//! status. The agent runs `done` on it with the finished PR; a Blocked ticket
//! needs no `take` for that (`ticket::split_off_proven`).
//!
//! Order: pure checks, the writer, the authority, the old ticket, the assignment checks (so
//! that a refusal creates nothing), then the writes: new issue, link line in the old body,
//! board item and queue entry, one signed note on the old ticket. Every write is idempotent:
//! a split that failed halfway is repaired by running the same command again.

use super::assign::assign;
use super::assign_checks::CapOverride;
use super::new::{adopt_info, check_new_ticket, precheck_assign};
use super::{Me, load_world_cached, me_of, post, to_agent};
use crate::error::{CbError, CbResult};
use crate::ops::say::prepare_body;
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::names::{Kind, Re};
use crate::rules::fields::Field;
use crate::rules::ids;
use crate::rules::status::{Status, Transition, check_status_transition};
use crate::services::ticket_loader::{
    IssueInfo, Ticket, check_repo, fetch_issue, load_ticket, parse_issue,
};
use crate::ticket::{WORK_LABEL, body_field};
use crate::ticket::{is_split_note, split_line, split_note, split_off_lines};

/// The ticket that an earlier split with this title made: the `Split off:` line of the old
/// body with the same title. Another title is another split.
fn earlier_split(old_body: &str, title: &str) -> Option<String> {
    split_off_lines(old_body)
        .into_iter()
        .find_map(|(key, t)| (t == title.trim()).then_some(key))
}

/// The text of the new ticket: the body file, a `Gate:` line copied from the old ticket unless
/// the file has its own, and the link to the old ticket.
fn new_body(file_body: &str, old_gate: Option<&str>, old_key: &str) -> String {
    let mut b = file_body.trim_end().to_string();
    if body_field(&b, "Gate").is_none()
        && let Some(g) = old_gate
    {
        b.push_str(&format!("\n\nGate: {g}"));
    }
    b.push_str(&format!("\n\nSplit from: {old_key}\n"));
    b
}

/// What the checks of `ticket split` found: everything the writes need.
struct SplitPlan {
    repo: String,
    n: u64,
    world: World,
    me: Me,
    old: Ticket,
    old_key: String,
    agent: Option<String>,
    title: String,
    body: String,
    /// the new ticket of an earlier run, if there is one
    existing: Option<u64>,
    /// the old body has the link line already
    linked: bool,
}

/// What the writes did now and what an earlier run did.
#[derive(Default)]
struct Done {
    now: Vec<String>,
    before: Vec<String>,
}

fn no_agent(old_key: &str) -> CbError {
    CbError::refused(
        "split-no-agent",
        format!("{old_key} has no Agent; there is nobody to queue the new ticket for"),
    )
}

/// `ticket split <issue> --title T --body-file F`.
pub fn ticket_split(
    ac: &mut AgentCtx,
    issue_arg: &str,
    title: &str,
    body_file: &str,
) -> CbResult<String> {
    let plan = plan_split(ac, issue_arg, title, body_file)?;
    perform_split(ac, &plan)
}

/// The pure checks, the writer, the authority, the old ticket and the assignment checks. Nothing
/// is written.
fn plan_split(
    ac: &mut AgentCtx,
    issue_arg: &str,
    title: &str,
    body_file: &str,
) -> CbResult<SplitPlan> {
    let file_body = prepare_body(Some(body_file), None)?;
    let title = title.trim();
    let (repo, n) = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    check_repo(&ac.ctx.cfg, &repo)?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    // writer first: only the coordinator splits
    check_status_transition(None, Status::Backlog, me.actor, &Transition::Create)?;
    crate::ctx::load_authority(&ac.ctx)?.check_split_ticket()?;
    let old = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    if !old.info.open {
        return Err(CbError::refused(
            "ticket-closed",
            format!("{repo}#{n} is closed"),
        ));
    }
    let old_key = old.key();
    let agent = old.agent().map(str::to_string);
    // A rerun repairs a split that failed halfway: reuse the ticket of the earlier run (the
    // link line in the old body, else an open issue with the same title and back link).
    let linked = earlier_split(&old.info.body, title)
        .and_then(|k| k.rsplit_once('#').and_then(|(_, m)| m.parse::<u64>().ok()));
    let existing = match linked {
        Some(m) => Some(m),
        None => find_split_issue(ac, &repo, &old_key, title)?,
    };
    if existing.is_none() {
        let status = old.status()?;
        if !matches!(status, Status::InProgress | Status::Blocked) {
            return Err(CbError::refused(
                "split-not-in-progress",
                format!(
                    "{old_key} is {}; only an In Progress or Blocked ticket is split",
                    status.name()
                ),
            ));
        }
    }
    if existing.is_none() && agent.is_none() {
        return Err(no_agent(&old_key));
    }
    let body = new_body(
        &file_body,
        body_field(&old.info.body, "Gate").as_deref(),
        &old_key,
    );
    check_new_ticket(title, &body)?;
    if let Some(a) = &agent {
        precheck_assign(ac, &world, a, title, &body)?;
    }
    Ok(SplitPlan {
        repo,
        n,
        world,
        me,
        old,
        old_key,
        agent,
        title: title.to_string(),
        body,
        existing,
        linked: linked.is_some(),
    })
}

/// The writes: new issue, link line in the old body, board item and queue entry, one signed
/// note on the old ticket.
fn perform_split(ac: &mut AgentCtx, p: &SplitPlan) -> CbResult<String> {
    let mut out = String::new();
    let mut done = Done::default();
    let info = ensure_new_issue(ac, p, &mut done)?;
    let m = info.number;
    let new_key = format!("{}#{m}", p.repo);
    link_old_body(ac, p, &new_key, &mut done)?;
    // (b) on the board, and in the queue of the agent
    let (item_id, lines) = adopt_info(ac, &p.world, &p.me, info, &p.repo, None)?;
    out.push_str(&lines);
    queue_new_ticket(ac, p, &item_id, &new_key, &mut out, &mut done)?;
    post_split_note(ac, p, &new_key, &mut out, &mut done)?;
    let list = |v: &[String]| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(", ")
        }
    };
    let old_key = &p.old_key;
    out.push_str(&format!(
        "split: {old_key} -> {new_key}; now: {}; already done: {}; {old_key} stays {}\n",
        list(&done.now),
        list(&done.before),
        p.old.status()?.name()
    ));
    Ok(out)
}

/// The new issue: the one of an earlier run, or a new one.
fn ensure_new_issue(ac: &AgentCtx, p: &SplitPlan, done: &mut Done) -> CbResult<IssueInfo> {
    Ok(match p.existing {
        Some(m) => {
            let i = fetch_issue(&ac.ctx.client, &p.repo, m)?;
            done.before.push("exists".into());
            i
        }
        None => {
            let r = ac.ctx.client.post(
                &format!("repos/{}/issues", p.repo),
                &serde_json::json!({"title": p.title, "body": p.body, "labels": [WORK_LABEL]}),
            )?;
            done.now.push("created".into());
            parse_issue(&r.body)?
        }
    })
}

/// (a) the link in the old body: one appended line, nothing else changed.
fn link_old_body(ac: &AgentCtx, p: &SplitPlan, new_key: &str, done: &mut Done) -> CbResult<()> {
    if p.linked {
        done.before.push("linked".into());
        return Ok(());
    }
    let mut old_body = p.old.info.body.clone();
    if !old_body.is_empty() && !old_body.ends_with('\n') {
        old_body.push('\n');
    }
    old_body.push_str(&format!("\n{}\n", split_line(new_key, &p.title)));
    ac.ctx.client.send(
        "PATCH",
        &format!("repos/{}/issues/{}", p.repo, p.n),
        Some(&serde_json::json!({"body": old_body})),
    )?;
    done.now.push("linked".into());
    Ok(())
}

/// (b) the queue entry: assign the new ticket to the agent unless it is past Ready already.
fn queue_new_ticket(
    ac: &mut AgentCtx,
    p: &SplitPlan,
    item_id: &str,
    new_key: &str,
    out: &mut String,
    done: &mut Done,
) -> CbResult<()> {
    let status = ac
        .ctx
        .board
        .read_item(&ac.ctx.client, item_id)?
        .get(Field::Status)
        .and_then(Status::parse_board);
    let queued = if matches!(status, Some(Status::Backlog | Status::Ready) | None) {
        let agent = p.agent.as_deref().ok_or_else(|| no_agent(&p.old_key))?;
        match assign(ac, new_key, Some(agent), CapOverride::default(), None) {
            Ok(text) => {
                out.push_str(&text);
                true
            }
            Err(CbError::Refused(r)) if r.rule == ids::TICKET_ALREADY_ASSIGNED => false,
            Err(e) => return Err(e),
        }
    } else {
        false
    };
    if queued {
        done.now.push("queued".into());
    } else {
        done.before.push("queued".into());
    }
    Ok(())
}

/// (c) the signed note on the old ticket.
fn post_split_note(
    ac: &AgentCtx,
    p: &SplitPlan,
    new_key: &str,
    out: &mut String,
    done: &mut Done,
) -> CbResult<()> {
    let old_key = &p.old_key;
    let noted = p.agent.as_deref().is_some_and(|a| {
        p.old
            .msgs
            .iter()
            .any(|v| is_split_note(v, a, old_key, new_key))
    });
    if noted {
        done.before.push("note posted".into());
        return Ok(());
    }
    let agent = p.agent.as_deref().ok_or_else(|| no_agent(old_key))?;
    let posted = post(
        ac,
        &p.old,
        to_agent(agent)?,
        Kind::Info,
        Re::None,
        &split_note(old_key, new_key, &p.title),
    )?;
    out.push_str(&format!("note {} posted ({})\n", posted.id, posted.url));
    done.now.push("note posted".into());
    Ok(())
}

/// An open issue of the repo that an earlier, failed split made: the same title and the line
/// `Split from: <old>` in its body. Returns the lowest number.
fn find_split_issue(
    ac: &AgentCtx,
    repo: &str,
    old_key: &str,
    title: &str,
) -> CbResult<Option<u64>> {
    let r = ac.ctx.client.get(&format!(
        "repos/{repo}/issues?state=open&labels=crew%3Awork&per_page=100"
    ))?;
    let list = r.body.as_array().ok_or_else(|| {
        CbError::config(
            "the issue list of GitHub is not a JSON array; cannot look for an earlier split",
        )
    })?;
    let back = format!("Split from: {old_key}");
    Ok(list
        .iter()
        .filter(|i| i.get("pull_request").is_none())
        .filter(|i| i["title"].as_str().map(str::trim) == Some(title.trim()))
        .filter(|i| {
            i["body"]
                .as_str()
                .is_some_and(|b| b.lines().any(|l| l.trim() == back))
        })
        .filter_map(|i| i["number"].as_u64())
        .min())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_earlier_split_is_found_by_its_title_only() {
        let body = format!(
            "Goal.\n\nGate: x\n\n{}\n",
            split_line("a/b#7", "Fix flaky B")
        );
        // pass: the same title finds the earlier ticket, also with spaces around
        assert_eq!(
            earlier_split(&body, " Fix flaky B ").as_deref(),
            Some("a/b#7")
        );
        // refusals: another title, and a body without a link line
        assert_eq!(earlier_split(&body, "Fix flaky C"), None);
        assert_eq!(earlier_split("Goal.\nGate: x\n", "Fix flaky B"), None);
    }

    #[test]
    fn the_new_body_keeps_its_own_gate_or_copies_the_old_one_and_links_back() {
        let copied = new_body("Open part.\n", Some("CI green"), "a/b#3");
        assert_eq!(body_field(&copied, "Gate").as_deref(), Some("CI green"));
        assert!(copied.ends_with("Split from: a/b#3\n"), "{copied}");
        let own = new_body("Open part.\n**Gate:** demo\n", Some("CI green"), "a/b#3");
        assert_eq!(body_field(&own, "Gate").as_deref(), Some("demo"));
        assert!(!own.contains("CI green"), "{own}");
    }
}
