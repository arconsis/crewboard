//! The one way to load a ticket (spec/tickets#ticket-content, spec/board#item-ids-are-the-source-of-truth):
//! the issue, its valid signed messages, and the board item that the signed info comment names,
//! read by id. [`read_messages`] is the first half for a command that needs only the messages.

use super::Reader;
use crate::base::Refusal;
use crate::base::TicketRef;
use crate::base::ids;
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::github::{Client, GhError};
use crate::items::ItemSnapshot;
use crate::registry::Trust;
use crate::rules::fields::Field;
use crate::rules::status::Status;
use crate::ticket::{VMsg, WORK_LABEL, find_item_id, verified_messages};
use serde_json::Value;

/// The issue as GitHub returns it.
#[derive(Debug, Clone)]
pub(crate) struct IssueInfo {
    pub(crate) node_id: String,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) open: bool,
    pub(crate) labels: Vec<String>,
    pub(crate) is_pr: bool,
}

pub(crate) fn parse_issue(v: &Value) -> Result<IssueInfo, GhError> {
    let shape = |m: &str| GhError::Shape(format!("issue: {m}"));
    Ok(IssueInfo {
        node_id: v
            .get("node_id")
            .and_then(Value::as_str)
            .ok_or_else(|| shape("`node_id` missing"))?
            .to_string(),
        number: v
            .get("number")
            .and_then(Value::as_u64)
            .ok_or_else(|| shape("`number` missing"))?,
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        body: v
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        open: v.get("state").and_then(Value::as_str) == Some("open"),
        labels: v
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.get("name").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        is_pr: v.get("pull_request").is_some(),
    })
}

/// spec/tickets#ticket-content: a ticket is an issue with the label `crew:work`, in a repo of the
/// initiative.
pub(crate) fn check_work_issue(i: &IssueInfo, repo: &str) -> Result<(), Refusal> {
    if i.is_pr {
        return Err(Refusal::new(
            ids::TICKET_IS_PR,
            format!("{repo}#{} is a pull request, not a ticket", i.number),
        ));
    }
    if !i.labels.iter().any(|l| l == WORK_LABEL) {
        return Err(Refusal::new(
            ids::TICKET_NOT_WORK,
            format!("{repo}#{} has no label {WORK_LABEL}", i.number),
        ));
    }
    Ok(())
}

pub(crate) fn fetch_issue(client: &Client, repo: &str, n: u64) -> CbResult<IssueInfo> {
    let r = client.get(&format!("repos/{repo}/issues/{n}"))?;
    Ok(parse_issue(&r.body)?)
}

/// A ticket lives in a repo of the initiative.
pub(crate) fn check_repo(cfg: &BoardConfig, repo: &str) -> CbResult<()> {
    if !cfg.repos.contains(&repo.to_string()) {
        return Err(CbError::refused(
            ids::REPO_NOT_IN_INITIATIVE,
            format!("{repo} is not a repo of initiative {}", cfg.initiative),
        ));
    }
    Ok(())
}

/// All comments of one issue and the valid signed messages among them (one comment-list read).
pub(crate) fn read_messages(
    r: &Reader,
    trust: &Trust,
    repo: &str,
    issue: u64,
) -> CbResult<(Vec<Comment>, Vec<VMsg>)> {
    let (comments, _) = feed::issue_comments(r.client, repo, issue, None, &r.cfg.owner_login)?;
    let msgs = verified_messages(&comments, r.cfg, trust);
    Ok((comments, msgs))
}

/// One ticket, loaded: the issue, the item by id, and the valid messages of the issue.
pub(crate) struct Ticket {
    pub(crate) repo: String,
    pub(crate) issue: u64,
    pub(crate) item_id: String,
    pub(crate) item: ItemSnapshot,
    pub(crate) info: IssueInfo,
    pub(crate) comments: Vec<Comment>,
    pub(crate) msgs: Vec<VMsg>,
}

impl Ticket {
    pub(crate) fn status(&self) -> CbResult<Status> {
        let s = self.item.get(Field::Status).ok_or_else(|| {
            CbError::refused(
                ids::TICKET_STATUS_UNSET,
                format!("{}#{} has no Status on the board", self.repo, self.issue),
            )
        })?;
        Status::parse_board(s).ok_or_else(|| {
            CbError::refused(
                ids::TICKET_STATUS_UNKNOWN,
                format!("Status '{s}' of {}#{} is not known", self.repo, self.issue),
            )
        })
    }

    pub(crate) fn agent(&self) -> Option<&str> {
        self.item.get(Field::Agent)
    }

    /// The one check that an agent-facing command runs before it trusts [`Ticket::agent`]:
    /// the board `Agent` must be the agent of the latest signed assignment
    /// (spec/tickets#assignment). Board fields are not signed; any holder of the crew token can
    /// write one. An empty `Agent` passes (it gives nobody a right).
    pub(crate) fn check_agent_signed(&self) -> Result<(), crate::rules::Refusal> {
        let log = crate::ticket::log::fold_ticket(&self.msgs);
        if !crate::ticket::log::agent_contradicts(&log, self.agent()) {
            return Ok(());
        }
        let board = self.agent().unwrap_or("?");
        let said = match &log.agent {
            Some(a) => format!("the signed log assigns it to {a}"),
            None => "the signed log has no assignment for it".to_string(),
        };
        Err(crate::rules::Refusal::new(
            ids::TICKET_AGENT_NOT_SIGNED,
            format!(
                "{}: the board Agent is {board}, but {said}; a board field alone gives no right to act. \
                 The coordinator repairs it with `crewboard assign {} --to <agent>` (a Ready ticket) \
                 or `crewboard reassign {} --to <agent>`; both post the signed assignment",
                self.key(),
                self.issue,
                self.issue
            ),
        ))
    }

    /// The ticket.
    pub(crate) fn tref(&self) -> TicketRef {
        TicketRef::new(&self.repo, self.issue)
    }

    /// `owner/repo#n`.
    pub(crate) fn key(&self) -> String {
        self.tref().to_string()
    }
}

/// Load a ticket: check the repo, read the issue, check that it is a `crew:work` ticket, read
/// its messages, take the item id from the signed info comment, and read the item by id.
pub(crate) fn load_ticket(r: &Reader, trust: &Trust, t: &TicketRef) -> CbResult<Ticket> {
    let (repo, issue) = (t.repo.as_str(), t.issue);
    check_repo(r.cfg, repo)?;
    let info = fetch_issue(r.client, repo, issue)?;
    check_work_issue(&info, repo)?;
    let (comments, msgs) = read_messages(r, trust, repo, issue)?;
    let item_id = find_item_id(&msgs).ok_or_else(|| {
        CbError::refused(
            ids::TICKET_NOT_ON_BOARD,
            format!("{t} has no signed info comment with its item id; the coordinator must `assign` it first"),
        )
    })?;
    let item = r.board.read_item(r.client, &item_id)?;
    Ok(Ticket {
        repo: repo.to_string(),
        issue,
        item_id,
        item,
        info,
        comments,
        msgs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_json() -> Value {
        json!({"node_id": "I_x", "number": 12, "title": "t", "body": null, "state": "open",
               "labels": [{"name": "crew:work"}, {"name": "other"}]})
    }

    #[test]
    fn an_issue_parses_and_a_null_body_is_empty() {
        let i = parse_issue(&issue_json()).unwrap();
        assert_eq!(
            (i.number, i.body.as_str(), i.open, i.is_pr),
            (12, "", true, false)
        );
        assert!(i.labels.contains(&"crew:work".to_string()));
        for k in ["node_id", "number"] {
            let mut j = issue_json();
            j.as_object_mut().unwrap().remove(k);
            assert!(parse_issue(&j).is_err(), "{k}");
        }
    }

    #[test]
    fn only_an_issue_with_the_work_label_is_a_ticket() {
        let ok = parse_issue(&issue_json()).unwrap();
        assert!(check_work_issue(&ok, "a/b").is_ok());
        let mut no_label = issue_json();
        no_label["labels"] = json!([{"name": "bug"}]);
        assert_eq!(
            check_work_issue(&parse_issue(&no_label).unwrap(), "a/b")
                .unwrap_err()
                .rule,
            "ticket-not-work"
        );
        let mut pr = issue_json();
        pr["pull_request"] = json!({"url": "x"});
        assert_eq!(
            check_work_issue(&parse_issue(&pr).unwrap(), "a/b")
                .unwrap_err()
                .rule,
            "ticket-is-pr"
        );
    }
}
