use super::*;
use crate::comments::tests::comment;
use crate::protocol::signature::generate_key;
use crate::registry::resolve;
use crate::registry::tests::{cfg, join_comment, owner, signed};
use ssh_key::PrivateKey;

struct W {
    ka: PrivateKey,
    kb: PrivateKey,
    kc: PrivateKey,
    reg: Registry,
    cfg: BoardConfig,
}

/// claude@a coordinator, hermes@b and codex@c dev; all approved.
fn w() -> W {
    let (ka, kb, kc) = (
        generate_key().unwrap(),
        generate_key().unwrap(),
        generate_key().unwrap(),
    );
    let cfg = cfg();
    let reg = resolve(
        &cfg,
        &[
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            join_comment(3, "codex@c", &kc, "PVTI_c"),
            owner(4, "/crew approve claude@a role=coordinator"),
            owner(5, "/crew approve hermes@b role=dev"),
            owner(6, "/crew approve codex@c role=dev"),
        ],
    );
    W {
        ka,
        kb,
        kc,
        reg,
        cfg,
    }
}

/// A comment of issue 7 (a ticket) `secs` seconds after the base time of the helpers.
fn at(mut c: Comment, secs: i64) -> Comment {
    c.issue = 7;
    c.created_epoch += secs;
    c.updated_epoch = c.created_epoch;
    c.url = format!("https://github.com/x/y/issues/7#issuecomment-{}", c.id);
    c
}

/// A message signed for issue 7, created `id` seconds after the base time.
fn by(_w: &W, k: &PrivateKey, from: &str, id: u64, to: &str, kind: Kind, body: &str) -> Comment {
    use crate::protocol::names::{AgentName, Re, Timestamp, To};
    use crate::protocol::signature::{Outgoing, sign_message};
    let mut c = at(comment(id, "owner-login", ""), id as i64);
    let msg = sign_message(
        k,
        &c.issue_ref(),
        Outgoing {
            from: AgentName::parse(from).unwrap(),
            to: To::parse(to).unwrap(),
            kind,
            re: Re::None,
            ts: Timestamp::parse(&crate::clock::format_ts(c.created_epoch)).unwrap(),
        },
        body,
    )
    .unwrap();
    c.body = msg.to_comment();
    c
}

fn run(w: &W, cs: &[Comment]) -> Report {
    audit(
        &w.cfg,
        &w.reg,
        cs,
        &BTreeMap::new(),
        60,
        &HashSet::new(),
        &BoardView::default(),
    )
}

fn kinds(r: &Report) -> Vec<(u64, FindingKind)> {
    r.findings.iter().map(|f| (f.comment_id, f.kind)).collect()
}

#[test]
fn a_headerless_owner_comment_inside_an_agent_session_is_listed_and_one_outside_is_not() {
    let w = w();
    let base = comment(1, "x", "x").created_epoch;
    let a1 = by(&w, &w.ka, "claude@a", 10, "owner", Kind::Info, "first");
    let a2 = by(&w, &w.ka, "claude@a", 30, "owner", Kind::Info, "last");
    // inside the window [base+10, base+30+60]
    let inside = at(comment(20, "owner-login", "use the other library"), 20);
    // long before the first and long after the last message plus the grace
    let before = at(comment(5, "owner-login", "early remark"), -1000);
    let after = at(comment(40, "owner-login", "late remark"), 1000);
    let r = run(&w, &[a1, a2, inside, before, after]);
    assert_eq!(kinds(&r), vec![(20, FindingKind::HeaderlessOwner)]);
    assert_eq!(r.owner_comments_outside_sessions, 2);
    assert!(
        r.findings[0].detail.contains("claude@a ran"),
        "{}",
        r.findings[0].detail
    );
    assert_eq!(r.windows.len(), 1);
    assert_eq!(r.windows[0].from, base + 10);
    assert_eq!(r.windows[0].to, base + 30 + 60);
}

#[test]
fn an_owner_command_is_listed_as_such_and_a_heartbeat_extends_the_window() {
    let w = w();
    let base = comment(1, "x", "x").created_epoch;
    let a1 = by(&w, &w.ka, "claude@a", 10, "owner", Kind::Info, "x");
    let cmd = at(comment(20, "owner-login", "/crew approve hermes@b"), 500);
    let r = run(&w, &[a1.clone(), cmd.clone()]);
    assert!(r.findings.is_empty(), "outside: {:?}", r.findings);
    // the agent is running now (heartbeat at base+520): the window reaches it
    let mut hb = BTreeMap::new();
    hb.insert("claude@a".to_string(), base + 520);
    let r = audit(
        &w.cfg,
        &w.reg,
        &[a1, cmd],
        &hb,
        60,
        &HashSet::new(),
        &BoardView::default(),
    );
    assert_eq!(kinds(&r), vec![(20, FindingKind::HeaderlessOwner)]);
    assert!(
        r.findings[0].detail.starts_with("owner command"),
        "{}",
        r.findings[0].detail
    );
}

#[test]
fn signed_messages_and_foreign_plain_comments_are_not_headerless_owner_comments() {
    let w = w();
    let a = by(&w, &w.ka, "claude@a", 10, "owner", Kind::Info, "valid");
    let stranger = at(comment(20, "mallory", "I am the owner"), 15);
    let r = run(&w, &[a, stranger]);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert_eq!(r.foreign_comments, 1);
}

#[test]
fn a_bad_signature_an_edit_an_unknown_sender_and_a_foreign_header_are_found() {
    let w = w();
    let good = by(
        &w,
        &w.ka,
        "claude@a",
        10,
        "hermes@b",
        Kind::Report,
        "original",
    );
    // tampered: the body of a real message changed
    let mut tampered = by(
        &w,
        &w.ka,
        "claude@a",
        11,
        "hermes@b",
        Kind::Report,
        "original",
    );
    tampered.body = tampered.body.replace("original", "tampered");
    // signed with a key nobody registered
    let wrong = by(
        &w,
        &generate_key().unwrap(),
        "claude@a",
        12,
        "hermes@b",
        Kind::Report,
        "x",
    );
    // edited after posting
    let mut edited = by(&w, &w.ka, "claude@a", 13, "hermes@b", Kind::Report, "x");
    edited.updated = "2026-10-15T15:00:00Z".into();
    // an agent that was never approved
    let ghost = by(
        &w,
        &generate_key().unwrap(),
        "ghost@z",
        14,
        "owner",
        Kind::Info,
        "x",
    );
    // a crew header by a stranger
    let mut foreign = by(&w, &w.ka, "claude@a", 15, "hermes@b", Kind::Report, "x");
    foreign.author = "mallory".into();
    // a broken header
    let broken = at(
        comment(16, "owner-login", "<!-- crew v=1 from=?? -->\nbody"),
        16,
    );
    let r = run(&w, &[good, tampered, wrong, edited, ghost, foreign, broken]);
    assert_eq!(
        kinds(&r),
        vec![
            (11, FindingKind::BadSignature),
            (12, FindingKind::BadSignature),
            (13, FindingKind::Edited),
            (14, FindingKind::UnknownSender),
            (15, FindingKind::ForeignAuthor),
            (16, FindingKind::Malformed),
        ]
    );
    // the valid message is not a finding (the check can say "fine")
    assert!(!kinds(&r).iter().any(|(id, _)| *id == 10));
    for f in &r.findings {
        assert!(f.kind.is_security_event(), "{:?}", f.kind);
        assert_eq!(f.reported_to_owner, Some(false));
    }
}

#[test]
fn a_message_of_an_agent_that_has_left_since_still_verifies() {
    // history: the registry key is the one the owner approved, whatever State says now
    let w = w();
    let m = by(
        &w,
        &w.kb,
        "hermes@b",
        10,
        "coordinator",
        Kind::Ack,
        "Taken.",
    );
    let assign = by(
        &w,
        &w.ka,
        "claude@a",
        9,
        "hermes@b",
        Kind::Request,
        "Assigned to you: t",
    );
    let r = run(&w, &[assign, m]);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[test]
fn a_message_signed_after_the_leave_marker_is_unknown_and_a_role_change_keeps_history() {
    use crate::registry::tests::left_marker;
    let w = w();
    // hermes@b leaves at comment 20; claude@a is made a dev at 25 and codex@c the coordinator
    let reg = resolve(
        &w.cfg,
        &[
            join_comment(1, "claude@a", &w.ka, "PVTI_a"),
            join_comment(2, "hermes@b", &w.kb, "PVTI_b"),
            join_comment(3, "codex@c", &w.kc, "PVTI_c"),
            owner(4, "/crew approve claude@a role=coordinator"),
            owner(5, "/crew approve hermes@b role=dev"),
            owner(6, "/crew approve codex@c role=dev"),
            left_marker(20, "hermes@b", &w.kb),
            owner(25, "/crew role claude@a dev"),
            owner(26, "/crew role codex@c coordinator"),
        ],
    );
    let w = W { reg, ..w };
    let assign = by(
        &w,
        &w.ka,
        "claude@a",
        9,
        "hermes@b",
        Kind::Request,
        "Assigned to you: t",
    );
    let early = by(
        &w,
        &w.kb,
        "hermes@b",
        10,
        "coordinator",
        Kind::Ack,
        "Taken.",
    );
    let late = by(&w, &w.kb, "hermes@b", 30, "coordinator", Kind::Report, "x");
    let r = run(&w, &[assign, early, late]);
    // 9 is a coordinator message of claude@a, who is a dev now: no wrong-writer finding;
    // 30 follows the marker of hermes@b
    assert_eq!(kinds(&r), vec![(30, FindingKind::UnknownSender)]);
}

#[test]
fn security_events_know_whether_they_were_reported_to_the_owner() {
    let w = w();
    let wrong = by(
        &w,
        &generate_key().unwrap(),
        "claude@a",
        12,
        "hermes@b",
        Kind::Report,
        "x",
    );
    let key = event_key(&wrong.repo, wrong.issue, 12, "bad-signature");
    let mut rep = HashSet::new();
    let r = audit(
        &w.cfg,
        &w.reg,
        std::slice::from_ref(&wrong),
        &BTreeMap::new(),
        60,
        &rep,
        &BoardView::default(),
    );
    assert_eq!(r.findings[0].reported_to_owner, Some(false));
    rep.insert(key);
    let r = audit(
        &w.cfg,
        &w.reg,
        &[wrong],
        &BTreeMap::new(),
        60,
        &rep,
        &BoardView::default(),
    );
    assert_eq!(r.findings[0].reported_to_owner, Some(true));
}

#[test]
fn reported_events_come_from_info_messages_to_the_owner_only() {
    let sender = |name: &str, role| Sender::Agent {
        name: name.into(),
        role,
    };
    let m = |id: u64, s: Sender, to: &str, kind: Kind, body: &str| VMsg {
        ts: String::new(),
        comment_id: id,
        created: 0,
        sender: s,
        to: to.into(),
        kind: Some(kind),
        re: crate::protocol::names::Re::None,
        body: body.into(),
    };
    let msgs = vec![
        m(
            1,
            sender("c@x", Role::Coordinator),
            "owner",
            Kind::Info,
            "Security event\nevent=a/b#2/c-9:bad-signature\n",
        ),
        // to the coordinator, not to the owner
        m(
            2,
            sender("d@x", Role::Dev),
            "coordinator",
            Kind::Info,
            "event=a/b#2/c-10:edited-message",
        ),
        // an owner comment is not a report
        m(
            3,
            Sender::Owner,
            "owner",
            Kind::Info,
            "event=a/b#2/c-11:bad-signature",
        ),
        // another kind
        m(
            4,
            sender("c@x", Role::Coordinator),
            "owner",
            Kind::Report,
            "event=a/b#2/c-12:bad-signature",
        ),
    ];
    let got = reported_events(&msgs);
    assert_eq!(got.len(), 1);
    assert!(got.contains("a/b#2/c-9:bad-signature"));
}

#[test]
fn a_wrong_writer_is_found_from_signed_messages_and_the_right_one_is_not() {
    let w = w();
    let assign = by(
        &w,
        &w.ka,
        "claude@a",
        10,
        "hermes@b",
        Kind::Request,
        "Assigned to you: t\nticket: x/y#7",
    );
    let ack = by(
        &w,
        &w.kb,
        "hermes@b",
        11,
        "coordinator",
        Kind::Ack,
        "Taken.",
    );
    // codex@c says done on a ticket that is assigned to hermes@b
    let done_c = by(
        &w,
        &w.kc,
        "codex@c",
        12,
        "coordinator",
        Kind::Done,
        "pr: u\nsha: s\nci: none",
    );
    let done_b = by(
        &w,
        &w.kb,
        "hermes@b",
        13,
        "coordinator",
        Kind::Done,
        "pr: u\nsha: s\nci: none",
    );
    let r = run(&w, &[assign, ack, done_c, done_b]);
    assert_eq!(kinds(&r), vec![(12, FindingKind::WrongWriter)]);
    let d = &r.findings[0].detail;
    assert!(
        d.contains("kind=done by codex@c") && d.contains("assigned to hermes@b"),
        "{d}"
    );
}

#[test]
fn coordinator_only_messages_by_a_dev_are_wrong_writers() {
    let w = w();
    let cases = [
        (Kind::Request, "Assigned to you: t", "assigned"),
        (
            Kind::Info,
            "Ticket is on the board.\nitem=PVTI_x",
            "board item",
        ),
        (Kind::Info, "Reassigned. Reason: x", "reassigned"),
        (Kind::Info, "Closed after a passed verify.", "closed"),
        (Kind::Report, "verify: pass\npr: u", "verify"),
    ];
    for (i, (kind, body, word)) in cases.iter().enumerate() {
        let id = 20 + i as u64;
        let by_dev = by(&w, &w.kb, "hermes@b", id, "codex@c", *kind, body);
        let r = run(&w, &[by_dev]);
        assert_eq!(kinds(&r), vec![(id, FindingKind::WrongWriter)], "{word}");
        // control: the same message from the coordinator is fine
        let by_coord = by(&w, &w.ka, "claude@a", id, "codex@c", *kind, body);
        assert!(run(&w, &[by_coord]).findings.is_empty(), "{word}");
    }
}

#[test]
fn done_without_an_assignment_and_after_a_reassign_is_a_wrong_writer() {
    let w = w();
    let lone = by(
        &w,
        &w.kb,
        "hermes@b",
        10,
        "coordinator",
        Kind::Done,
        "pr: u",
    );
    let r = run(&w, &[lone]);
    assert!(
        r.findings[0].detail.contains("not assigned to anybody"),
        "{:?}",
        r.findings
    );
    // assigned, taken, reassigned (by the coordinator), then a late done by the old agent
    let a = by(
        &w,
        &w.ka,
        "claude@a",
        10,
        "hermes@b",
        Kind::Request,
        "Assigned to you: t",
    );
    let k = by(
        &w,
        &w.kb,
        "hermes@b",
        11,
        "coordinator",
        Kind::Ack,
        "Taken.",
    );
    let re = by(
        &w,
        &w.ka,
        "claude@a",
        12,
        "hermes@b",
        Kind::Info,
        "Reassigned. Reason: dead",
    );
    let late = by(
        &w,
        &w.kb,
        "hermes@b",
        13,
        "coordinator",
        Kind::Done,
        "pr: u",
    );
    let r = run(&w, &[a, k, re, late]);
    assert_eq!(kinds(&r), vec![(13, FindingKind::WrongWriter)]);
}

#[test]
fn the_registry_issue_is_not_judged_by_the_ticket_rules() {
    // acks of owner commands and pongs are posted by devs in the registry issue
    let w = w();
    let mut c = signed(
        10,
        "owner-login",
        &w.kb,
        "hermes@b",
        "owner",
        Kind::Ack,
        "applied: pause",
    );
    c.issue = w.cfg.registry_issue;
    c.repo = w.cfg.main_repo().to_string();
    let r = run(&w, &[c]);
    assert!(r.findings.is_empty(), "{:?}", r.findings);
}

#[test]
fn key_words_are_stable() {
    assert_eq!(FindingKind::BadSignature.word(), "bad-signature");
    assert_eq!(FindingKind::Edited.word(), "edited-message");
    assert_eq!(
        event_key("a/b", 2, 9, "bad-signature"),
        "a/b#2/c-9:bad-signature"
    );
    assert!(!FindingKind::WrongWriter.is_security_event());
    assert!(!FindingKind::HeaderlessOwner.is_security_event());
}

fn view(tickets: &[(&str, &str, Option<&str>)], agents: Vec<BoardAgent>) -> BoardView {
    BoardView {
        tickets: tickets
            .iter()
            .map(|(key, status, agent)| BoardTicket {
                key: key.to_string(),
                item_id: "PVTI_t".into(),
                status: Some(status.to_string()),
                agent: agent.map(str::to_string),
            })
            .collect(),
        agents,
    }
}

fn with_board(w: &W, cs: &[Comment], b: &BoardView) -> Vec<String> {
    audit(&w.cfg, &w.reg, cs, &BTreeMap::new(), 60, &HashSet::new(), b)
        .findings
        .into_iter()
        .filter(|f| f.kind == FindingKind::FieldMismatch)
        .map(|f| f.detail)
        .collect()
}

/// A ticket of issue 7: assigned to hermes@b and taken.
fn taken(w: &W) -> (Vec<Comment>, String) {
    let assign = by(
        w,
        &w.ka,
        "claude@a",
        10,
        "hermes@b",
        Kind::Request,
        "Assigned to you: t",
    );
    let ack = by(w, &w.kb, "hermes@b", 11, "coordinator", Kind::Ack, "Taken.");
    let key = format!("{}#7", assign.repo);
    (vec![assign, ack], key)
}

#[test]
fn a_ticket_status_that_the_log_supports_is_no_mismatch_and_another_one_is() {
    let w = w();
    let (cs, key) = taken(&w);
    let ok = view(&[(&key, "In Progress", Some("hermes@b"))], vec![]);
    assert!(with_board(&w, &cs, &ok).is_empty());
    let bad = view(&[(&key, "Done", Some("hermes@b"))], vec![]);
    let got = with_board(&w, &cs, &bad);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains("board Status is Done, the log shows In Progress")
            && got[0].ends_with("an owner edit is not in the log"),
        "{got:?}"
    );
    // an empty or unknown option does not match a known status either
    let empty = BoardView {
        tickets: vec![BoardTicket {
            key: key.clone(),
            item_id: "PVTI_t".into(),
            status: None,
            agent: Some("hermes@b".into()),
        }],
        agents: vec![],
    };
    assert_eq!(with_board(&w, &cs, &empty).len(), 1);
}

#[test]
fn blocked_after_a_question_to_the_owner_is_no_mismatch() {
    let w = w();
    let (mut cs, key) = taken(&w);
    cs.push(by(
        &w,
        &w.kb,
        "hermes@b",
        12,
        "owner",
        Kind::Question,
        "which library?",
    ));
    for st in ["Needs Human", "Blocked", "In Progress"] {
        let b = view(&[(&key, st, Some("hermes@b"))], vec![]);
        assert!(with_board(&w, &cs, &b).is_empty(), "{st}");
    }
    let b = view(&[(&key, "Done", Some("hermes@b"))], vec![]);
    assert_eq!(with_board(&w, &cs, &b).len(), 1);
}

#[test]
fn a_ticket_without_a_status_in_the_log_is_not_judged() {
    let w = w();
    let none = view(&[("x/y#99", "Done", None)], vec![]);
    assert!(with_board(&w, &[], &none).is_empty());
}

#[test]
fn a_ticket_agent_that_the_log_does_not_show_is_a_mismatch() {
    let w = w();
    let (cs, key) = taken(&w);
    let other = view(&[(&key, "In Progress", Some("codex@c"))], vec![]);
    let got = with_board(&w, &cs, &other);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("board Agent is codex@c, the log shows hermes@b"));
    let none = view(&[(&key, "In Progress", None)], vec![]);
    assert_eq!(with_board(&w, &cs, &none).len(), 1);
}

fn agent_row(w: &W, name: &str, role: &str, state: &str) -> BoardAgent {
    let k = match name {
        "claude@a" => &w.ka,
        "hermes@b" => &w.kb,
        _ => &w.kc,
    };
    BoardAgent {
        name: name.into(),
        role: Some(role.into()),
        key: Some(crate::registry::tests::pub_line(k, name)),
        state: Some(state.into()),
    }
}

fn agent_findings(w: &W, a: BoardAgent) -> Vec<String> {
    with_board(w, &[], &view(&[], vec![a]))
}

#[test]
fn a_clean_agent_row_is_no_mismatch() {
    let w = w();
    for (n, r) in [("claude@a", "Coordinator"), ("hermes@b", "Dev")] {
        for st in ["approved", "active", "draining"] {
            assert!(
                agent_findings(&w, agent_row(&w, n, r, st)).is_empty(),
                "{n} {st}"
            );
        }
    }
}

#[test]
fn an_agent_role_that_differs_from_the_span_role_is_a_mismatch() {
    let w = w();
    let got = agent_findings(&w, agent_row(&w, "hermes@b", "Coordinator", "active"));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("board Role is Coordinator, the registry shows dev"));
}

#[test]
fn an_agent_key_that_is_not_the_join_key_is_a_mismatch() {
    let w = w();
    let mut a = agent_row(&w, "hermes@b", "Dev", "active");
    a.key = Some(crate::registry::tests::pub_line(&w.kc, "hermes@b"));
    let got = agent_findings(&w, a.clone());
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("is not the key of the join"));
    a.key = None;
    assert_eq!(agent_findings(&w, a).len(), 1);
}

#[test]
fn a_state_of_a_left_or_rejected_agent_must_not_let_it_speak() {
    let (ka, kb, kc) = (
        generate_key().unwrap(),
        generate_key().unwrap(),
        generate_key().unwrap(),
    );
    let cfg = cfg();
    let reg = resolve(
        &cfg,
        &[
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            join_comment(3, "codex@c", &kc, "PVTI_c"),
            join_comment(4, "ghost@z", &generate_key().unwrap(), "PVTI_z"),
            owner(5, "/crew approve claude@a role=coordinator"),
            owner(6, "/crew approve hermes@b role=dev"),
            owner(7, "/crew approve codex@c role=dev"),
            crate::registry::tests::left_marker(8, "hermes@b", &kb),
            owner(9, "/crew reject codex@c"),
        ],
    );
    let w = W {
        ka,
        kb,
        kc,
        reg,
        cfg,
    };
    // left: only `left` fits
    let got = agent_findings(&w, agent_row(&w, "hermes@b", "Dev", "active"));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("the log shows a left marker"));
    assert!(agent_findings(&w, agent_row(&w, "hermes@b", "Dev", "left")).is_empty());
    // rejected: a speaking state is a mismatch
    let got = agent_findings(&w, agent_row(&w, "codex@c", "Dev", "draining"));
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].contains("the owner rejected the agent"));
    assert!(agent_findings(&w, agent_row(&w, "codex@c", "Dev", "left")).is_empty());
    // a join without an approval does not speak
    let mut g = agent_row(&w, "ghost@z", "Dev", "active");
    g.key = None;
    let got = agent_findings(&w, g);
    assert!(
        got.iter().any(|d| d.contains("no approval of the agent")),
        "{got:?}"
    );
    let mut g = agent_row(&w, "ghost@z", "Dev", "requested");
    g.key = None;
    assert!(!agent_findings(&w, g).iter().any(|d| d.contains("approval")));
}

#[test]
fn the_field_mismatch_is_no_security_event() {
    assert!(!FindingKind::FieldMismatch.is_security_event());
    assert_eq!(FindingKind::FieldMismatch.word(), "field-mismatch");
    assert_eq!(
        serde_json::to_string(&FindingKind::FieldMismatch).unwrap(),
        "\"field_mismatch\""
    );
}
