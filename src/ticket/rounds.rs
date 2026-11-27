//! The rounds of repair that a ticket has spent (spec/tickets#round-limit).
//!
//! Pure, and from the signed log only: no board field holds the count, so nobody can set it by
//! hand. `verify` compares the count with `max_rounds` of AUTHORITY.md. `take`, `done` and `mine`
//! ask whether the round limit stopped the ticket.

use super::{Act, VMsg, closes_question};
use crate::protocol::acts::{
    counts_as_round, fail_classes, is_round_limit_fail, round_limit_rounds,
};
use crate::protocol::names::Kind;

/// What the signed log says about the round limit of one ticket ([`fold_rounds`]). It is a
/// state machine with one step per message ([`Rounds::step`]); the status fold
/// ([`super::log::fold_ticket`]) runs the same step, so both agree on when a ticket is stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rounds {
    /// The rounds that the ticket has spent since the count started: one for each failed verify
    /// that counts, whatever `done` it answers. The log alone cannot tell a second run of
    /// `verify` from a verify after a `done` that its author deleted, so both are a round.
    spent: usize,
    /// The question of this stop is open: a coordinator asked the owner with the fixed first
    /// line, and no message that closes a question, no reassign and no assignment came after it.
    pub question_open: bool,
    /// The round limit stopped the ticket, and nothing ended the stop.
    pub stopped: bool,
}

impl Rounds {
    /// The rounds that the ticket has spent.
    pub fn spent(&self) -> usize {
        self.spent
    }

    /// The stop ends and the count starts again.
    fn restart(&mut self) {
        self.spent = 0;
        self.stopped = false;
        self.question_open = false;
    }

    /// One message of the log, in comment order.
    ///
    /// - A round is a failed verify of a coordinator that counts: every fail, except one whose
    ///   only class is `head-moved` ([`counts_as_round`]). A second failed verify for the same
    ///   `done` counts too.
    /// - A stop starts with the verify result that has the `case:` line of the round limit, or
    ///   with the question of the round limit of a coordinator.
    /// - A stop ends, and the count starts again, with a message that closes a question to the
    ///   owner while the stop is on ([`closes_question`]: an owner message, which an owner move
    ///   is too, or a coordinator `answer` that withdraws a question).
    /// - A reassign notice and an assignment of a coordinator end a stop and start the count
    ///   again at any time: a new agent does not inherit the rounds.
    /// - The same closing message while no stop is on changes nothing: an owner comment during
    ///   normal repair keeps the count.
    pub fn step(&mut self, m: &VMsg) {
        if closes_question(m) {
            if self.stopped {
                self.restart();
            }
            self.question_open = false;
            return;
        }
        if !m.sender.actor().is_coordinator() {
            return;
        }
        match m.act() {
            Act::Reassigned | Act::Assignment { .. } => self.restart(),
            Act::Verify { pass: false } => {
                if counts_as_round(&fail_classes(&m.body)) {
                    self.spent += 1;
                }
                if is_round_limit_fail(&m.body) {
                    self.stopped = true;
                }
            }
            _ if m.kind == Some(Kind::Question)
                && m.to == "owner"
                && round_limit_rounds(&m.body).is_some() =>
            {
                self.question_open = true;
                self.stopped = true;
            }
            _ => {}
        }
    }
}

/// Fold the verified messages of one ticket, in comment order ([`Rounds::step`]).
pub fn fold_rounds(msgs: &[VMsg]) -> Rounds {
    let mut r = Rounds::default();
    for m in msgs {
        r.step(m);
    }
    r
}

/// Did the round limit stop the ticket, with no end of the stop since? Then the ticket does not
/// go back to its agent: `take` and `done` of the assignee are refused, also for a ticket that
/// the agent parked or that the coordinator split before the stop.
pub fn round_limit_stopped(msgs: &[VMsg]) -> bool {
    fold_rounds(msgs).stopped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::acts::{round_limit_case, round_limit_question, withdraw_body};
    use crate::protocol::names::Re;
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
    fn every_counting_result_is_a_round_whatever_done_it_answers() {
        // a second run of `verify` for the same `done`: two rounds
        assert_eq!(rework_rounds(&[review_fail(10), review_fail(10)]), 2);
        let msgs = [review_fail(10), review_fail(10), review_fail(11)];
        assert_eq!(rework_rounds(&msgs), 3);
        // a head-moved-only result is free, also next to a counting result for the same `done`
        assert_eq!(
            rework_rounds(&[fail_for(10, HEAD_MOVED), review_fail(10)]),
            1
        );
        assert_eq!(
            rework_rounds(&[fail_for(10, HEAD_MOVED), fail_for(10, HEAD_MOVED)]),
            0
        );
        // a result without `re` is a round too
        let loose = || VMsg {
            re: Re::None,
            ..review_fail(0)
        };
        assert_eq!(rework_rounds(&[loose(), loose(), review_fail(10)]), 3);
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

    fn assignment() -> VMsg {
        by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Request,
            "Assigned to you: t\ngate: g\n",
        )
    }

    #[test]
    fn a_done_that_its_author_deleted_gives_no_free_round() {
        let dev = |kind: Kind, body: &str| by(DEV, Role::Dev, "coordinator", kind, body);
        // the dev delivered again and deleted that `done`: the next result answers the older
        // `done` again. With or without a message of the dev between the two results, it is a
        // round.
        let msgs = [assignment(), review_fail(10), review_fail(10)];
        assert_eq!(rework_rounds(&msgs), 2);
        for kind in [Kind::Ack, Kind::Done, Kind::Blocked] {
            let msgs = [
                assignment(),
                review_fail(10),
                dev(kind, "x"),
                review_fail(10),
            ];
            assert_eq!(rework_rounds(&msgs), 2, "{kind:?}");
        }
    }

    #[test]
    fn a_stop_starts_with_the_result_also_when_its_question_is_missing() {
        // the write of the question failed: the ticket is stopped, no question is open
        let msgs = [review_fail(10), stop_fail(11)];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (2, true, false));
        // sequence A: an owner move in that window ends the stop, and so does any owner comment
        for text in ["/crew move In Progress", "Go on."] {
            let msgs = [review_fail(10), stop_fail(11), owner(text)];
            let r = fold_rounds(&msgs);
            assert_eq!((r.spent(), r.stopped), (0, false), "{text}");
        }
    }

    #[test]
    fn a_reassign_and_an_assignment_end_the_stop_and_start_the_count_again() {
        // the duty that returns the ticket of a dead agent posts the notice only: the stop
        // ends, and the old question is not the question of a later stop
        let msgs = [
            review_fail(10),
            stop_fail(11),
            round_question(),
            reassigned(),
        ];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (0, false, false));
        // the new agent has its own rounds, and its stop asks the owner again
        let mut msgs = msgs.to_vec();
        msgs.extend([assignment(), review_fail(12), stop_fail(13)]);
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped, r.question_open), (2, true, false));
        // sequence B: a reassign to the same agent (the withdrawn question, the notice, the
        // assignment, in any order of the first two)
        for first_two in [[withdraw(), reassigned()], [reassigned(), withdraw()]] {
            let mut msgs = vec![review_fail(10), stop_fail(11), round_question()];
            msgs.extend(first_two);
            msgs.push(assignment());
            let r = fold_rounds(&msgs);
            assert_eq!((r.spent(), r.stopped, r.question_open), (0, false, false));
        }
        // an assignment alone (the owner moved the ticket to Backlog before) ends a stop too
        let msgs = [review_fail(10), stop_fail(11), assignment()];
        let r = fold_rounds(&msgs);
        assert_eq!((r.spent(), r.stopped), (0, false));
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
