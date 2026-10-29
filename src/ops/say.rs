//! `say` (spec/cli#commands, spec/messages#format, spec/messages#secrets): post a signed message.
//!
//! Start of the message flow: secret scan and unread check, then header, canonical form and
//! signature (`protocol/`), then one comment through `feed.rs`. `ops/watch.rs` reads it back.

use super::{AgentCtx, World, load_world, parse_issue_arg};
use crate::clock::now_ts;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::protocol::names::{Kind, Re, Timestamp, To};
use crate::protocol::signature::{Outgoing, sign_message};
use crate::registry::TrustedAgent;
use crate::rules::status::Status;
use crate::rules::{Refusal, Role, limits};
use crate::secrets;
use crate::services::ticket_loader::load_ticket;
use std::io::Read;

/// Read the message body and run the checks that need no network: the secret scan
/// (spec/messages#secrets) and the size limit (spec/messages#format). `--body-file -` reads
/// standard input.
pub fn prepare_body(body_file: Option<&str>, body: Option<&str>) -> CbResult<String> {
    let text = match (body_file, body) {
        (Some(_), Some(_)) => {
            return Err(CbError::config(
                "use either --body or --body-file, not both",
            ));
        }
        (Some("-"), None) => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| CbError::config(format!("cannot read standard input: {e}")))?;
            s
        }
        (Some(path), None) => std::fs::read_to_string(path)
            .map_err(|e| CbError::config(format!("cannot read body file '{path}': {e}")))?,
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    };
    check_body_text(&text)?;
    Ok(text)
}

/// A free text must not read as an act (spec/messages#acts): `closed: ...`, `verify: pass`,
/// `Reassigned.` and the other act lines belong to one command each. Every writer of free text
/// (`say`, `block --why`) calls this.
pub fn check_free_text(kind: Kind, text: &str) -> CbResult<()> {
    match crate::protocol::acts::reserved_for(kind, text) {
        None => Ok(()),
        Some(cmd) => Err(Refusal::new(
            "text-reads-as-act",
            format!(
                "this {kind} text starts like a protocol act (a reader would take it for one); start with other words, or use the command that writes the act: {cmd}"
            ),
        )
        .into()),
    }
}

/// The two pre-network checks, also used by every other command that posts.
pub fn check_body_text(text: &str) -> CbResult<()> {
    limits::check_body(text.trim())?;
    if let Some(msg) = secrets::refusal_message(&secrets::scan(text)) {
        return Err(Refusal::new("secret-in-body", msg).into());
    }
    Ok(())
}

/// The agent must be approved, its span in the registry log open, and its item readable.
pub fn require_approved<'a>(world: &'a World, me: &str) -> CbResult<&'a TrustedAgent> {
    world.trust.get(me).ok_or_else(|| {
        let why = world
            .trust
            .problems
            .iter()
            .filter(|p| p.starts_with(&format!("{me}:")))
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let state = match world.reg.agents.get(me) {
            None => "no join request in the registry".to_string(),
            Some(e) if e.rejected => "the owner rejected it".to_string(),
            Some(e) if e.approval.is_none() => "the owner has not approved it yet".to_string(),
            Some(_) => format!("approved, but not trusted ({why})"),
        };
        CbError::refused(
            "sender-not-approved",
            format!("{me} is not an approved agent: {state}"),
        )
    })
}

/// Is a heartbeat due? Only an agent in a running state writes one, and only once per
/// interval. Pure, so the two directions can be tested.
pub fn heartbeat_wanted(
    state: Option<&str>,
    heartbeat: Option<&str>,
    now: i64,
    interval: i64,
) -> bool {
    state.is_some_and(crate::late::liveness::heartbeat_state)
        && crate::late::liveness::heartbeat_due(
            heartbeat.and_then(crate::clock::parse_ts),
            now,
            interval,
        )
}

/// `take`, `say` and `done` refresh the heartbeat when it is due. A headless wake
/// can run for a long time, and the scheduler does not tick while it runs: without this the
/// agent turns stale in the middle of its work, and `reassign --to` refuses it. A failed write
/// is only a warning: the command itself goes on. Returns true if a heartbeat was written.
pub fn refresh_heartbeat(ac: &AgentCtx, world: &World) -> bool {
    let item = world.items.get(&ac.agent.item_id);
    let now = crate::clock::now_epoch();
    if !heartbeat_wanted(
        item.and_then(|i| i.field("State")),
        item.and_then(|i| i.field("Heartbeat")),
        now,
        ac.ctx.hb_interval,
    ) {
        return false;
    }
    match ac.ctx.board.set_text(
        &ac.ctx.client,
        &ac.agent.item_id,
        "Heartbeat",
        &crate::clock::format_ts(now),
    ) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("crewboard: warning: the heartbeat was not written: {e}");
            false
        }
    }
}

pub struct Posted {
    pub id: u64,
    pub url: String,
    pub repo: String,
    pub issue: u64,
}

/// Sign and post. The caller has checked approval.
#[allow(clippy::too_many_arguments)]
pub fn send_signed(
    ac: &AgentCtx,
    repo: &str,
    issue: u64,
    to: To,
    kind: Kind,
    re: Re,
    body: &str,
    ts: &str,
) -> CbResult<Posted> {
    check_body_text(body)?;
    let issue_ref = crate::protocol::canonical::IssueRef::parse(&format!("{repo}#{issue}"))?;
    let msg = sign_message(
        &ac.key,
        &issue_ref,
        Outgoing {
            from: ac.name.clone(),
            to,
            kind,
            re,
            ts: Timestamp::parse(ts)?,
        },
        body,
    )?;
    let (id, url) = feed::post_comment(&ac.ctx.client, repo, issue, &msg.to_comment())?;
    Ok(Posted {
        id,
        url,
        repo: repo.to_string(),
        issue,
    })
}

pub struct SayArgs<'a> {
    pub issue: &'a str,
    pub to: To,
    pub kind: Kind,
    pub re: Option<&'a str>,
    pub body: String,
    /// Post although a message for the sender is unread on the ticket
    /// (spec/events#unread-messages-on-a-ticket).
    pub anyway: bool,
}

pub fn run(ac: &AgentCtx, a: SayArgs<'_>) -> CbResult<String> {
    check_body_text(&a.body)?;
    let re = match a.re {
        Some(r) => Re::parse(r)?,
        None => Re::None,
    };
    check_free_text(a.kind, &a.body)?;
    let (repo, issue) = parse_issue_arg(a.issue, ac.ctx.main_repo())?;
    if !ac.ctx.cfg.repos.contains(&repo) {
        return Err(CbError::refused(
            "repo-not-in-initiative",
            format!(
                "{repo} is not a repo of initiative {}",
                ac.ctx.cfg.initiative
            ),
        ));
    }
    let mut cache = ac.load_cache();
    let world = load_world(&ac.ctx, std::slice::from_ref(&ac.agent.item_id), &mut cache)?;
    ac.save_cache(&cache);
    require_approved(&world, ac.name.as_str())?;
    refresh_heartbeat(ac, &world);
    // spec/events#unread-messages-on-a-ticket: refuse while a message for the sender is unread on
    // this ticket.
    let (comments, _) =
        feed::issue_comments(&ac.ctx.client, &repo, issue, None, &ac.ctx.cfg.owner_login)?;
    let held = crate::unread::held_in_cache(&cache, &repo, issue, ac.name.as_str());
    let indexed = cache.index.tickets.contains_key(&format!("{repo}#{issue}"));
    let line = crate::unread::guard(ac, &world, &comments, &repo, issue, held, indexed, a.anyway)?;
    let body = crate::unread::with_override(&a.body, line.as_deref());
    check_body_text(&body)?;
    let note = crate::unread::note(line.as_deref());
    if let Some(w) = info_to_blocked_warning(ac, &world, &repo, issue, &a.to, a.kind, &body) {
        eprint!("{w}");
    }
    let p = send_signed(ac, &repo, issue, a.to.clone(), a.kind, re, &body, &now_ts())?;
    Ok(format!(
        "posted {} -> {} ({}) as comment {} in {}#{}\nref: #{}/c-{}\n{}\n{note}",
        ac.name, a.to, a.kind, p.id, p.repo, p.issue, p.issue, p.id, p.url
    ))
}

/// The warning of `say`: a coordinator sends `kind=info` to the assignee of a Blocked
/// ticket. An `info` does not unblock it. The same holds for a plain `kind=answer` when an open
/// question to the owner blocks the ticket: only an owner message or an answer that withdraws the
/// question unblocks it. A read that fails gives no warning: `say` goes on.
fn info_to_blocked_warning(
    ac: &AgentCtx,
    world: &World,
    repo: &str,
    issue: u64,
    to: &To,
    kind: Kind,
    body: &str,
) -> Option<String> {
    let To::Agent(name) = to else { return None };
    if !matches!(kind, Kind::Info | Kind::Answer)
        || world.trust.get(ac.name.as_str())?.role != Role::Coordinator
    {
        return None;
    }
    let t = load_ticket(&ac.ctx.reader(), &world.trust, repo, issue).ok()?;
    if t.status().ok()? != Status::Blocked || t.agent() != Some(name.as_str()) {
        return None;
    }
    if kind == Kind::Info {
        return Some(format!(
            "crewboard: warning: info does not unblock; use `crewboard unblock {issue}` or `--kind answer` ({} stays Blocked)\n",
            t.key()
        ));
    }
    let withdraws = body
        .lines()
        .any(|l| l.trim() == crate::protocol::acts::WITHDRAWS_QUESTION_LINE);
    (!withdraws && crate::ticket::blocked_by_owner_question(&t.msgs, t.agent())).then(|| {
        format!(
            "crewboard: warning: a plain answer does not unblock {}: it is blocked by a question to the owner; use `crewboard unblock {issue}` (an answer that withdraws the question)\n",
            t.key()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{cfg, join_comment, owner};
    use crate::registry::{build_trust, resolve};
    use std::collections::HashMap;

    fn token(prefix: &str) -> String {
        let t: String = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ"
            .chars()
            .cycle()
            .take(36)
            .collect();
        format!("{prefix}{t}")
    }

    #[test]
    fn a_body_with_a_secret_or_over_the_limit_is_refused_before_any_network() {
        let e = check_body_text(&format!("token {}", token("ghp_"))).unwrap_err();
        assert!(e.to_string().contains("secret-in-body"));
        assert!(!e.to_string().contains(&token("ghp_")));
        assert!(check_body_text(&"a".repeat(65_537)).is_err());
        // controls: clean text and exactly 65536 pass
        assert!(check_body_text("CI is green").is_ok());
        assert!(check_body_text(&"a".repeat(65_536)).is_ok());
    }

    #[test]
    fn a_heartbeat_is_written_when_due_and_not_when_fresh() {
        let now = crate::clock::parse_ts("2026-10-16T12:00:00Z").unwrap();
        let at = |s: i64| crate::clock::format_ts(now - s);
        // due: older than one interval, or none yet
        assert!(heartbeat_wanted(Some("working"), Some(&at(601)), now, 600));
        assert!(heartbeat_wanted(Some("active"), None, now, 600));
        assert!(heartbeat_wanted(Some("working"), Some("garbage"), now, 600));
        // fresh: nothing is written, so a busy agent does not write on every command
        assert!(!heartbeat_wanted(Some("working"), Some(&at(30)), now, 600));
        assert!(!heartbeat_wanted(Some("working"), Some(&at(599)), now, 600));
        // an agent that does not run writes none, even when the last one is old
        for st in ["requested", "approved", "left"] {
            assert!(
                !heartbeat_wanted(Some(st), Some(&at(9999)), now, 600),
                "{st}"
            );
        }
        assert!(!heartbeat_wanted(None, None, now, 600));
    }

    #[test]
    fn body_and_body_file_together_are_refused() {
        assert!(prepare_body(Some("x"), Some("y")).is_err());
        assert_eq!(prepare_body(None, Some("hi")).unwrap(), "hi");
        assert_eq!(prepare_body(None, None).unwrap(), "");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("b.txt");
        std::fs::write(&f, "from file").unwrap();
        assert_eq!(prepare_body(f.to_str(), None).unwrap(), "from file");
        assert!(prepare_body(Some("/nonexistent/x"), None).is_err());
    }

    #[test]
    fn an_unapproved_agent_may_not_say_and_the_reason_is_named() {
        let k = crate::protocol::signature::generate_key().unwrap();
        let reg = resolve(&cfg(), &[join_comment(1, "claude@a", &k, "PVTI_a")]);
        let world = World {
            trust: build_trust(&reg, &HashMap::new(), 15),
            reg,
            items: HashMap::new(),
            unreadable: vec![],
            registry_comments: vec![],
        };
        let e = require_approved(&world, "claude@a").unwrap_err();
        assert!(
            e.to_string().contains("not approved it yet")
                || e.to_string().contains("has not approved"),
            "{e}"
        );
        let e = require_approved(&world, "ghost@x").unwrap_err();
        assert!(e.to_string().contains("no join request"), "{e}");
        assert_eq!(e.exit(), crate::exit::Exit::Refused);
        // control: an approved and trusted agent passes
        let reg2 = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a"),
            ],
        );
        let mut items = HashMap::new();
        items.insert(
            "PVTI_a".to_string(),
            crate::items::ItemSnapshot {
                id: "PVTI_a".into(),
                updated_at: "2026-10-15T14:00:00Z".into(),
                item_type: "DRAFT_ISSUE".into(),
                project_number: Some(15),
                issue_state: None,
                fields: [
                    (
                        "Key".to_string(),
                        crate::registry::tests::pub_line(&k, "claude@a"),
                    ),
                    ("State".to_string(), "approved".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );
        let ok = World {
            trust: build_trust(&reg2, &items, 15),
            reg: reg2,
            items,
            unreadable: vec![],
            registry_comments: vec![],
        };
        assert!(require_approved(&ok, "claude@a").is_ok());
    }
}
