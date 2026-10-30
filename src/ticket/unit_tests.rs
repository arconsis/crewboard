use super::*;
use crate::comments::Comment;
use crate::comments::tests::comment;
use crate::config::BoardConfig;
use crate::events::{Event, EventClass};
use crate::items::ItemSnapshot;
use crate::protocol::names::{Kind, Re};
use crate::protocol::signature::generate_key;
use crate::registry::Trust;
use crate::registry::tests::{cfg, join_comment, owner, pub_line, signed};
use crate::registry::{build_trust, resolve};
use crate::rules::Role;
use crate::rules::status::{BlockReason, Status};
use crate::state::TicketSnap;
use crate::testkit::row;
use ssh_key::PrivateKey;
use std::collections::BTreeMap;
use std::collections::HashMap;

struct W {
    ka: PrivateKey,
    kb: PrivateKey,
    trust: Trust,
    cfg: BoardConfig,
}

fn item(id: &str, key: &str) -> ItemSnapshot {
    ItemSnapshot {
        id: id.into(),
        updated_at: "2026-10-15T14:00:00Z".into(),
        item_type: "DRAFT_ISSUE".into(),
        project_number: Some(15),
        issue_state: None,
        fields: [
            ("Key".to_string(), key.to_string()),
            ("State".to_string(), "active".to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

/// claude@a coordinator, hermes@b dev.
fn w() -> W {
    let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
    let cfg = cfg();
    let reg = resolve(
        &cfg,
        &[
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            owner(3, "/crew approve claude@a role=coordinator"),
            owner(4, "/crew approve hermes@b role=dev"),
        ],
    );
    let mut items = HashMap::new();
    items.insert(
        "PVTI_a".to_string(),
        item("PVTI_a", &pub_line(&ka, "claude@a")),
    );
    items.insert(
        "PVTI_b".to_string(),
        item("PVTI_b", &pub_line(&kb, "hermes@b")),
    );
    let trust = build_trust(&reg, &items, 15);
    assert_eq!(trust.agents.len(), 2);
    W { ka, kb, trust, cfg }
}

fn by_a(w: &W, id: u64, to: &str, kind: Kind, body: &str) -> Comment {
    signed(id, "owner-login", &w.ka, "claude@a", to, kind, body)
}
fn by_b(w: &W, id: u64, to: &str, kind: Kind, body: &str) -> Comment {
    signed(id, "owner-login", &w.kb, "hermes@b", to, kind, body)
}
fn msgs(w: &W, cs: &[Comment]) -> Vec<VMsg> {
    verified_messages(cs, &w.cfg, &w.trust)
}

#[test]
fn an_owner_comment_counts_unless_somebody_but_the_owner_edited_it() {
    let w = w();
    let edited = |id: u64, editors: Option<&[&str]>| {
        let mut c = crate::comments::tests::comment(id, "owner-login", "go");
        c.updated = "2026-10-15T14:09:00Z".into();
        c.editors = editors.map(|e| e.iter().map(|x| x.to_string()).collect());
        c
    };
    let cs = [
        crate::comments::tests::comment(1, "owner-login", "go"),
        edited(2, Some(&["owner-login"])),
        edited(3, Some(&["owner-login", "crew-bot"])),
        edited(4, Some(&["crew-bot"])),
        edited(5, None),
    ];
    let ids: Vec<u64> = msgs(&w, &cs).iter().map(|m| m.comment_id).collect();
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn old_coordinator_messages_count_after_a_handover() {
    // claude@a is the coordinator at first and a dev after comment 20; hermes@b becomes
    // the coordinator. What claude@a signed as the coordinator keeps its meaning.
    let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
    let cfg = cfg();
    let reg = resolve(
        &cfg,
        &[
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            owner(3, "/crew approve claude@a role=coordinator"),
            owner(4, "/crew approve hermes@b role=dev"),
            owner(20, "/crew role claude@a dev"),
            owner(21, "/crew role hermes@b coordinator"),
        ],
    );
    assert!(reg.notes.is_empty(), "{:?}", reg.notes);
    let mut items = HashMap::new();
    items.insert(
        "PVTI_a".to_string(),
        item("PVTI_a", &pub_line(&ka, "claude@a")),
    );
    items.insert(
        "PVTI_b".to_string(),
        item("PVTI_b", &pub_line(&kb, "hermes@b")),
    );
    let trust = build_trust(&reg, &items, 15);
    let a =
        |id, to: &str, kind, body: &str| signed(id, "owner-login", &ka, "claude@a", to, kind, body);
    let b =
        |id, to: &str, kind, body: &str| signed(id, "owner-login", &kb, "hermes@b", to, kind, body);
    let split = split_note("o/r#5", "o/r#6", "Open part");
    let cs = [
        a(
            10,
            "all",
            Kind::Info,
            "Ticket is on the board\nitem=PVTI_t1",
        ),
        b(11, "coordinator", Kind::Blocked, "need a decision"),
        a(12, "hermes@b", Kind::Answer, "use the old API"),
        a(13, "hermes@b", Kind::Info, &split),
        // after the handover claude@a is a dev: this info makes no ticket
        a(
            30,
            "all",
            Kind::Info,
            "Ticket is on the board\nitem=PVTI_t2",
        ),
    ];
    let v = verified_messages(&cs, &cfg, &trust);
    assert_eq!(v.len(), 5, "history and the later message are all valid");
    let role_of = |id: u64| match &v.iter().find(|m| m.comment_id == id).unwrap().sender {
        Sender::Agent { role, .. } => *role,
        Sender::Owner => panic!("owner"),
    };
    assert_eq!(role_of(10), Role::Coordinator);
    assert_eq!(role_of(30), Role::Dev);
    // a fresh cache finds the ticket of the old coordinator, not the later dev "info"
    assert_eq!(find_item_id(&v).as_deref(), Some("PVTI_t1"));
    // its answer to a block still counts
    assert!(answer_after_block(
        &v[..3],
        Status::Blocked,
        Some("hermes@b")
    ));
    // and its split note still proves the split
    let body = format!("Goal.\n\n{}\n", split_line("o/r#6", "Open part"));
    assert!(split_off_proven(&body, &v, "hermes@b", "o/r#5"));
    // the same comments judged with the present tense role alone would not
    let now: Vec<Role> = ["claude@a", "hermes@b"]
        .iter()
        .map(|n| trust.get(n).unwrap().role)
        .collect();
    assert_eq!(now, [Role::Dev, Role::Coordinator]);
}

#[test]
fn only_valid_comments_become_messages() {
    let w = w();
    let good = by_b(&w, 10, "coordinator", Kind::Ack, "taken");
    let mut edited = by_b(&w, 11, "coordinator", Kind::Ack, "x");
    edited.updated = "2026-10-15T15:00:00Z".into();
    let mut tampered = by_b(&w, 12, "coordinator", Kind::Done, "pr: u");
    tampered.body = tampered.body.replace("pr: u", "pr: v");
    let stranger = signed(
        13,
        "owner-login",
        &generate_key().unwrap(),
        "codex@c",
        "owner",
        Kind::Info,
        "x",
    );
    let mut foreign = by_b(&w, 14, "coordinator", Kind::Ack, "x");
    foreign.author = "mallory".into();
    let mallory_plain = comment(15, "mallory", "I am the owner");
    let ownerc = comment(16, "owner-login", "use crate x");
    let v = msgs(
        &w,
        &[
            good,
            edited,
            tampered,
            stranger,
            foreign,
            mallory_plain,
            ownerc,
        ],
    );
    let ids: Vec<u64> = v.iter().map(|m| m.comment_id).collect();
    assert_eq!(ids, vec![10, 16]);
    assert_eq!(v[1].sender, Sender::Owner);
}

#[test]
fn blocked_needs_an_owner_message_or_a_coordinator_answer_after_the_blocked_message() {
    let w = w();
    let blocked = by_b(&w, 10, "coordinator", Kind::Blocked, "need creds");
    // nothing after: no answer
    assert!(!answer_after_block(
        &msgs(&w, std::slice::from_ref(&blocked)),
        Status::Blocked,
        Some("hermes@b")
    ));
    // an owner message after: answered
    let owner_msg = comment(11, "owner-login", "use the token in the vault");
    assert!(answer_after_block(
        &msgs(&w, &[blocked.clone(), owner_msg.clone()]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // a coordinator answer after: answered
    let ans = by_a(&w, 12, "hermes@b", Kind::Answer, "here you go");
    assert!(answer_after_block(
        &msgs(&w, &[blocked.clone(), ans]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // a coordinator message of another kind is not an answer
    let info = by_a(&w, 13, "hermes@b", Kind::Info, "fyi");
    assert!(!answer_after_block(
        &msgs(&w, &[blocked.clone(), info]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // an answer BEFORE the blocked message does not count
    let early = comment(5, "owner-login", "earlier remark");
    assert!(!answer_after_block(
        &msgs(&w, &[early, blocked.clone()]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // the dev's own message is not an answer
    let own = by_b(&w, 14, "coordinator", Kind::Report, "still stuck");
    assert!(!answer_after_block(
        &msgs(&w, &[blocked.clone(), own]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // a second block needs a second answer
    let again = by_b(&w, 15, "coordinator", Kind::Blocked, "blocked again");
    assert!(!answer_after_block(
        &msgs(&w, &[blocked, owner_msg, again]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // no blocked message at all
    assert!(!answer_after_block(&[], Status::Blocked, Some("hermes@b")));
}

#[test]
fn park_messages_are_parsed_strictly() {
    let parked = |n| BlockReason::Parked { for_issue: n };
    let block_reason_of_body =
        |body: &str| match crate::protocol::acts::parse(Some(Kind::Blocked), body) {
            Act::Parked { for_issue } => parked(for_issue),
            _ => BlockReason::Plain,
        };
    assert_eq!(
        block_reason_of_body(&crate::protocol::acts::park_body(49, "P0 first")),
        parked(49)
    );
    assert_eq!(block_reason_of_body("parked for #7: x"), parked(7));
    for bad in [
        "parked for #: x",
        "parked for #49",
        "parked for 49: x",
        "need creds",
        " parked for #4: x",
        "parked for #4x: y",
        "parked for #99999999999999999999999: y",
    ] {
        assert_eq!(block_reason_of_body(bad), BlockReason::Plain, "{bad}");
    }
}

#[test]
fn a_park_by_the_assignee_is_accepted_and_by_another_agent_it_is_not() {
    let w = w();
    let park = by_b(
        &w,
        10,
        "coordinator",
        Kind::Blocked,
        "parked for #49: P0 arrived",
    );
    assert_eq!(parked_for(&msgs(&w, &[park]), "hermes@b"), Some(49));
    // the same words from another agent do not count for hermes@b
    let foreign = by_a(
        &w,
        11,
        "hermes@b",
        Kind::Blocked,
        "parked for #49: P0 arrived",
    );
    assert_eq!(parked_for(&msgs(&w, &[foreign]), "hermes@b"), None);
    // a normal block is no park
    let normal = by_b(&w, 12, "coordinator", Kind::Blocked, "need creds");
    assert_eq!(
        parked_for(&msgs(&w, std::slice::from_ref(&normal)), "hermes@b"),
        None
    );
    // the LATEST blocked message decides: a normal block after a park
    let park = by_b(&w, 10, "coordinator", Kind::Blocked, "parked for #49: x");
    assert_eq!(
        parked_for(&msgs(&w, &[park.clone(), normal]), "hermes@b"),
        None
    );
    // a message of another kind after the park does not hide it
    let info = by_b(&w, 13, "coordinator", Kind::Report, "still on #49");
    assert_eq!(parked_for(&msgs(&w, &[park, info]), "hermes@b"), Some(49));
    assert_eq!(parked_for(&[], "hermes@b"), None);
}

#[test]
fn a_question_needs_an_owner_message_or_a_withdrawing_answer() {
    let w = w();
    let q = by_b(&w, 10, "owner", Kind::Question, "which API?");
    let coord_answer = by_a(&w, 11, "hermes@b", Kind::Answer, "I think v2");
    assert!(!answer_after_block(
        &msgs(&w, &[q.clone(), coord_answer.clone()]),
        Status::Blocked,
        Some("hermes@b")
    ));
    let owner_msg = comment(12, "owner-login", "v3");
    assert!(answer_after_block(
        &msgs(&w, &[q.clone(), owner_msg.clone()]),
        Status::Blocked,
        Some("hermes@b")
    ));
    let withdraw = by_a(
        &w,
        13,
        "hermes@b",
        Kind::Answer,
        "take it back\nwithdraws: question\n",
    );
    assert!(answer_after_block(
        &msgs(&w, &[q.clone(), withdraw]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // a `blocked` message after the question is answered by a plain coordinator answer
    let blocked = by_b(&w, 14, "coordinator", Kind::Blocked, "need creds");
    assert!(answer_after_block(
        &msgs(
            &w,
            &[
                q.clone(),
                blocked.clone(),
                by_a(&w, 15, "hermes@b", Kind::Answer, "here")
            ]
        ),
        Status::Blocked,
        Some("hermes@b")
    ));
    assert!(!answer_after_block(
        &msgs(&w, &[q.clone(), blocked]),
        Status::Blocked,
        Some("hermes@b")
    ));
    // other statuses never have an answer
    assert!(!answer_after_block(
        &[],
        Status::InProgress,
        Some("hermes@b")
    ));
}

#[test]
fn a_question_to_the_coordinator_does_not_block_a_ticket() {
    let w = w();
    let q = by_b(&w, 10, "coordinator", Kind::Question, "which API?");
    assert!(question_open(&msgs(&w, std::slice::from_ref(&q)), Some("coordinator")).is_none());
    let blocked = by_b(&w, 11, "coordinator", Kind::Blocked, "stuck");
    let answer = by_a(&w, 12, "hermes@b", Kind::Answer, "use v2");
    assert!(answer_after_block(
        &msgs(&w, &[blocked, q, answer]),
        Status::Blocked,
        Some("hermes@b")
    ));
}

#[test]
fn a_question_of_a_dev_that_is_not_the_assignee_opens_and_blocks_nothing() {
    let w = w();
    let q = by_b(&w, 10, "owner", Kind::Question, "which API?");
    let m = msgs(&w, &[q]);
    assert!(question_open(&m, Some("other@c")).is_none());
    assert!(question_open(&m, None).is_none());
    assert!(question_open(&m, Some("hermes@b")).is_some());
    // a blocked ticket is answered by a plain coordinator answer, as the question does not count
    let blocked = by_b(&w, 11, "coordinator", Kind::Blocked, "stuck");
    let q2 = by_b(&w, 12, "owner", Kind::Question, "again?");
    let answer = by_a(&w, 13, "hermes@b", Kind::Answer, "go on");
    let m = msgs(&w, &[blocked, q2, answer]);
    assert!(!answer_after_block(&m, Status::Blocked, Some("hermes@b")));
    assert!(answer_after_block(&m, Status::Blocked, Some("other@c")));
}

#[test]
fn a_question_is_open_until_the_owner_or_a_withdrawing_coordinator_answers() {
    let w = w();
    let q = by_b(&w, 10, "owner", Kind::Question, "which API?");
    let m = msgs(&w, std::slice::from_ref(&q));
    assert_eq!(
        question_open(&m, Some("hermes@b")).map(|x| x.comment_id),
        Some(10)
    );
    assert!(question_open(&[], Some("hermes@b")).is_none());
    // the owner answers on the ticket
    let owner_msg = comment(11, "owner-login", "v3");
    assert!(question_open(&msgs(&w, &[q.clone(), owner_msg.clone()]), Some("hermes@b")).is_none());
    // a withdrawing answer of the coordinator
    let withdraw = by_a(
        &w,
        12,
        "hermes@b",
        Kind::Answer,
        "back\nwithdraws: question\n",
    );
    assert!(question_open(&msgs(&w, &[q.clone(), withdraw]), Some("hermes@b")).is_none());
    // a plain answer of the coordinator, an info, a dev message and a dev answer do not
    let plain = by_a(&w, 13, "hermes@b", Kind::Answer, "I think v2");
    let info = by_a(&w, 14, "hermes@b", Kind::Info, "withdraws: question");
    let dev = by_b(&w, 15, "coordinator", Kind::Report, "still waiting");
    let dev_answer = by_b(&w, 16, "owner", Kind::Answer, "withdraws: question");
    let m = msgs(&w, &[q.clone(), plain, info, dev, dev_answer]);
    assert_eq!(
        question_open(&m, Some("hermes@b")).map(|x| x.comment_id),
        Some(10)
    );
    // the latest question wins: a new question after the owner's answer is open again
    let q2 = by_b(&w, 20, "owner", Kind::Question, "and the schema?");
    let m = msgs(&w, &[q, owner_msg, q2]);
    assert_eq!(
        question_open(&m, Some("hermes@b")).map(|x| x.comment_id),
        Some(20)
    );
}

#[test]
fn evidence_round_trips_and_the_latest_done_of_the_assignee_wins() {
    let w = w();
    let e1 = EvidenceMsg {
        pr: "https://github.com/a/b/pull/1".into(),
        sha: "a".repeat(40),
        ci: "none".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    };
    let e2 = EvidenceMsg {
        sha: "b".repeat(40),
        ..e1.clone()
    };
    let d1 = by_b(&w, 10, "coordinator", Kind::Done, &done_body(&e1, ""));
    let d2 = by_b(
        &w,
        20,
        "coordinator",
        Kind::Done,
        &done_body(&e2, "second try"),
    );
    let v = msgs(&w, &[d1, d2]);
    let (m, e) = latest_done(&v, "hermes@b").unwrap();
    assert_eq!((m.comment_id, e.sha.clone()), (20, "b".repeat(40)));
    assert!(latest_done(&v, "other@x").is_none());
    // a done without all three parts is not evidence
    let bad = by_b(&w, 30, "coordinator", Kind::Done, "pr: u\nsha: s");
    assert!(latest_done(&msgs(&w, &[bad]), "hermes@b").is_none());
    assert_eq!(
        evidence_field(&e1),
        format!(
            "pr=https://github.com/a/b/pull/1 sha={} ci=none",
            "a".repeat(40)
        )
    );
}

#[test]
fn an_override_line_in_a_done_body_does_not_change_the_evidence() {
    let e = EvidenceMsg {
        pr: "https://github.com/a/b/pull/1".into(),
        sha: "a".repeat(40),
        ci: "https://github.com/a/b/actions/runs/1".into(),
        ci_sources: Some("workflow-runs".into()),
        ci_runs: None,
        local_gate: None,
    };
    let plain = done_body(&e, "owner-route: x.patch\n");
    let line = "override: unread-message-on-ticket (comments 123, 456)\n";
    let with = done_body(&e, &format!("owner-route: x.patch\n{line}"));
    assert!(with.contains(line));
    assert!(!plain.contains("override:"));
    // both directions: the same evidence with and without the line
    assert_eq!(parse_evidence(&plain), Some(e.clone()));
    assert_eq!(parse_evidence(&with), Some(e.clone()));
    assert_eq!(
        evidence_field(&parse_evidence(&with).unwrap()),
        evidence_field(&e)
    );
}

#[test]
fn the_ci_sources_are_recorded_in_the_message_and_the_board_field_and_read_back() {
    let w = w();
    let e = EvidenceMsg {
        pr: "https://github.com/a/b/pull/1".into(),
        sha: "a".repeat(40),
        ci: "https://github.com/a/b/actions/runs/1".into(),
        ci_sources: Some("workflow-runs, statuses; unreadable: check-runs".into()),
        ci_runs: None,
        local_gate: None,
    };
    let body = done_body(&e, "");
    assert!(body.contains("ci-sources: workflow-runs, statuses; unreadable: check-runs\n"));
    assert!(
        evidence_field(&e).ends_with("ci-sources=workflow-runs, statuses; unreadable: check-runs")
    );
    let d = by_b(&w, 10, "coordinator", Kind::Done, &body);
    let v = msgs(&w, &[d]);
    let (_, back) = latest_done(&v, "hermes@b").unwrap();
    assert_eq!(back, e);
    // control: without sources no line is written and none is read
    let plain = EvidenceMsg {
        ci_sources: None,
        ..e
    };
    assert!(!done_body(&plain, "").contains("ci-sources"));
    assert_eq!(
        parse_evidence(&done_body(&plain, "")).unwrap().ci_sources,
        None
    );
}

fn local_evidence(sha: &str) -> EvidenceMsg {
    EvidenceMsg {
        pr: "https://github.com/a/b/pull/1".into(),
        sha: sha.into(),
        ci: LOCAL_GATE_CI.into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: Some("host-a: make test rc=0, log /tmp/gate.log".into()),
    }
}

#[test]
fn a_local_gate_is_recorded_in_the_message_and_the_field_and_read_back() {
    let w = w();
    let e = local_evidence(&"a".repeat(40));
    let body = done_body(&e, "");
    assert!(body.contains("\nci: local-gate\n"), "{body}");
    assert!(body.contains("\nlocal-gate: host-a: make test rc=0, log /tmp/gate.log\n"));
    assert!(body.contains(LOCAL_GATE_OWNER_LINE));
    assert_eq!(parse_evidence(&body), Some(e.clone()));
    assert_eq!(
        evidence_field(&e),
        format!(
            "pr=https://github.com/a/b/pull/1 sha={} ci=local-gate local-gate=host-a: make test rc=0, log /tmp/gate.log",
            "a".repeat(40)
        )
    );
    let d = by_b(&w, 10, "coordinator", Kind::Done, &body);
    let (_, back) = latest_done(&msgs(&w, &[d]), "hermes@b").unwrap();
    assert!(is_local_gate(&back));
    // control: CI evidence has no gate line and is no local gate
    let plain = EvidenceMsg {
        ci: "none".into(),
        ..e
    };
    assert!(!done_body(&plain, "").contains("local-gate"));
    assert!(!is_local_gate(&plain));
}

#[test]
fn the_verify_body_carries_both_gate_lines_and_merge_reads_the_passed_one_on_the_same_head() {
    let w = w();
    let sha = "a".repeat(40);
    let e = local_evidence(&sha);
    let done = by_b(&w, 10, "coordinator", Kind::Done, &done_body(&e, ""));
    let report = |id: u64, pass: bool, head: &str, gate: Option<&str>| {
        let body = verify_body(pass, &e, head, &[], "", None, gate);
        let mut c = by_a(&w, id, "hermes@b", Kind::Report, &body);
        let msg = crate::protocol::signature::sign_message(
            &w.ka,
            &c.issue_ref(),
            crate::protocol::signature::Outgoing {
                from: crate::protocol::names::AgentName::parse("claude@a").unwrap(),
                to: crate::protocol::names::To::parse("hermes@b").unwrap(),
                kind: Kind::Report,
                re: Re::Comment {
                    issue: c.issue,
                    comment: 10,
                },
                ts: crate::protocol::names::Timestamp::parse(&crate::clock::format_ts(
                    c.created_epoch,
                ))
                .unwrap(),
            },
            &body,
        )
        .unwrap();
        c.body = msg.to_comment();
        c
    };
    let gate = "host-c: make test rc=0";
    let body = verify_body(true, &e, &sha, &[], "", None, Some(gate));
    assert!(body.contains("\nlocal-gate: host-a: make test rc=0, log /tmp/gate.log\n"));
    assert!(body.contains(&format!("\ncoordinator-local-gate: {gate}\n")));
    let pass_of = |cs: Vec<Comment>| {
        let mut all = vec![done.clone()];
        all.extend(cs);
        let v = msgs(&w, &all);
        let d = v.iter().find(|m| m.comment_id == 10).unwrap().clone();
        passed_coordinator_local_gate(&v, &d, &e, all[0].issue)
    };
    assert_eq!(
        pass_of(vec![report(11, true, &sha, Some(gate))]).as_deref(),
        Some(gate)
    );
    // none of: no coordinator line, a failed verify, a head that is not the evidence's
    assert_eq!(pass_of(vec![report(11, true, &sha, None)]), None);
    assert_eq!(pass_of(vec![report(11, false, &sha, Some(gate))]), None);
    assert_eq!(
        pass_of(vec![report(11, true, &"b".repeat(40), Some(gate))]),
        None
    );
    assert_eq!(pass_of(vec![]), None);
}

#[test]
fn a_verify_result_counts_only_for_its_own_done_message_and_from_the_coordinator() {
    let w = w();
    let e = EvidenceMsg {
        pr: "p".into(),
        sha: "s".into(),
        ci: "none".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    };
    let d1 = by_b(&w, 10, "coordinator", Kind::Done, &done_body(&e, ""));
    let v = |id: u64, pass: bool, re: u64| {
        let mut c = by_a(
            &w,
            id,
            "hermes@b",
            Kind::Report,
            &verify_body(pass, &e, "s", &[], "", None, None),
        );
        // rebuild with the re field: sign again through a message with re
        let issue = c.issue;
        let msg = crate::protocol::signature::sign_message(
            &w.ka,
            &c.issue_ref(),
            crate::protocol::signature::Outgoing {
                from: crate::protocol::names::AgentName::parse("claude@a").unwrap(),
                to: crate::protocol::names::To::parse("hermes@b").unwrap(),
                kind: Kind::Report,
                re: Re::Comment { issue, comment: re },
                ts: crate::protocol::names::Timestamp::parse(&crate::clock::format_ts(
                    c.created_epoch,
                ))
                .unwrap(),
            },
            &verify_body(pass, &e, "s", &[], "", None, None),
        )
        .unwrap();
        c.body = msg.to_comment();
        c
    };
    let all = msgs(&w, &[d1.clone(), v(11, true, 10)]);
    let done = all.iter().find(|m| m.comment_id == 10).unwrap();
    assert_eq!(verify_result(&all, done, 2), Some(true));
    let fail = msgs(&w, &[d1.clone(), v(11, false, 10)]);
    assert_eq!(
        verify_result(&fail, fail.iter().find(|m| m.comment_id == 10).unwrap(), 2),
        Some(false)
    );
    // a pass for ANOTHER done message does not verify this one
    let other = msgs(&w, &[d1.clone(), v(11, true, 99)]);
    assert_eq!(
        verify_result(
            &other,
            other.iter().find(|m| m.comment_id == 10).unwrap(),
            2
        ),
        None
    );
    // nothing yet
    let none = msgs(&w, std::slice::from_ref(&d1));
    assert_eq!(verify_result(&none, &none[0], 2), None);
    // a "verify: pass" written by the dev itself is not a verify
    let mut dev_fake = by_b(
        &w,
        12,
        "coordinator",
        Kind::Report,
        &verify_body(true, &e, "s", &[], "", None, None),
    );
    dev_fake.body = dev_fake.body.clone();
    let fake = msgs(&w, &[d1, dev_fake]);
    assert_eq!(
        verify_result(&fake, fake.iter().find(|m| m.comment_id == 10).unwrap(), 2),
        None
    );
}

#[test]
fn a_ticket_known_only_by_its_signed_info_comment_is_found_and_a_forged_one_is_not() {
    let w = w();
    let real = by_a(&w, 10, "owner", Kind::Info, &info_body("PVTI_real", 15));
    // the developer cannot announce a ticket: only the coordinator's info counts
    let dev = by_b(&w, 12, "owner", Kind::Info, &info_body("PVTI_dev", 15));
    // an edited message fails its signature
    let mut tampered = by_a(&w, 13, "owner", Kind::Info, &info_body("PVTI_x", 15));
    tampered.body = tampered.body.replace("PVTI_x", "PVTI_forged");
    let got = announced_item_ids(
        &[real.clone(), dev.clone(), tampered.clone()],
        &w.cfg,
        &w.trust,
    );
    assert_eq!(got.len(), 1, "{got:?}");
    let (key, id) = got.iter().next().unwrap();
    assert_eq!(id, "PVTI_real");
    assert!(key.ends_with("#2"), "{key}");
    // control: without the real announcement nothing is found
    assert!(announced_item_ids(&[dev, tampered], &w.cfg, &w.trust).is_empty());
    // control: no comment, no ticket
    assert!(announced_item_ids(&[], &w.cfg, &w.trust).is_empty());
}

#[test]
fn a_done_needs_a_verify_until_the_coordinator_answers_that_very_message() {
    let w = w();
    let e = EvidenceMsg {
        pr: "p".into(),
        sha: "s".into(),
        ci: "none".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    };
    let verify = |id: u64, pass: bool, re: u64| {
        let mut c = by_a(&w, id, "hermes@b", Kind::Report, "x");
        let msg = crate::protocol::signature::sign_message(
            &w.ka,
            &c.issue_ref(),
            crate::protocol::signature::Outgoing {
                from: crate::protocol::names::AgentName::parse("claude@a").unwrap(),
                to: crate::protocol::names::To::parse("hermes@b").unwrap(),
                kind: Kind::Report,
                re: Re::Comment {
                    issue: c.issue,
                    comment: re,
                },
                ts: crate::protocol::names::Timestamp::parse(&crate::clock::format_ts(
                    c.created_epoch,
                ))
                .unwrap(),
            },
            &verify_body(pass, &e, "s", &[], "", None, None),
        )
        .unwrap();
        c.body = msg.to_comment();
        c
    };
    let d1 = by_b(&w, 10, "coordinator", Kind::Done, &done_body(&e, ""));
    // no done: nothing to verify
    assert!(!needs_verify(&msgs(&w, &[]), "hermes@b", 2));
    // a done without a verify appears
    assert!(needs_verify(
        &msgs(&w, std::slice::from_ref(&d1)),
        "hermes@b",
        2
    ));
    // a verified one does not (pass or fail: both answer the message)
    assert!(!needs_verify(
        &msgs(&w, &[d1.clone(), verify(11, true, 10)]),
        "hermes@b",
        2
    ));
    assert!(!needs_verify(
        &msgs(&w, &[d1.clone(), verify(11, false, 10)]),
        "hermes@b",
        2
    ));
    // a verify of an earlier done does not cover a later done
    let d2 = by_b(&w, 12, "coordinator", Kind::Done, &done_body(&e, "again"));
    assert!(needs_verify(
        &msgs(&w, &[d1.clone(), verify(11, true, 10), d2]),
        "hermes@b",
        2
    ));
    // a done of another agent does not count for this assignee
    assert!(!needs_verify(&msgs(&w, &[d1]), "someone@else", 2));
}

#[test]
fn the_item_id_comes_only_from_a_verified_info_of_the_coordinator() {
    let w = w();
    let good = by_a(&w, 10, "owner", Kind::Info, &info_body("PVTI_real", 15));
    assert_eq!(
        find_item_id(&msgs(&w, std::slice::from_ref(&good))).as_deref(),
        Some("PVTI_real")
    );
    // the dev cannot announce an item
    let dev = by_b(&w, 11, "owner", Kind::Info, &info_body("PVTI_evil", 15));
    assert_eq!(
        find_item_id(&msgs(&w, &[good.clone(), dev])).as_deref(),
        Some("PVTI_real")
    );
    assert_eq!(
        find_item_id(&msgs(
            &w,
            &[by_b(
                &w,
                11,
                "owner",
                Kind::Info,
                &info_body("PVTI_evil", 15)
            )]
        )),
        None
    );
    // an owner comment with the same text is not a signed info
    let plain = comment(12, "owner-login", "item=PVTI_owner");
    assert_eq!(find_item_id(&msgs(&w, &[plain])), None);
    // a value that is no item id is ignored
    let junk = by_a(&w, 13, "owner", Kind::Info, "item=not-an-id");
    assert_eq!(find_item_id(&msgs(&w, &[junk])), None);
    // a forged info (wrong key) is not read
    let forged = signed(
        14,
        "owner-login",
        &w.kb,
        "claude@a",
        "owner",
        Kind::Info,
        &info_body("PVTI_forged", 15),
    );
    assert_eq!(find_item_id(&msgs(&w, &[forged])), None);
}

#[test]
fn an_adopted_branch_comes_only_from_a_signed_info_of_a_coordinator() {
    let w = w();
    let url = "https://github.com/o/r/pull/9";
    let branch = "crew/hermes@b/5-old";
    let good = by_a(
        &w,
        10,
        "owner",
        Kind::Info,
        &info_body_adopting("PVTI_real", 15, url, branch),
    );
    // pass: the item id is still found, and the branch is read for that PR (any letter case)
    let m = msgs(&w, std::slice::from_ref(&good));
    assert_eq!(find_item_id(&m).as_deref(), Some("PVTI_real"));
    assert_eq!(adopted_branch(&m, "o/r", 9).as_deref(), Some(branch));
    assert_eq!(adopted_branch(&m, "O/R", 9).as_deref(), Some(branch));
    // another PR, or a ticket that adopted nothing: no branch
    assert_eq!(adopted_branch(&m, "o/r", 10), None);
    assert_eq!(adopted_branch(&m, "o/other", 9), None);
    let plain = by_a(&w, 11, "owner", Kind::Info, &info_body("PVTI_real", 15));
    assert_eq!(adopted_branch(&msgs(&w, &[plain]), "o/r", 9), None);
    // refusals: a dev agent writes the same line, a plain owner comment, a coordinator
    // message of another kind, a line without a branch
    let forged = by_b(
        &w,
        12,
        "owner",
        Kind::Info,
        &info_body_adopting("PVTI_x", 15, url, branch),
    );
    assert_eq!(adopted_branch(&msgs(&w, &[forged]), "o/r", 9), None);
    let owner = comment(
        13,
        "owner-login",
        &format!("adopts-pr: {url} branch: {branch}"),
    );
    assert_eq!(adopted_branch(&msgs(&w, &[owner]), "o/r", 9), None);
    let say = by_a(
        &w,
        14,
        "hermes@b",
        Kind::Request,
        &format!("adopts-pr: {url} branch: {branch}"),
    );
    assert_eq!(adopted_branch(&msgs(&w, &[say]), "o/r", 9), None);
    let bare = by_a(
        &w,
        15,
        "owner",
        Kind::Info,
        &format!("adopts-pr: {url} branch: "),
    );
    assert_eq!(adopted_branch(&msgs(&w, &[bare]), "o/r", 9), None);
}

#[test]
fn gate_and_ci_lines_are_read_from_the_body_in_the_usual_spellings() {
    let b = "Goal: x\n**Gate:** `cargo test` passes\n- CI: none\n";
    assert_eq!(
        body_field(b, "Gate").as_deref(),
        Some("`cargo test` passes")
    );
    assert!(declares_ci_none(b));
    assert_eq!(body_field("**Gate**: a b", "Gate").as_deref(), Some("a b"));
    assert_eq!(body_field("gate: lower", "Gate").as_deref(), Some("lower"));
    // missing, empty, or only mentioned in a sentence
    for bad in [
        "",
        "Goal: x",
        "Gate:",
        "Gate:   ",
        "The gate is here",
        "Gatekeeper: x",
    ] {
        assert_eq!(body_field(bad, "Gate"), None, "{bad:?}");
    }
    assert!(declares_ci_none("**CI:** none."));
    assert!(declares_ci_none("ci: None"));
    for no in [
        "",
        "CI: required",
        "CI: none of the above",
        "we have no CI: none",
        "CI:",
        "Goal: CI none",
    ] {
        assert!(!declares_ci_none(no), "{no:?}");
    }
    // non-ASCII before the name does not panic
    assert_eq!(body_field("äöüäöü", "Gate"), None);
}

#[test]
fn an_assignment_event_names_the_status() {
    let r = row(1, "Ready", Some("me@x"), None);
    let (ev, _) = ticket_events(
        "me@x",
        false,
        std::slice::from_ref(&r),
        &BTreeMap::new(),
        "t",
    );
    assert_eq!(ev[0].body, "assigned to you; status Ready");
}

fn snaps(rows: &[TicketRow]) -> BTreeMap<String, TicketSnap> {
    rows.iter().map(|r| (r.item_id.clone(), r.snap())).collect()
}

fn kinds(e: &[Event]) -> Vec<&str> {
    e.iter().map(|e| e.kind.as_str()).collect()
}

#[test]
fn assignment_status_and_unassignment_events_for_a_dev() {
    let before = [row(1, "Backlog", None, None)];
    let prev = snaps(&before);
    // assigned to me
    let (e, next) = ticket_events(
        "hermes@b",
        false,
        &[row(1, "Ready", Some("hermes@b"), None)],
        &prev,
        "t",
    );
    assert_eq!(kinds(&e), vec!["assignment"]);
    assert!(!e[0].verified && e[0].class == EventClass::Ticket);
    // the same state again: nothing (the snapshot was stored)
    let (e2, _) = ticket_events(
        "hermes@b",
        false,
        &[row(1, "Ready", Some("hermes@b"), None)],
        &next,
        "t",
    );
    assert!(e2.is_empty());
    // status change of my ticket (the owner moved it)
    let (e3, next3) = ticket_events(
        "hermes@b",
        false,
        &[row(1, "Blocked", Some("hermes@b"), None)],
        &next,
        "t",
    );
    assert_eq!(kinds(&e3), vec!["status-changed"]);
    assert!(e3[0].body.contains("Ready -> Blocked"));
    // unassigned
    let (e4, _) = ticket_events(
        "hermes@b",
        false,
        &[row(1, "Ready", Some("codex@c"), None)],
        &next3,
        "t",
    );
    assert_eq!(kinds(&e4), vec!["unassigned"]);
    // status change of someone else's ticket: not for a dev (control for the filter)
    let other = [row(2, "Ready", Some("codex@c"), None)];
    let (e5, _) = ticket_events(
        "hermes@b",
        false,
        &[row(2, "In Progress", Some("codex@c"), None)],
        &snaps(&other),
        "t",
    );
    assert!(e5.is_empty());
}

#[test]
fn event_text_is_plain_text_without_html_entities() {
    let prev = snaps(&[row(2, "Ready -&gt; x", Some("codex@c"), None)]);
    let (e, _) = ticket_events(
        "claude@a",
        true,
        &[row(2, "In &amp;lt; Progress", Some("codex@c"), None)],
        &prev,
        "t",
    );
    assert_eq!(e.len(), 1);
    // decoded once: `&amp;lt;` becomes `&lt;`, `-&gt;` becomes `->`
    assert_eq!(e[0].body, "status Ready -> x -> In &lt; Progress");
    // pass: a plain status is unchanged
    let prev = snaps(&[row(3, "Ready", Some("codex@c"), None)]);
    let (e, _) = ticket_events(
        "claude@a",
        true,
        &[row(3, "Review", Some("codex@c"), None)],
        &prev,
        "t",
    );
    assert_eq!(e[0].body, "status Ready -> Review");
}

#[test]
fn the_coordinator_sees_every_status_change_and_a_fresh_cache_does_not_replay_history() {
    let prev = snaps(&[row(2, "Ready", Some("codex@c"), None)]);
    let (e, _) = ticket_events(
        "claude@a",
        true,
        &[row(2, "Review", Some("codex@c"), None)],
        &prev,
        "t",
    );
    assert_eq!(kinds(&e), vec!["status-changed"]);
    // first sight: no event for the coordinator, none for a Done or Dropped ticket of mine,
    // one for an open ticket of mine
    let empty = BTreeMap::new();
    let rows = [
        row(3, "Review", Some("codex@c"), None),
        row(4, "Done", Some("hermes@b"), None),
        row(5, "In Progress", Some("hermes@b"), None),
        row(6, "Dropped", Some("hermes@b"), None),
    ];
    let (c, _) = ticket_events("claude@a", true, &rows, &empty, "t");
    assert!(c.is_empty());
    let (d, next) = ticket_events("hermes@b", false, &rows, &empty, "t");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].issue, 5);
    assert_eq!(next.len(), 4, "every ticket is remembered");
}

#[test]
fn a_later_comment_with_the_same_signature_is_dropped_from_the_log() {
    let w = w();
    let first = by_a(&w, 10, "hermes@b", Kind::Answer, "go ahead");
    let mut copy = first.clone();
    copy.id = 14;
    let other = by_a(&w, 15, "hermes@b", Kind::Answer, "go ahead, now");
    let ids: Vec<u64> = msgs(&w, &[copy, first, other])
        .iter()
        .map(|m| m.comment_id)
        .collect();
    assert_eq!(
        ids,
        vec![10, 15],
        "the copy (14) is dropped, in any input order"
    );
}

#[test]
fn the_owner_route_line_has_one_parser_that_accepts_an_indented_line() {
    let line = "pr: u\nsha: s\nci: none\nowner-route: docs/patches/5-workflow.patch\n";
    assert_eq!(
        owner_route_patch(line).as_deref(),
        Some("docs/patches/5-workflow.patch")
    );
    assert_eq!(
        owner_route_patch("pr: u\n  owner-route: x.patch trailing\n").as_deref(),
        Some("x.patch")
    );
    assert_eq!(owner_route_patch("pr: u\nsha: s\nci: none\n"), None);
    // a line without a word names nothing
    assert_eq!(owner_route_patch("owner-route:\n"), None);
}

#[test]
fn an_owner_move_counts_only_from_the_owner_and_not_after_an_edit_by_another_account() {
    use crate::ticket::log::{Expected, fold_ticket};
    let w = w();
    let base = vec![
        by_a(&w, 10, "hermes@b", Kind::Request, "Assigned to you: t"),
        by_b(&w, 11, "coordinator", Kind::Ack, "Taken."),
    ];
    let status_with = |extra: Comment| {
        let mut cs = base.clone();
        cs.push(extra);
        fold_ticket(&msgs(&w, &cs)).status
    };
    let edited = |editors: &[&str]| {
        let mut c = comment(12, "owner-login", "/crew move Backlog");
        c.updated = "2026-10-15T14:09:00Z".into();
        c.editors = Some(editors.iter().map(|x| x.to_string()).collect());
        c
    };
    // control: the owner's move counts, also after an edit by the owner
    assert_eq!(
        status_with(comment(12, "owner-login", "/crew move Backlog")),
        Expected::Is(Status::Backlog)
    );
    assert_eq!(
        status_with(edited(&["owner-login"])),
        Expected::Is(Status::Backlog)
    );
    // an edit by another account, and the same line by another account, change nothing
    assert_eq!(
        status_with(edited(&["owner-login", "crew-bot"])),
        Expected::Is(Status::InProgress)
    );
    assert_eq!(
        status_with(comment(12, "crew-bot", "/crew move Backlog")),
        Expected::Is(Status::InProgress)
    );
}
