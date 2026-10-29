//! Issue comments as read from the REST feed (spec/events#polling-and-rate-limits).
//!
//! The plain comment type. `feed.rs` fills it from GitHub; `events.rs` and `registry.rs` read it.

use crate::base::GhError;
use crate::clock::parse_ts;
use crate::protocol::canonical::IssueRef;
use crate::protocol::header::Message;
use crate::protocol::replay::{Replay, check_window};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    /// `owner/repo`.
    pub repo: String,
    pub issue: u64,
    /// GitHub login of the author.
    pub author: String,
    pub created: String,
    pub updated: String,
    pub created_epoch: i64,
    pub updated_epoch: i64,
    pub body: String,
    pub url: String,
    /// GraphQL node id (`node_id` of the REST comment); empty when the feed lacks it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// Who edited the comment: the logins of its edits. Read lazily, only for an edited
    /// comment of the owner (`feed::resolve_owner_edits`); `None` = not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editors: Option<Vec<String>>,
}

/// The editor that stands for "GitHub did not name one" (deleted account, history cut off).
pub const UNKNOWN_EDITOR: &str = "(unknown)";

impl Comment {
    /// Build from one element of `GET /repos/{r}/issues/comments`. Every field that the
    /// logic needs is checked; a missing one is a shape error, never a default.
    pub fn from_json(v: &Value, repo: &str) -> Result<Self, GhError> {
        let shape = |m: &str| GhError::Shape(format!("comment: {m}"));
        let s = |k: &str| -> Result<String, GhError> {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| shape(&format!("`{k}` missing")))
        };
        let id = v
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| shape("`id` missing"))?;
        let issue_url = s("issue_url")?;
        let issue = issue_url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| shape("`issue_url` has no issue number"))?;
        let created = s("created_at")?;
        let updated = s("updated_at")?;
        // A deleted user has `user: null`; the author is then unknown, never trusted.
        let author = v
            .pointer("/user/login")
            .and_then(Value::as_str)
            .unwrap_or("(unknown)")
            .to_string();
        Ok(Comment {
            id,
            repo: repo.to_string(),
            issue,
            author,
            created_epoch: parse_ts(&created).ok_or_else(|| shape("bad created_at"))?,
            updated_epoch: parse_ts(&updated).ok_or_else(|| shape("bad updated_at"))?,
            created,
            updated,
            body: v
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            url: v
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            node_id: v
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            editors: None,
        })
    }

    /// A comment with `updated_at != created_at` was edited (spec/events#polling-and-rate-limits).
    pub fn edited(&self) -> bool {
        self.updated != self.created
    }

    /// An owner comment is trusted when it was never edited, or when every edit was made by
    /// the owner (spec/security-details#edited-owner-comments). `None`: trusted. `Some(editor)`: the
    /// first editor who is not the owner, or [`UNKNOWN_EDITOR`] when the editors were not read
    /// (fail closed). Call it for comments of the owner only.
    pub fn untrusted_owner_edit(&self, owner: &str) -> Option<String> {
        if !self.edited() {
            return None;
        }
        match &self.editors {
            Some(list) if !list.is_empty() => list
                .iter()
                .find(|e| !e.eq_ignore_ascii_case(owner))
                .cloned(),
            _ => Some(UNKNOWN_EDITOR.to_string()),
        }
    }

    /// Does the body start with the crew header? Such a comment is judged by its signature.
    pub fn has_crew_header(&self) -> bool {
        self.body.starts_with(crate::protocol::header::PREFIX)
    }

    /// Must the feed read who edited this comment? Yes for an edited comment of the owner
    /// without a crew header, as long as the editors are not known.
    pub fn needs_editor_read(&self, owner: &str) -> bool {
        self.edited()
            && self.editors.is_none()
            && !self.node_id.is_empty()
            && !self.has_crew_header()
            && self.by(owner)
    }

    /// The replay rule for a signed message that this comment carries: the signed `ts` must be
    /// close to the GitHub `created_at` of the comment ([`crate::protocol::replay::check_window`]). Call it after
    /// the signature checked out.
    pub fn check_ts(&self, msg: &Message) -> Result<(), Replay> {
        // `Timestamp` is validated on parse, so the time always reads; a failure is a refusal.
        let ts = parse_ts(msg.header.ts.as_str()).unwrap_or(i64::MIN / 2);
        check_window(ts, self.created_epoch)
    }

    pub fn issue_ref(&self) -> IssueRef {
        let (o, r) = self.repo.split_once('/').unwrap_or((&self.repo, ""));
        IssueRef::new(o, r, self.issue)
    }

    /// Verify a message that this comment carries. The signature is checked under the current
    /// repo name; only if that fails, under each former name that AUTHORITY.md lists and whose
    /// cutoff is after the signed `ts` (spec/messages#repository-moves).
    ///
    /// `former` is the list for this comment's repo (`BoardConfig::former_for`); the check reads
    /// nothing else.
    pub fn verify(
        &self,
        msg: &crate::protocol::header::Message,
        key: &ssh_key::PublicKey,
        former: &[crate::protocol::former::FormerRepo],
    ) -> Result<(), crate::protocol::ProtocolError> {
        crate::protocol::former::verify_with_former(msg, &self.issue_ref(), former, key)
    }

    pub fn by(&self, login: &str) -> bool {
        self.author.eq_ignore_ascii_case(login)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn comment(id: u64, author: &str, body: &str) -> Comment {
        Comment {
            id,
            repo: "example-org/app".into(),
            issue: 2,
            author: author.into(),
            created: "2026-10-15T14:00:00Z".into(),
            updated: "2026-10-15T14:00:00Z".into(),
            created_epoch: 1_792_072_800,
            updated_epoch: 1_792_072_800,
            body: body.into(),
            url: format!("https://github.com/example-org/app/issues/2#issuecomment-{id}"),
            node_id: format!("IC_{id}"),
            editors: None,
        }
    }

    fn json_comment() -> Value {
        json!({
            "id": 99,
            "issue_url": "https://api.github.com/repos/example-org/app/issues/2",
            "html_url": "https://github.com/example-org/app/issues/2#issuecomment-99",
            "user": {"login": "owner-login"},
            "created_at": "2026-10-15T14:00:00Z",
            "updated_at": "2026-10-15T14:00:00Z",
            "body": "hello"
        })
    }

    #[test]
    fn parses_a_feed_element() {
        let c = Comment::from_json(&json_comment(), "example-org/app").unwrap();
        assert_eq!((c.id, c.issue), (99, 2));
        assert_eq!(c.author, "owner-login");
        assert!(!c.edited());
        assert_eq!(c.issue_ref().to_string(), "example-org/app#2");
        assert!(c.by("Owner-Login"));
    }

    #[test]
    fn edited_is_updated_not_equal_created() {
        let mut j = json_comment();
        j["updated_at"] = json!("2026-10-15T14:09:00Z");
        let c = Comment::from_json(&j, "a/b").unwrap();
        assert!(c.edited());
    }

    #[test]
    fn a_deleted_user_becomes_an_unknown_author_and_missing_fields_are_errors() {
        let mut j = json_comment();
        j["user"] = Value::Null;
        assert_eq!(Comment::from_json(&j, "a/b").unwrap().author, "(unknown)");
        for k in ["id", "issue_url", "created_at", "updated_at"] {
            let mut j = json_comment();
            j.as_object_mut().unwrap().remove(k);
            assert!(Comment::from_json(&j, "a/b").is_err(), "{k}");
        }
        let mut j = json_comment();
        j["issue_url"] = json!("https://api.github.com/x/issues/abc");
        assert!(Comment::from_json(&j, "a/b").is_err());
    }

    #[test]
    fn an_owner_comment_counts_when_unedited_or_edited_by_the_owner_only() {
        let mut c = comment(1, "Owner-Login", "x");
        assert_eq!(c.untrusted_owner_edit("owner-login"), None);
        c.updated = "2026-10-15T14:09:00Z".into();
        // edited, editors not read: fail closed
        assert_eq!(
            c.untrusted_owner_edit("owner-login").as_deref(),
            Some(UNKNOWN_EDITOR)
        );
        c.editors = Some(vec![]);
        assert_eq!(
            c.untrusted_owner_edit("owner-login").as_deref(),
            Some(UNKNOWN_EDITOR)
        );
        c.editors = Some(vec!["owner-login".into(), "OWNER-login".into()]);
        assert_eq!(c.untrusted_owner_edit("owner-login"), None);
        c.editors = Some(vec!["owner-login".into(), "crew-bot".into()]);
        assert_eq!(
            c.untrusted_owner_edit("owner-login").as_deref(),
            Some("crew-bot")
        );
    }

    #[test]
    fn a_comment_verifies_under_a_former_name_only_in_the_main_repo() {
        use crate::protocol::canonical::IssueRef;
        use crate::protocol::former::FormerRepo;
        use crate::protocol::names::{AgentName, Kind, Re, Timestamp, To};
        use crate::protocol::signature::{Outgoing, generate_key, sign_message};
        let former =
            vec![FormerRepo::parse("former-org/former-repo", "2030-01-01T00:00:00Z").unwrap()];
        let key = generate_key().unwrap();
        // signed for issue 2 of the former name
        let msg = sign_message(
            &key,
            &IssueRef::new("former-org", "former-repo", 2),
            Outgoing {
                from: AgentName::parse("claude@host-a").unwrap(),
                to: To::Owner,
                kind: Kind::Info,
                re: Re::None,
                ts: Timestamp::parse("2026-10-15T14:00:00Z").unwrap(),
            },
            "hi",
        )
        .unwrap();
        let mut c = comment(1, "owner-login", &msg.to_comment());
        c.repo = "moved-org/moved-repo".into();
        let mut cfg = crate::registry::tests::cfg();
        cfg.repos = vec!["moved-org/moved-repo".into(), "moved-org/other-repo".into()];
        cfg.former_repos = former;
        assert!(
            c.verify(&msg, key.public_key(), cfg.former_for(&c.repo))
                .is_ok()
        );
        // the same message on issue 2 of another repo of the initiative: refused
        c.repo = "moved-org/other-repo".into();
        assert!(
            c.verify(&msg, key.public_key(), cfg.former_for(&c.repo))
                .is_err()
        );
        // control: with no list the main repo refuses it too
        c.repo = "moved-org/moved-repo".into();
        assert!(c.verify(&msg, key.public_key(), &[]).is_err());
    }
}
