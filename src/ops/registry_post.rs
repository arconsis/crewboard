//! Two writes that several cycle duties share: a liveness event for the agent itself, and a
//! signed post to the registry issue. They live here so that `duties` and `log_check` do not
//! import each other.

use super::AgentCtx;
use super::say::send_signed;
use crate::clock::now_ts;
use crate::error::CbResult;
use crate::events::{Event, EventClass};
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
