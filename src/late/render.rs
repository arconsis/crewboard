//! The texts of a late wait: the event for the agent, the line of `status`, the note for the
//! coordinator and the owner.

use super::*;

impl Late {
    fn ticket(&self) -> Option<(&str, &str, u64, &str, Option<&str>)> {
        match &self.subject {
            Subject::Ticket {
                key,
                repo,
                issue,
                item_id,
                agent,
            } => Some((key, repo, *issue, item_id, agent.as_deref())),
            Subject::Agent { .. } => None,
        }
    }

    /// `owner/repo#n` of the ticket, or the name of the agent.
    pub fn subject_name(&self) -> String {
        match &self.subject {
            Subject::Ticket { key, .. } => key.clone(),
            Subject::Agent { name, .. } => name.clone(),
        }
    }

    /// Is the subject the coordinator?
    pub fn is_coordinator(&self) -> bool {
        matches!(
            &self.subject,
            Subject::Agent {
                coordinator: true,
                ..
            }
        )
    }

    /// The key of one report to the owner: `<wait>:<ticket or agent>:<since>`. For `dead`
    /// `since` is the heartbeat value the agent was last seen with, so one dead episode has one
    /// key, and an agent that comes back and dies again gets a new one. For `verify`
    /// the key ends with `:<coordinator_since>` (the baseline of the late coordinator), so a
    /// coordinator that comes back and goes away again gives a new report for the same ticket.
    pub fn report_key(&self, coordinator_since: Option<i64>) -> String {
        let mut key = format!(
            "{}:{}:{}",
            self.wait.word(),
            self.subject_name(),
            self.since
        );
        if self.wait == Wait::Verify
            && let Some(c) = coordinator_since
        {
            key.push_str(&format!(":{c}"));
        }
        key
    }

    pub fn repo(&self) -> Option<&str> {
        self.ticket().map(|t| t.1)
    }

    pub fn issue(&self) -> Option<u64> {
        self.ticket().map(|t| t.2)
    }

    pub fn item_id(&self) -> Option<&str> {
        self.ticket().map(|t| t.3)
    }

    pub fn agent(&self) -> Option<&str> {
        self.ticket().and_then(|t| t.4)
    }

    /// The key in the seen map.
    pub fn key(&self) -> Option<String> {
        self.item_id().map(|i| seen_key(i, self.wait))
    }

    /// What distinguishes two events of the same wait: the head SHA, else `-`.
    pub fn value(&self) -> String {
        self.sha.clone().unwrap_or_else(|| "-".into())
    }

    /// The value of the seen entry of what the coordinator was told: the head SHA (or `-`) and
    /// the time given.
    pub fn told_up_value(&self, now: i64) -> String {
        format!("{}|{now}", self.value())
    }

    pub fn to_json(&self, e: &Expect, coordinator_late: bool, now: i64) -> Value {
        let told: Vec<String> = tells(self, e, coordinator_late, now)
            .iter()
            .map(Who::label)
            .collect();
        json!({
            "wait": self.wait.word(),
            "subject": self.subject_name(),
            "since": crate::clock::format_ts(self.since),
            "age_s": self.age,
            "expected_s": self.expected,
            "head_sha": self.sha,
            "pr": self.pr,
            "told": told,
        })
    }

    /// One line of the `late:` section of `status`.
    pub fn status_line(&self, e: &Expect, coordinator_late: bool, now: i64) -> String {
        let told = tells(self, e, coordinator_late, now)
            .iter()
            .map(Who::label)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "  {:<11} {:<34} since {}  age {}  expected {}  told: {}\n",
            self.wait.word(),
            self.subject_name(),
            crate::clock::format_ts(self.since),
            crate::clock::fmt_age(self.age),
            crate::clock::fmt_age(self.expected),
            if told.is_empty() { "nobody yet" } else { &told }
        )
    }

    /// The first line of an event body: `stale-work: <case>` for `push`, `done` and `verify`,
    /// else `late: <wait>`.
    fn first_line(&self) -> String {
        match self.wait {
            Wait::Push | Wait::Done | Wait::Verify => format!("stale-work: {}", self.wait.case()),
            other => format!("late: {}", other.word()),
        }
    }

    /// The text of the event for the one that must act. `prefix` is the crew branch prefix.
    pub fn event_body(&self, to: &Who, e: &Expect, prefix: &str) -> String {
        if matches!(to, Who::Coordinator) && coordinator_hears_again(self.wait) {
            return self.escalated_body(e);
        }
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let head = format!("{}\nticket: {}\n", self.first_line(), self.subject_name());
        match self.wait {
            Wait::Push => format!(
                "{head}It is In Progress for {age} (limit {limit}) and no branch `{prefix}{}/{}-...` is on GitHub.\n\
                 Push your work now, then run `done`. If you cannot finish, run `block` and say why.\n",
                self.agent().unwrap_or("<agent>"),
                self.issue().unwrap_or(0)
            ),
            Wait::Done => format!(
                "{head}PR {} has green CI on head {} for {age} (limit {limit}) and no `done` names that head.\n\
                 Run `done` now. If the work is not finished, push it or run `block` and say why.\n",
                self.pr.as_deref().unwrap_or("?"),
                self.sha
                    .as_deref()
                    .map_or("?".to_string(), |s| s.chars().take(7).collect())
            ),
            Wait::Verify => format!(
                "{head}It is in Review for {age} (limit {limit}) and its latest `done` has no passing `verify` on the current PR head.\n\
                 Run `verify` now. If you cannot, tell the owner why. A ticket that passed `verify` on the current head is not reported: if you hold its merge, nothing is late.\n"
            ),
            Wait::Take => format!(
                "{head}It is Ready for {age} (expected {limit}) and was not taken.\n\
                 Run `take` now. If you cannot, tell the coordinator why.\n"
            ),
            Wait::Answer => format!(
                "{head}It is Blocked for {age} (expected {limit}) and its question has no answer.\n\
                 Answer it, or tell the owner why you cannot.\n"
            ),
            Wait::OwnerMerge | Wait::Owner | Wait::Heartbeat | Wait::Dead => {
                format!("{head}It waits for {age} (expected {limit}).\n")
            }
        }
    }

    /// What clears the wait, for the `next:` line of the note to the owner.
    /// `None` for the waits that the owner never hears of (`take`, `push`, `done`).
    pub fn next_step(&self) -> Option<&'static str> {
        Some(match self.wait {
            Wait::Verify => "the interactive coordinator verifies, or the owner decides",
            Wait::OwnerMerge => "the owner reviews and merges the PR",
            Wait::Owner => "the owner answers the question on the ticket",
            Wait::Answer => "the coordinator answers, or the owner decides",
            Wait::Heartbeat if self.is_coordinator() => {
                "restart the coordinator session, or approve another agent with `crewboard approve <agent> --role coordinator`"
            }
            Wait::Heartbeat => {
                "the agent writes a heartbeat again, or the coordinator reassigns its tickets once it is dead"
            }
            Wait::Dead => "restart the agent session; its open tickets are back in Ready",
            Wait::Take | Wait::Push | Wait::Done => return None,
        })
    }

    /// The signed `info` to the owner (one per [`Late::report_key`]): the first line
    /// `late: <wait> <subject>`, the lines `late-key:`, `since:` and `next:`, then the text for a
    /// person. `detail` is added at the end (the facts of the caller, such as the coordinator
    /// heartbeat or the report of a dead agent).
    /// `coordinator_since` is the baseline of the late coordinator (see [`Late::report_key`]).
    /// `None` for a wait that the owner never hears of.
    pub fn owner_info(
        &self,
        owner_login: &str,
        detail: &str,
        coordinator_since: Option<i64>,
    ) -> Option<String> {
        let next = self.next_step()?;
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let since = crate::clock::format_ts(self.since);
        let text = match self.wait {
            Wait::Heartbeat => format!(
                "@{owner_login}: {} shows no sign of life for {} s (since {since}; late after {} s). It is not running.\n{}",
                if self.is_coordinator() {
                    format!("the coordinator {}", self.subject_name())
                } else {
                    format!("the agent {}", self.subject_name())
                },
                self.age,
                self.expected,
                if self.is_coordinator() {
                    "Nobody assigns, answers or verifies until it is back. Dev agents go on with their open tickets and take no coordinator duties.\n"
                } else {
                    ""
                }
            ),
            Wait::Dead => format!(
                "@{owner_login}: the agent {} is dead (no sign of life since {since}).\n",
                self.subject_name()
            ),
            Wait::Verify => format!(
                "@{owner_login}: the ticket is in Review for {age} (limit {limit}) and its latest `done` has no passing `verify` on the current PR head. The coordinator shows no sign of life, so nobody verifies it.\n"
            ),
            Wait::OwnerMerge => format!(
                "@{owner_login}: the ticket is in Review for {age} (expected {limit}). Its `done` names protected paths or an owner route, so you merge the PR.\n"
            ),
            Wait::Owner => {
                format!("@{owner_login}: a question waits for you for {age} (expected {limit}).\n")
            }
            Wait::Answer => format!(
                "@{owner_login}: the ticket is Blocked for {age} (expected {limit}) and its question has no answer.\n"
            ),
            Wait::Take | Wait::Push | Wait::Done => return None,
        };
        Some(format!(
            "{}since: {since}\nnext: {next}\n{text}{detail}",
            crate::protocol::acts::late_head(
                self.wait.word(),
                &self.subject_name(),
                &self.report_key(coordinator_since),
            ),
        ))
    }

    /// The line of the headless `tick` log.
    pub fn tick_line(&self) -> String {
        let what = match self.wait {
            Wait::Push | Wait::Done => format!("stale-work escalated: {}", self.wait.case()),
            other => format!("late: {}", other.word()),
        };
        format!(
            "left for the interactive coordinator: {}, {what} for {}, agent {}; the interactive coordinator runs `say`, `reassign` or `ticket split`\n",
            self.subject_name(),
            crate::clock::fmt_age(self.age),
            self.agent().unwrap_or("?")
        )
    }

    /// The text of the event for the coordinator when a dev wait goes on.
    /// `escalated: true` is a line of its own, so a prompt can match it.
    fn escalated_body(&self, e: &Expect) -> String {
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let more = crate::clock::fmt_age(e.escalate);
        let mut out = format!(
            "{}\nticket: {}\nescalated: true\nagent: {}\nage: {age}\nlimit: {limit}\nescalated after: {more} past the limit\n",
            self.first_line(),
            self.subject_name(),
            self.agent().unwrap_or("?"),
        );
        if let Some(sha) = &self.sha {
            out.push_str(&format!(
                "head: {}\n",
                sha.chars().take(7).collect::<String>()
            ));
        }
        if let Some(pr) = &self.pr {
            out.push_str(&format!("pr: {pr}\n"));
        }
        out.push_str(
            "The dev agent got this marker and did not act. Ask it with `say`, or `reassign` the ticket, or `ticket split` it.\n",
        );
        out
    }
}
