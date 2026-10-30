//! `agent selftest` (spec/messages#selftest): prove on the live board that this agent can see events.
//!
//! 1. Post a signed `kind=ping` from this agent to itself.
//! 2. Run one watch cycle and require exactly that event.
//! 3. Post a comment with a broken signature (signed with a throwaway key, claiming to be
//!    this agent) and require that the cycle refuses it as a security event and does not
//!    deliver it.
//! 4. Exit 0 only if both directions behaved.
//!
//! The cycle uses its own cursor and cache. It never moves the stored cursor, so real
//! events are not consumed. Both comments are deleted afterwards.

use super::say::{require_approved, send_signed};
use super::watch::collect;
use super::{AgentCtx, load_world};
use crate::base::ids;
use crate::clock::{now_epoch, now_ts};
use crate::cursor::Cursor;
use crate::error::{CbError, CbResult};
use crate::events::Reason;
use crate::feed;
use crate::github::Client;
use crate::protocol::names::{Kind, Re, Timestamp, To};
use crate::protocol::signature::{Outgoing, generate_key, sign_message};
use crate::state::StateCache;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

/// A random nonce: 24 hex digits from the hash of a fresh key.
fn new_nonce() -> CbResult<String> {
    let key = generate_key()?;
    let text = key
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(|e| CbError::config(format!("nonce: {e}")))?;
    let digest = Sha256::digest(text.as_bytes());
    Ok(digest.iter().take(12).map(|b| format!("{b:02x}")).collect())
}

pub const SELFTEST_LABEL: &str = "crew:selftest";

/// Find the open selftest issue of the main repo, or create it (and its label).
pub fn ensure_selftest_issue(client: &Client, repo: &str, initiative: &str) -> CbResult<u64> {
    let path = format!("repos/{repo}/issues?labels={SELFTEST_LABEL}&state=open&per_page=10");
    let r = client.get(&path)?;
    let list = r.body.as_array().ok_or_else(|| {
        CbError::from(crate::github::GhError::Shape(
            "issue list is not an array".into(),
        ))
    })?;
    if let Some(n) = list
        .iter()
        .filter(|i| i.get("pull_request").is_none())
        .find_map(|i| i.get("number").and_then(Value::as_u64))
    {
        return Ok(n);
    }
    match client.post(
        &format!("repos/{repo}/labels"),
        &json!({"name": SELFTEST_LABEL, "color": "ededed", "description": "crewboard selftest comments"}),
    ) {
        Ok(_) => {}
        Err(e) if crate::github::is_already_exists(&e) => {} // exists: reuse
        Err(e) => return Err(e.into()),
    }
    let r = client.post(
        &format!("repos/{repo}/issues"),
        &json!({
            "title": format!("Crew selftest - {initiative}"),
            "body": "crewboard selftest posts a signed ping and a forged comment here and deletes both. Do not comment.",
            "labels": [SELFTEST_LABEL],
        }),
    )?;
    r.body
        .get("number")
        .and_then(Value::as_u64)
        .ok_or_else(|| crate::github::GhError::Shape("created issue has no number".into()).into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelftestResult {
    pub ping_seen: bool,
    pub forged_refused: bool,
    /// The forged comment was wrongly delivered as a message.
    pub forged_delivered: bool,
}

impl SelftestResult {
    pub fn passed(&self) -> bool {
        self.ping_seen && self.forged_refused && !self.forged_delivered
    }
}

pub fn run(ac: &AgentCtx) -> CbResult<(SelftestResult, String)> {
    let mut scratch = StateCache::default();
    let me = ac.name.to_string();
    let world = load_world(
        &ac.ctx,
        std::slice::from_ref(&ac.agent.item_id),
        &mut scratch,
    )?;
    require_approved(&world, &me).map_err(|e| match e {
        CbError::Refused(r) => CbError::refused(
            ids::SELFTEST_NEEDS_APPROVAL,
            format!("selftest needs an approved agent: {}", r.message),
        ),
        other => other,
    })?;
    // The token and plain `gh` of this agent must act as the account that the registry bound
    // to it: the author of its join (spec/identity#identity-of-the-agents-own-gh).
    let account = world
        .reg
        .agents
        .get(&me)
        .and_then(|e| e.login())
        .unwrap_or_default()
        .to_string();
    super::judge_agent_account(&ac.ctx.client.viewer_login()?, &me, &account)?;
    let gh_note =
        crate::agent_gh::check(&ac.ctx.cfg, &account, &crate::agent_gh::ShellGh::from_env())?;
    let repo = ac.ctx.main_repo().to_string();
    let issue = ensure_selftest_issue(&ac.ctx.client, &repo, &ac.ctx.cfg.initiative)?;
    // The cursor starts before the posts, so the cycle sees them whatever the clock skew.
    let cursor = Cursor::at(now_epoch() - 30);
    let (ping_id, forged_id) = post_probes(ac, &repo, issue)?;
    let (result, detail) = poll_probes(ac, &mut scratch, &cursor, ping_id, forged_id)?;
    // Clean up our own comments. A failure here is reported, not hidden.
    let mut cleanup = String::new();
    // The forgery goes first: while it exists, the ping must still prove it
    // (spec/messages#selftest).
    for id in [forged_id, ping_id] {
        if let Err(e) = feed::delete_comment(&ac.ctx.client, &repo, id) {
            cleanup.push_str(&format!(" cleanup of comment {id} failed: {e};"));
        }
    }
    let line = result_line(&result, ping_id, forged_id, &detail, &cleanup);
    Ok((result, with_warnings(ac, line, &gh_note)))
}

/// Post the genuine ping and the forged comment. Returns the two comment ids (ping, forged).
fn post_probes(ac: &AgentCtx, repo: &str, issue: u64) -> CbResult<(u64, u64)> {
    // The nonce is signed in the genuine ping. Other agents quiet the forged comment only if it
    // carries the same nonce (spec/messages#selftest).
    let nonce = new_nonce()?;
    let ping = send_signed(
        ac,
        repo,
        issue,
        To::Agent(ac.name.clone()),
        Kind::Ping,
        Re::None,
        &format!("selftest ping selftest={nonce}"),
        &now_ts(),
    )?;
    // Forged: claims to be this agent, signed by a key that is not registered.
    let wrong = generate_key()?;
    let issue_ref = crate::protocol::canonical::IssueRef::parse(&format!("{repo}#{issue}"))?;
    let forged_msg = sign_message(
        &wrong,
        &issue_ref,
        Outgoing {
            from: ac.name.clone(),
            to: To::Agent(ac.name.clone()),
            kind: Kind::Pong,
            re: Re::None,
            ts: Timestamp::parse(&now_ts())?,
        },
        &format!("selftest forged message (wrong key) selftest={nonce}"),
    )?;
    let (forged_id, _) = feed::post_comment(&ac.ctx.client, repo, issue, &forged_msg.to_comment())?;
    Ok((ping.id, forged_id))
}

/// Run watch cycles until both comments were judged or 40 s have passed. Returns the verdict and
/// the detail text for a timeout.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a poll against the live board"
)]
fn poll_probes(
    ac: &AgentCtx,
    scratch: &mut StateCache,
    cursor: &Cursor,
    ping_id: u64,
    forged_id: u64,
) -> CbResult<(SelftestResult, String)> {
    let mut result = SelftestResult {
        ping_seen: false,
        forged_refused: false,
        forged_delivered: false,
    };
    let mut detail = String::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let w = load_world(&ac.ctx, std::slice::from_ref(&ac.agent.item_id), scratch)?;
        let c = collect(ac, &w, cursor, scratch, false)?;
        result.ping_seen = c
            .events
            .iter()
            .any(|e| e.comment_id == ping_id && e.kind == "ping" && e.verified);
        result.forged_delivered = c.events.iter().any(|e| e.comment_id == forged_id);
        // This agent sees its own forgery as bad-signature; the nonce rule only quiets it for
        // OTHER agents' reports, so both verdicts prove that the signature check fired.
        result.forged_refused = c
            .security
            .iter()
            .any(|s| s.comment_id == forged_id && s.reason == Reason::BadSignature)
            || c.selftest_forgeries
                .iter()
                .any(|s| s.comment_id == forged_id);
        if (result.ping_seen && result.forged_refused) || result.forged_delivered {
            break;
        }
        if Instant::now() >= deadline {
            detail = format!(
                " (feed did not show both comments within 40 s; seen {} event(s), {} security event(s))",
                c.events.len(),
                c.security.len()
            );
            break;
        }
        crate::clock::pause(Duration::from_secs(2));
    }
    Ok((result, detail))
}

/// The first line of the report: the verdict, the two comments, the timeout detail and the
/// cleanup failures.
fn result_line(
    result: &SelftestResult,
    ping_id: u64,
    forged_id: u64,
    detail: &str,
    cleanup: &str,
) -> String {
    format!(
        "selftest {}: ping to self {} (comment {ping_id}); forged message {} (comment {forged_id}){detail}{cleanup}",
        if result.passed() { "PASS" } else { "FAIL" },
        if result.ping_seen { "seen" } else { "NOT seen" },
        if result.forged_delivered {
            "WRONGLY DELIVERED".to_string()
        } else if result.forged_refused {
            "refused as bad-signature".to_string()
        } else {
            "NOT reported".to_string()
        },
    )
}

/// Add the warnings that `start` shows too: contradicting authority keys, CI that does not run
/// for crew branches, built-in workflows that write Status, and the note about the agent's `gh`.
fn with_warnings(ac: &AgentCtx, line: String, gh_note: &str) -> String {
    // AUTHORITY.md keys that contradict each other (spec/board#contradicting-keys-in-authoritymd).
    // `start` runs this too.
    let line = match crate::ctx::load_authority_quiet(&ac.ctx) {
        Some(a) if !a.contradictions().is_empty() => {
            format!("{line}\n{}", a.contradiction_warnings().trim_end())
        }
        _ => line,
    };
    // CI must run for crew branches (spec/ci#ci-must-run-for-crew-branches). `start` runs this too.
    let prefix = crate::ctx::load_authority_quiet(&ac.ctx)
        .unwrap_or_default()
        .branch_prefix();
    let ci_warn =
        crate::services::ci_triggers::check_repos(&ac.ctx.client, &ac.ctx.cfg.repos, &prefix);
    let line = if ci_warn.is_empty() {
        line
    } else {
        format!("{line}\n{}", ci_warn.trim_end())
    };
    // Built-in project workflows that write Status behind crewboard
    // (spec/board#built-in-project-workflows). `init` and `doctor --board` run this too.
    let wf_warn = crate::builtin_workflows::check(&ac.ctx.client, &ac.ctx.board);
    let line = if wf_warn.is_empty() {
        line
    } else {
        format!("{line}\n{}", wf_warn.trim_end())
    };
    if gh_note.is_empty() {
        line
    } else {
        format!("{line}; {}", gh_note.trim_end().replace('\n', " "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes_only_if_both_directions_behaved() {
        let ok = SelftestResult {
            ping_seen: true,
            forged_refused: true,
            forged_delivered: false,
        };
        assert!(ok.passed());
        // each wrong combination must fail
        for r in [
            SelftestResult {
                ping_seen: false,
                forged_refused: true,
                forged_delivered: false,
            },
            SelftestResult {
                ping_seen: true,
                forged_refused: false,
                forged_delivered: false,
            },
            SelftestResult {
                ping_seen: true,
                forged_refused: true,
                forged_delivered: true,
            },
            SelftestResult {
                ping_seen: false,
                forged_refused: false,
                forged_delivered: false,
            },
        ] {
            assert!(!r.passed(), "{r:?}");
        }
    }
}
