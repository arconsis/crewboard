//! The audit trail of profile changes (spec/profiles#who-may-change-a-profile-and-the-history).
//! Pure.
//!
//! A change by the agent itself is a signed `kind=info` message in the registry issue whose
//! body starts with `Profile change.`; a change by the owner is the owner comment
//! `/crew profile <name> ...` (an owner message, no header). [`history`] reads both back
//! from the registry comments. Only valid messages count: a forged or edited one is not part
//! of the history, for the same reason it is not delivered (spec/messages#signature).

use crate::capabilities::{Applied, Change};
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::owner_cmd::{self, OwnerCommand};
use crate::protocol::names::Kind;
use crate::registry::Trust;
use crate::ticket::{Sender, verified_messages};

pub const BODY_HEAD: &str = "Profile change.";

/// What a change message says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Record {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// `Some("high")`, or `Some("none")` for a cleared tier; `None` if the tier did not change.
    pub tier: Option<String>,
    pub about_changed: bool,
    /// All declared tags after the change.
    pub tags: Vec<String>,
}

fn join(v: &[String]) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        v.join(",")
    }
}

fn split(v: &str) -> Vec<String> {
    if v == "-" {
        Vec::new()
    } else {
        v.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// The body of the signed message that an agent posts for its own change.
pub fn change_body(a: &Applied) -> String {
    let tier = if a.tier_changed {
        format!("tier={}\n", a.caps.tier.as_deref().unwrap_or("none"))
    } else {
        String::new()
    };
    format!(
        "{BODY_HEAD}\nby=agent\nadded={}\nremoved={}\n{tier}about={}\ntags={}\n",
        join(&a.added),
        join(&a.removed),
        if a.about_changed {
            "changed"
        } else {
            "unchanged"
        },
        join(&a.caps.declared.iter().cloned().collect::<Vec<_>>()),
    )
}

pub fn parse_change_body(body: &str) -> Option<Record> {
    let mut lines = body.lines();
    if lines.next()?.trim() != BODY_HEAD {
        return None;
    }
    let mut r = Record::default();
    let mut by = None;
    for l in lines {
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k {
            "by" => by = Some(v.to_string()),
            "added" => r.added = split(v),
            "removed" => r.removed = split(v),
            "tier" => r.tier = Some(v.to_string()),
            "about" => r.about_changed = v == "changed",
            "tags" => r.tags = split(v),
            _ => {}
        }
    }
    (by.as_deref() == Some("agent")).then_some(r)
}

/// One line of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub comment_id: u64,
    /// `agent` or `owner`.
    pub by: &'static str,
    pub summary: String,
}

fn summary(added: &[String], removed: &[String], tier: Option<&str>, about: bool) -> String {
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("+{}", added.join(" +")));
    }
    if !removed.is_empty() {
        parts.push(format!("-{}", removed.join(" -")));
    }
    if let Some(t) = tier {
        parts.push(format!("tier={t}"));
    }
    if about {
        parts.push("background text".to_string());
    }
    if parts.is_empty() {
        "no change".to_string()
    } else {
        parts.join(" ")
    }
}

/// The valid profile changes of `agent` in the registry comments, oldest first.
pub fn history(comments: &[Comment], cfg: &BoardConfig, trust: &Trust, agent: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for m in verified_messages(comments, cfg, trust) {
        match &m.sender {
            Sender::Agent { name, .. } if name == agent && m.kind == Some(Kind::Info) => {
                if let Some(r) = parse_change_body(&m.body) {
                    out.push(Entry {
                        comment_id: m.comment_id,
                        by: "agent",
                        summary: summary(&r.added, &r.removed, r.tier.as_deref(), r.about_changed),
                    });
                }
            }
            Sender::Owner => {
                if let Ok(Some(cmd)) = owner_cmd::parse(&m.body)
                    && let OwnerCommand::Profile { name, .. } = &cmd
                    && name.as_str() == agent
                    && let Some(c) = owner_cmd::profile_change(&cmd, &m.body)
                {
                    out.push(owner_entry(m.comment_id, &c));
                }
            }
            _ => {}
        }
    }
    out
}

fn owner_entry(comment_id: u64, c: &Change) -> Entry {
    let tier = match (&c.tier, c.clear_tier) {
        (Some(t), _) => Some(t.as_str()),
        (None, true) => Some("none"),
        _ => None,
    };
    Entry {
        comment_id,
        by: "owner",
        summary: summary(&c.tags, &c.untags, tier, c.about.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{Caps, apply_change};
    use crate::comments::tests::comment;
    use crate::items::ItemSnapshot;
    use crate::protocol::names::Kind;
    use crate::protocol::signature::generate_key;
    use crate::registry::tests::{cfg, join_comment, owner, signed};
    use crate::registry::{build_trust, resolve};
    use std::collections::{BTreeMap, HashMap};

    fn applied() -> Applied {
        apply_change(
            &Caps::parse("gpu arch:arm64"),
            "",
            &Change {
                tags: vec!["e2e".into()],
                untags: vec!["gpu".into()],
                tier: Some("high".into()),
                about: Some("text".into()),
                ..Change::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn a_change_body_round_trips_and_a_foreign_body_is_not_a_record() {
        let a = applied();
        let r = parse_change_body(&change_body(&a)).unwrap();
        assert_eq!(r.added, ["e2e"]);
        assert_eq!(r.removed, ["gpu"]);
        assert_eq!(r.tier.as_deref(), Some("high"));
        assert!(r.about_changed);
        assert_eq!(r.tags, ["e2e"]);
        // empty lists and an unchanged tier
        let none = apply_change(
            &Caps::default(),
            "",
            &Change {
                tags: vec!["a".into()],
                ..Change::default()
            },
        )
        .unwrap();
        let r = parse_change_body(&change_body(&none)).unwrap();
        assert_eq!((r.removed.len(), r.tier, r.about_changed), (0, None, false));
        // other bodies and a body that claims another author type are no record
        assert_eq!(parse_change_body("Resumed."), None);
        assert_eq!(parse_change_body("Profile change.\nby=owner\n"), None);
        assert_eq!(parse_change_body(""), None);
    }

    fn trusted_world() -> (BoardConfig, Trust, ssh_key::PrivateKey) {
        let k = generate_key().unwrap();
        let c = cfg();
        let comments = [
            join_comment(1, "claude@a", &k, "PVTI_a"),
            owner(2, "/crew approve claude@a role=dev"),
        ];
        let reg = resolve(&c, &comments);
        let mut fields = BTreeMap::new();
        fields.insert(
            "Key".to_string(),
            crate::registry::tests::pub_line(&k, "claude@a"),
        );
        fields.insert("State".to_string(), "active".to_string());
        let item = ItemSnapshot {
            id: "PVTI_a".into(),
            updated_at: "2026-10-15T14:00:00Z".into(),
            item_type: "DRAFT_ISSUE".into(),
            project_number: Some(15),
            issue_state: None,
            fields,
        };
        let mut m = HashMap::new();
        m.insert("PVTI_a".to_string(), item);
        (c.clone(), build_trust(&reg, &m, 15), k)
    }

    #[test]
    fn the_history_lists_signed_agent_changes_and_owner_changes_in_order() {
        let (c, trust, k) = trusted_world();
        let agent_msg = signed(
            10,
            "owner-login",
            &k,
            "claude@a",
            "coordinator",
            Kind::Info,
            &change_body(&applied()),
        );
        let owner_msg = owner(11, "/crew profile claude@a +gpu tier=none");
        let other_agent = owner(12, "/crew profile hermes@b +gpu");
        let h = history(&[owner_msg, other_agent, agent_msg], &c, &trust, "claude@a");
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!((h[0].comment_id, h[0].by), (10, "agent"));
        assert_eq!(h[0].summary, "+e2e -gpu tier=high background text");
        assert_eq!((h[1].comment_id, h[1].by), (11, "owner"));
        assert_eq!(h[1].summary, "+gpu tier=none");
    }

    #[test]
    fn forged_edited_and_foreign_changes_are_not_history() {
        let (c, trust, k) = trusted_world();
        let body = change_body(&applied());
        // signed by another key than the agent's
        let evil = generate_key().unwrap();
        let forged = signed(
            10,
            "owner-login",
            &evil,
            "claude@a",
            "coordinator",
            Kind::Info,
            &body,
        );
        // edited after posting
        let mut edited = signed(
            11,
            "owner-login",
            &k,
            "claude@a",
            "coordinator",
            Kind::Info,
            &body,
        );
        edited.updated = "2026-10-15T15:00:00Z".into();
        // a stranger's `/crew profile`
        let stranger = comment(12, "mallory", "/crew profile claude@a +gpu");
        // an agent that is not trusted
        let nobody = generate_key().unwrap();
        let untrusted = signed(
            13,
            "owner-login",
            &nobody,
            "ghost@x",
            "coordinator",
            Kind::Info,
            &body,
        );
        // control: the valid one is there
        let good = signed(
            14,
            "owner-login",
            &k,
            "claude@a",
            "coordinator",
            Kind::Info,
            &body,
        );
        let h = history(
            &[forged, edited, stranger, untrusted, good],
            &c,
            &trust,
            "claude@a",
        );
        assert_eq!(h.len(), 1, "{h:?}");
        assert_eq!(h[0].comment_id, 14);
        // an agent cannot write the history of another agent
        assert!(history(&[], &c, &trust, "claude@a").is_empty());
    }
}
