//! The writes that several cycle duties share: a liveness event for the agent itself, a signed
//! post to the registry issue, and the report of a security event to the owner. They live here
//! so that `duties`, `log_check` and `tick` do not import each other.

use super::AgentCtx;
use super::say::send_signed;
use crate::audit::event_key;
use crate::clock::now_ts;
use crate::error::CbResult;
use crate::events::{Event, EventClass, SecurityEvent};
use crate::protocol::names::{Kind, Re, To};

/// A board-made liveness event for this agent (not a signed message).
pub(super) fn liveness_event(ac: &AgentCtx, kind: &str, body: String) -> Event {
    let cfg = &ac.ctx.cfg;
    Event {
        class: EventClass::Liveness,
        comment_id: 0,
        repo: cfg.main_repo().to_string(),
        issue: cfg.registry_issue,
        url: format!(
            "https://github.com/{}/issues/{}",
            cfg.main_repo(),
            cfg.registry_issue
        ),
        author: "board".into(),
        from: "board".into(),
        to: ac.name.to_string(),
        kind: kind.into(),
        re: "none".into(),
        ts: now_ts(),
        verified: false,
        body: crate::events::decode_html_entities(&body),
    }
}

/// Post one signed message to the registry issue.
pub(super) fn post_registry(
    ac: &AgentCtx,
    to: To,
    kind: Kind,
    re: Re,
    body: &str,
) -> CbResult<crate::ops::say::Posted> {
    send_signed(
        ac,
        ac.ctx.main_repo(),
        ac.ctx.cfg.registry_issue,
        to,
        kind,
        re,
        body,
        &now_ts(),
    )
}

/// The key (`event=` line) of the owner note on a board `State` that the registry log does not
/// support. The coordinator's own `stop --force` writes it into its notice, so that notice is
/// the report and the cycle does not repeat it.
pub(super) fn state_event_key(agent: &str, state: &str) -> String {
    format!("{agent}/state:{state}")
}

/// The security event as a signed owner message, once per event: `reported` holds the `event=`
/// keys that the registry issue shows already (`audit::reported_events`). `Some(url)` if it was
/// posted now.
pub(super) fn report_security(
    ac: &AgentCtx,
    s: &SecurityEvent,
    reported: &mut std::collections::HashSet<String>,
) -> CbResult<Option<String>> {
    let key = event_key(&s.repo, s.issue, s.comment_id, s.reason.as_str());
    if reported.contains(&key) {
        return Ok(None);
    }
    let body = format!(
        "Security event: {}\nevent={key}\nwhere: {}\nauthor: {}; claims from={}\ndetail: {}\n{}\n",
        s.reason.as_str(),
        s.url,
        s.author,
        s.claimed_from.as_deref().unwrap_or("(unreadable)"),
        s.detail,
        s.reason.advice()
    );
    let p = post_registry(ac, To::Owner, Kind::Info, Re::None, &body)?;
    reported.insert(key);
    Ok(Some(p.url))
}
