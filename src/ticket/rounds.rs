//! The rounds of repair that a ticket has spent (spec/tickets#round-limit).
//!
//! Pure, and from the signed log only: no board field holds the count, so nobody can set it by
//! hand. `verify` compares the count with `max_rounds` of AUTHORITY.md.

use super::{Act, VMsg, closes_question, question_open};
use crate::protocol::acts::{counts_as_round, fail_classes, round_limit_rounds};

/// Is `m` a failed verify of a coordinator that counts as a round? Every fail counts, except one
/// whose only class is `head-moved` ([`counts_as_round`]).
fn is_round(m: &VMsg) -> bool {
    m.sender.actor().is_coordinator()
        && m.act() == Act::Verify { pass: false }
        && counts_as_round(&fail_classes(&m.body))
}

/// The rounds that the ticket has spent: the failed verifies of a coordinator that count, after
/// the latest release. A release is a message that closes a question to the owner
/// ([`closes_question`]): an owner message, or a coordinator `answer` that withdraws a question.
/// After a release the count starts again.
pub fn rework_rounds(msgs: &[VMsg]) -> usize {
    let from = msgs
        .iter()
        .rposition(closes_question)
        .map_or(0, |at| at + 1);
    msgs[from..].iter().filter(|m| is_round(m)).count()
}

/// Is the question to the owner that is open on the ticket the question of the round limit (a
/// coordinator wrote it, with the fixed first line)? Then `verify` posts no second one.
pub fn round_limit_question_open(msgs: &[VMsg], assignee: Option<&str>) -> bool {
    question_open(msgs, assignee)
        .is_some_and(|m| m.sender.actor().is_coordinator() && round_limit_rounds(&m.body).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::acts::{round_limit_question, withdraw_body};
    use crate::protocol::names::Kind;
    use crate::rules::Role;
    use crate::testkit::{agent, msg};

    const COORD: &str = "agent-a@host-a";
    const DEV: &str = "agent-b@host-b";

    fn by(name: &str, role: Role, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(1, agent(name, role), to, Some(kind), body)
    }

    fn fail(failed: &str) -> VMsg {
        by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Report,
            &format!("verify: fail\npr: x\n{failed}"),
        )
    }

    fn review_fail() -> VMsg {
        fail("failed: Gate check by the coordinator: part 2 is missing\n")
    }

    fn head_moved() -> VMsg {
        fail("failed: PR head moved: the evidence says a, the head is b\n")
    }

    fn owner(body: &str) -> VMsg {
        msg(2, crate::ticket::Sender::Owner, "coordinator", None, body)
    }

    #[test]
    fn every_coordinator_fail_counts_but_a_head_moved_only_fail() {
        assert_eq!(rework_rounds(&[]), 0);
        let ci_red = fail("failed: CI of abc is not green: build\n");
        // a fail without a `failed:` line is a review fail
        let bare = fail("");
        let both = fail(
            "failed: PR head moved: the evidence says a, the head is b\nfailed: Gate check by the coordinator: x\n",
        );
        let msgs = [review_fail(), head_moved(), ci_red, bare, both];
        assert_eq!(rework_rounds(&msgs), 4);
        assert_eq!(rework_rounds(&[head_moved(), head_moved()]), 0);
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
            "verify: fail\nfailed: Gate check by the coordinator: x\n",
        );
        // the same text of a coordinator with another kind is no verify result
        let info = by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Info,
            "verify: fail\nfailed: Gate check by the coordinator: x\n",
        );
        let msgs = [review_fail(), done.clone(), pass, done, forged, info];
        assert_eq!(rework_rounds(&msgs), 1);
    }

    #[test]
    fn the_count_starts_again_after_an_owner_message_or_a_withdrawn_question() {
        let withdraw = by(
            COORD,
            Role::Coordinator,
            DEV,
            Kind::Answer,
            &withdraw_body("go on"),
        );
        // a plain answer of the coordinator and any message of a dev agent release nothing
        let plain = by(COORD, Role::Coordinator, DEV, Kind::Answer, "go on");
        let dev_withdraw = by(
            DEV,
            Role::Dev,
            "coordinator",
            Kind::Answer,
            &withdraw_body("go on"),
        );
        let msgs = [review_fail(), review_fail(), plain, dev_withdraw];
        assert_eq!(rework_rounds(&msgs), 2);
        let msgs = [
            review_fail(),
            review_fail(),
            owner("Change the plan."),
            review_fail(),
        ];
        assert_eq!(rework_rounds(&msgs), 1);
        let msgs = [review_fail(), review_fail(), withdraw.clone()];
        assert_eq!(rework_rounds(&msgs), 0);
        // the latest release decides
        let msgs = [
            review_fail(),
            owner("first"),
            review_fail(),
            review_fail(),
            withdraw,
            review_fail(),
        ];
        assert_eq!(rework_rounds(&msgs), 1);
    }

    #[test]
    fn the_open_question_of_the_round_limit_is_a_coordinator_question_with_the_fixed_line() {
        let text = round_limit_question(3, 3, "https://github.com/a/b/issues/1#issuecomment-5");
        let q = by(COORD, Role::Coordinator, "owner", Kind::Question, &text);
        assert!(round_limit_question_open(
            std::slice::from_ref(&q),
            Some(DEV)
        ));
        // answered by the owner: not open
        assert!(!round_limit_question_open(
            &[q.clone(), owner("Drop it.")],
            Some(DEV)
        ));
        // another open question, and the same text of the assignee
        let other = by(DEV, Role::Dev, "owner", Kind::Question, "Which layout?");
        assert!(!round_limit_question_open(
            std::slice::from_ref(&other),
            Some(DEV)
        ));
        let same_text = by(DEV, Role::Dev, "owner", Kind::Question, &text);
        assert!(!round_limit_question_open(&[same_text], Some(DEV)));
        // the latest open question decides
        assert!(!round_limit_question_open(
            &[q.clone(), other.clone()],
            Some(DEV)
        ));
        assert!(round_limit_question_open(&[other, q], Some(DEV)));
        assert!(!round_limit_question_open(&[], Some(DEV)));
    }
}
