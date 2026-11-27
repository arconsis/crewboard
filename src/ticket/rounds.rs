//! The rounds of repair that a ticket has spent (spec/tickets#round-limit).
//!
//! Pure, and from the signed log only: no board field holds the count, so nobody can set it by
//! hand. `verify` compares the count with `max_rounds` of AUTHORITY.md. `take`, `done` and `mine`
//! ask whether the round limit stopped the ticket.

use super::{Act, VMsg, closes_question};
use crate::protocol::acts::{
    counts_as_round, fail_classes, is_round_limit_fail, round_limit_rounds,
};
use crate::protocol::names::{Kind, Re};
use std::collections::BTreeSet;

/// What the signed log says about the round limit of one ticket ([`fold_rounds`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rounds {
    /// The `done` comments that a failed verify that counts answers. Two results for one `done`
    /// are one round: a second run of `verify` after a failed write spends no round.
    answered: BTreeSet<u64>,
    /// The failed verifies that count and name no `done` (no `re`): each is a round of its own.
    loose: usize,
    /// The question of the round limit is open: a coordinator asked the owner with the fixed
    /// first line, and no message that closes a question came after it.
    pub question_open: bool,
    /// The round limit stopped the ticket, and nothing released it. The stop starts with the
    /// verify result that has the `case:` line of the round limit, or with the question. It ends
    /// with a release or with a reassign.
    pub stopped: bool,
}

impl Rounds {
    /// The rounds that the ticket has spent.
    pub fn spent(&self) -> usize {
        self.answered.len() + self.loose
    }

    /// The rounds that the ticket has spent, without a round for the `done` in comment
    /// `done_comment`: what a verify of that `done` adds its own round to.
    pub fn spent_without(&self, done_comment: u64) -> usize {
        self.spent() - usize::from(self.answered.contains(&done_comment))
    }

    fn restart(&mut self) {
        self.answered.clear();
        self.loose = 0;
    }
}

/// Fold the verified messages of one ticket, in comment order.
///
/// - A round is a failed verify of a coordinator that counts: every fail, except one whose only
///   class is `head-moved` ([`counts_as_round`]). One round per `done` ([`Rounds::spent`]).
/// - A release is a message that closes a question to the owner ([`closes_question`]: an owner
///   message, or a coordinator `answer` that withdraws a question) while the question of the
///   round limit is open. It ends the stop, and the count starts again. The same message at any
///   other time releases nothing: an owner comment during normal repair keeps the count.
/// - A reassign notice of a coordinator ends the stop and starts the count again: a new agent
///   does not inherit the rounds. It closes no question.
pub fn fold_rounds(msgs: &[VMsg]) -> Rounds {
    let mut r = Rounds::default();
    for m in msgs {
        if closes_question(m) {
            if r.question_open {
                r.restart();
                r.stopped = false;
            }
            r.question_open = false;
            continue;
        }
        if !m.sender.actor().is_coordinator() {
            continue;
        }
        match m.act() {
            Act::Reassigned => {
                r.restart();
                r.stopped = false;
            }
            Act::Verify { pass: false } => {
                if counts_as_round(&fail_classes(&m.body)) {
                    match m.re {
                        Re::Comment { comment, .. } => {
                            r.answered.insert(comment);
                        }
                        _ => r.loose += 1,
                    }
                }
                if is_round_limit_fail(&m.body) {
                    r.stopped = true;
                }
            }
            _ if m.kind == Some(Kind::Question)
                && m.to == "owner"
                && round_limit_rounds(&m.body).is_some() =>
            {
                r.question_open = true;
                r.stopped = true;
            }
            _ => {}
        }
    }
    r
}

/// Did the round limit stop the ticket, with no release since? Then the ticket does not go back
/// to its agent: `take` and `done` of the assignee are refused, also for a ticket that the agent
/// parked or that the coordinator split before the stop.
pub fn round_limit_stopped(msgs: &[VMsg]) -> bool {
    fold_rounds(msgs).stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::acts::{round_limit_case, round_limit_question, withdraw_body};
    use crate::rules::Role;
    use crate::testkit::{agent, msg};

    const COORD: &str = "agent-a@host-a";
    const DEV: &str = "agent-b@host-b";

    fn rework_rounds(msgs: &[VMsg]) -> usize {
        fold_rounds(msgs).spent()
    }

    fn round_limit_question_open(msgs: &[VMsg]) -> bool {
        fold_rounds(msgs).question_open
    }

    fn by(name: &str, role: Role, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(1, agent(name, role), to, Some(kind), body)
    }

    /// A failed verify of the coordinator for the `done` in comment `done`.
    fn fail_for(done: u64, failed: &str) -> VMsg {
        VMsg {
            re: Re::Comment {
                issue: 1,
                comment: done,
            },
            ..by(
                COORD,
                Role::Coordinator,
                DEV,
                Kind::Report,
                &format!("verify: fail\npr: x\n{failed}"),
            )
        }
    }

    const REVIEW: &str = "failed: Gate check by the coordinator: part 2 is missing\n";
    const HEAD_MOVED: &str = "failed: PR head moved: the evidence says a, the head is b\n";

    fn review_fail(done: u64) -> VMsg {
        fail_for(done, REVIEW)
    }

    /// The failed verify that stops the ticket: a review fail with the `case:` line.
    fn stop_fail(done: u64) -> VMsg {
        fail_for(done, &format!("{REVIEW}case: {}\n", round_limit_case(2, 2)))
    }

    fn round_question() -> VMsg {
        let text = round_limit_question(2, 2, "https://github.com/a/b/issues/1#issuecomment-5");
        by(COORD, Role::Coordinator, "owner", Kind::Question, &text)
    }

    fn owner(body: &str) -> VMsg {
        msg(2, crate::ticket::Sender::Owner, "coordinator", None, body)
    }

    fn withdraw() -> VMsg {
        by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Answer,
            &withdraw_body("go on"),
        )
    }

    fn reassigned() -> VMsg {
        by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Info,
            "Reassigned. Reason: the agent is dead",
        )
    }

    #[test]
    fn every_coordinator_fail_counts_but_a_head_moved_only_fail() {
        assert_eq!(rework_rounds(&[]), 0);
        let ci_red = fail_for(12, "failed: CI of abc is not green: build\n");
        // a fail without a `failed:` line is a review fail
        let bare = fail_for(13, "");
        let both = fail_for(14, &format!("{HEAD_MOVED}{REVIEW}"));
        let msgs = [
            review_fail(10),
            fail_for(11, HEAD_MOVED),
            ci_red,
            bare,
            both,
        ];
        assert_eq!(rework_rounds(&msgs), 4);
        assert_eq!(
            rework_rounds(&[fail_for(10, HEAD_MOVED), fail_for(11, HEAD_MOVED)]),
            0
        );
    }

    #[test]
    fn two_results_for_one_done_are_one_round_and_a_result_without_re_is_its_own() {
        // a second run of `verify` for the same `done` spends no second round
        let msgs = [review_fail(10), review_fail(10), review_fail(11)];
        let r = fold_rounds(&msgs);
        assert_eq!(r.spent(), 2);
        // a verify of the `done` 11 adds no round, a verify of a new `done` adds one
        assert_eq!(r.spent_without(11), 1);
        assert_eq!(r.spent_without(12), 2);
        // a head-moved-only result and a counting result for one `done`: one round
        assert_eq!(
            rework_rounds(&[fail_for(10, HEAD_MOVED), review_fail(10)]),
            1
        );
        // no `re`: each result is a round
        let loose = || VMsg {
            re: Re::None,
            ..review_fail(0)
        };
        let msgs = [loose(), loose(), review_fail(10)];
        assert_eq!(rework_rounds(&msgs), 3);
        assert_eq!(fold_rounds(&msgs).spent_without(10), 2);
    }

    #[test]
    fn a_pass_a_done_and_a_fail_text_of_another_sender_do_not_count() {
        let pass = by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Report,
            "verify: pass\npr: x\n",
        );
        let done = by(
            DEV,
            Role::Dev,
            "coordinator",
            Kind::Done,
            "Done.\npr: x\nsha: y\nci: none\n",
        );
        // a dev agent that posts the text of a failed verify is no coordinator
        let forged = by(
            DEV,
            Role::Dev,
            "coordinator",
            Kind::Report,
            &format!("verify: fail\n{REVIEW}case: {}\n", round_limit_case(1, 1)),
        );
        // the same text of a coordinator with another kind is no verify result
        let info = by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Info,
            &format!("verify: fail\n{REVIEW}"),
        );
        let msgs = [review_fail(10), done.clone(), pass, done, forged, info];
        let r = fold_rounds(&msgs);
        assert_eq!(r.spent(), 1);
        assert!(!r.stopped && !r.question_open);
    }

    #[test]
    fn the_count_starts_again_only_after_the_release_of_a_stop() {
        // during normal repair an owner comment and a withdrawn question release nothing
        let msgs = [
            review_fail(10),
            owner("Looks slow, keep going."),
            withdraw(),
            review_fail(11),
        ];
        assert_eq!(rework_rounds(&msgs), 2);
        // the question of a dev agent, answered by the owner, is no stop
        let dev_question = by(DEV, Role::Dev, "owner", Kind::Question, "Which layout?");
        let msgs = [review_fail(10), dev_question, owner("The second one.")];
        assert_eq!(rework_rounds(&msgs), 1);

        // a stop: the owner comment releases it, and the count starts again
        let stop = [review_fail(10), stop_fail(11), round_question()];
        let r = fold_rounds(&stop);
        assert_eq!((r.spent(), r.stopped, r.question_open), (2, true, true));
        let mut msgs = stop.to_vec();
        msgs.push(owner("Change the plan."));
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (0, false, false));
        msgs.push(review_fail(12));
        assert_eq!(rework_rounds(&msgs), 1);
        // the coordinator's withdrawn question releases it too
        let mut msgs = stop.to_vec();
        msgs.push(withdraw());
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (0, false, false));
        // a plain answer of the coordinator and any message of a dev agent release nothing
        let mut msgs = stop.to_vec();
        msgs.push(by(COORD, Role::Coordinator, DEV, Kind::Answer, "go on"));
        msgs.push(by(
            DEV,
            Role::Dev,
            "coordinator",
            Kind::Answer,
            &withdraw_body("go on"),
        ));
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (2, true, true));
        // a later question of the dev agent hides nothing: the stop stays until a release
        msgs.push(by(DEV, Role::Dev, "owner", Kind::Question, "May I go on?"));
        assert!(round_limit_stopped(&msgs));
    }

    #[test]
    fn a_stop_starts_with_the_result_also_when_its_question_is_missing() {
        // the write of the question failed: the ticket is stopped, no question is open
        let msgs = [review_fail(10), stop_fail(11)];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (2, true, false));
        // an owner comment now answers no question of the limit: it releases nothing
        let msgs = [review_fail(10), stop_fail(11), owner("What is this?")];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped), (2, true));
    }

    #[test]
    fn a_reassign_ends_the_stop_and_starts_the_count_again_but_closes_no_question() {
        // the duty that returns the ticket of a dead agent posts the notice only
        let msgs = [
            review_fail(10),
            stop_fail(11),
            round_question(),
            reassigned(),
        ];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (0, false, true));
        // the new agent has its own rounds
        let mut msgs = msgs.to_vec();
        msgs.push(review_fail(12));
        assert_eq!(rework_rounds(&msgs), 1);
        // a reassign during normal repair starts the count again too
        assert_eq!(rework_rounds(&[review_fail(10), reassigned()]), 0);
        // the notice of a dev agent is no reassign
        let forged = by(
            DEV,
            Role::Dev,
            "coordinator",
            Kind::Info,
            "Reassigned. Reason: x",
        );
        assert_eq!(rework_rounds(&[review_fail(10), forged]), 1);
    }

    #[test]
    fn the_question_of_the_round_limit_is_a_coordinator_question_with_the_fixed_line() {
        assert!(round_limit_question_open(&[round_question()]));
        assert!(round_limit_stopped(&[round_question()]));
        // answered by the owner: not open
        assert!(!round_limit_question_open(&[
            round_question(),
            owner("Drop it.")
        ]));
        // another question, and the same text of a dev agent
        let other = by(COORD, Role::Coordinator, "owner", Kind::Question, "Which?");
        assert!(!round_limit_question_open(std::slice::from_ref(&other)));
        let same_text = VMsg {
            sender: agent(DEV, Role::Dev),
            ..round_question()
        };
        assert!(!round_limit_question_open(&[same_text]));
        // the fixed text to another addressee
        let to_dev = VMsg {
            to: DEV.into(),
            ..round_question()
        };
        assert!(!round_limit_question_open(&[to_dev]));
        assert!(!round_limit_question_open(&[]));
    }
}
