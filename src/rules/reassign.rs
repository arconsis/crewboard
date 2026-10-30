//! The reasons of `reassign` and the status that it leads to (spec/tickets#status-flow-and-writers).
//!
//! A reassign is a judgement of the coordinator, so its reason is one of a short list and a
//! free-text detail, never free text alone. A ticket in Ready always has an agent: a reassign
//! with a new agent ends in Ready, one without ends in Backlog (the coordinator `assign`s it
//! when it has chosen an agent). Pure.

use super::Refusal;
use super::status::Status;
use crate::base::ids;

/// Why the coordinator takes a ticket from its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The agent sends no heartbeat (dead).
    Dead,
    /// The agent drains or has left.
    Stopping,
    /// The agent is late and does not answer.
    Late,
    /// The ticket waited for an answer or a decision for too long.
    BlockedTooLong,
}

impl Reason {
    /// The words that `--reason` accepts.
    pub const WORDS: [&'static str; 4] = ["dead", "stopping", "late", "blocked-too-long"];

    const ALL: [Reason; 4] = [
        Reason::Dead,
        Reason::Stopping,
        Reason::Late,
        Reason::BlockedTooLong,
    ];

    /// The word of the reason on the command line.
    pub fn word(self) -> &'static str {
        match self {
            Reason::Dead => "dead",
            Reason::Stopping => "stopping",
            Reason::Late => "late",
            Reason::BlockedTooLong => "blocked-too-long",
        }
    }

    /// Read a `--reason` value.
    pub fn parse(s: &str) -> Result<Reason, Refusal> {
        Reason::ALL
            .into_iter()
            .find(|r| r.word() == s.trim())
            .ok_or_else(|| {
                Refusal::new(
                    ids::STATUS_REASSIGN_REASON,
                    format!(
                        "'{}' is not a reassign reason: use one of {} (and --why for the text)",
                        s.trim(),
                        Reason::WORDS.join(", ")
                    ),
                )
            })
    }

    /// The sentence of the reason.
    fn sentence(self) -> &'static str {
        match self {
            Reason::Dead => "agent is dead",
            Reason::Stopping => "agent is stopping",
            Reason::Late => "agent is late and does not answer",
            Reason::BlockedTooLong => "ticket was blocked too long",
        }
    }

    /// The text that the notice, the override record and the output carry: the sentence, then
    /// the detail when there is one.
    pub fn text(self, detail: Option<&str>) -> String {
        match detail.map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => format!("{}: {d}", self.sentence()),
            None => self.sentence().to_string(),
        }
    }
}

/// The status a reassign ends in: Ready only with an agent, else Backlog.
pub fn target_status(new_agent: bool) -> Status {
    if new_agent {
        Status::Ready
    } else {
        Status::Backlog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_parses_and_round_trips() {
        for w in Reason::WORDS {
            assert_eq!(Reason::parse(w).unwrap().word(), w);
        }
        assert_eq!(Reason::ALL.len(), Reason::WORDS.len());
    }

    #[test]
    fn free_text_is_not_a_reason() {
        for bad in ["", "agent stopped", "Dead", "blocked too long", "x"] {
            let e = Reason::parse(bad).unwrap_err();
            assert_eq!(e.rule, "status-reassign-reason", "{bad:?}");
            assert!(e.message.contains("dead, stopping, late, blocked-too-long"));
        }
    }

    #[test]
    fn the_text_has_the_sentence_and_the_detail() {
        assert_eq!(Reason::Dead.text(None), "agent is dead");
        assert_eq!(Reason::Dead.text(Some("  ")), "agent is dead");
        assert_eq!(
            Reason::BlockedTooLong.text(Some("no answer since Monday")),
            "ticket was blocked too long: no answer since Monday"
        );
    }

    #[test]
    fn ready_always_has_an_agent() {
        assert_eq!(target_status(true), Status::Ready);
        assert_eq!(target_status(false), Status::Backlog);
    }
}
