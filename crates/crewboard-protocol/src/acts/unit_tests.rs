use super::*;

fn assigned(gate: Option<&str>, ci: Option<bool>) -> Act {
    Act::Assignment {
        gate: gate.map(str::to_string),
        ci_none: ci,
    }
}

// One golden test per act. Each text is made by the code that posts it.

#[test]
fn free_text_that_reads_as_an_act_is_reserved_for_its_command() {
    let cases: Vec<(Kind, String, &str)> = vec![
        (
            Kind::Request,
            assignment_body("T", "o/r#1", "x", None, None, ""),
            "assign",
        ),
        (Kind::Info, info_body("PVTI_x", 15), "assign or ticket new"),
        (Kind::Info, reassigned_body("late"), "reassign"),
        (Kind::Info, closed_verified_body(), "close or merge"),
        (
            Kind::Info,
            merged_body("p", "s", "squash", "m", "b", None),
            "close or merge",
        ),
        (
            Kind::Info,
            format!("{OWNER_MERGED_HEAD}\n"),
            "close or merge",
        ),
        (Kind::Info, dropped_body("not needed"), "close or merge"),
        (
            Kind::Info,
            "closed: not needed".to_string(),
            "close or merge",
        ),
        (Kind::Report, format!("{}\nx", verify_head(true)), "verify"),
        (Kind::Report, verify_head(false), "verify"),
        (Kind::Blocked, park_body(7, "r"), "take --interrupt"),
        (Kind::Info, left_body("a@b"), "stop"),
        (
            Kind::Info,
            "late: 1h waiting\nlate-key: k".to_string(),
            "no command",
        ),
    ];
    for (kind, body, cmd) in cases {
        for text in [body.clone(), format!("\n  {body}")] {
            let got = reserved_for(kind, &text);
            assert!(
                got.is_some_and(|c| c.starts_with(cmd)),
                "{kind:?} {text:?}: {got:?}"
            );
        }
    }
    // plain text, also text that only looks alike, is free
    for (kind, text) in [
        (Kind::Info, "Closed the loop with the owner"),
        (Kind::Info, "we closed: nothing"),
        (Kind::Report, "the verify: pass is next"),
        (Kind::Answer, "withdraws a question? no"),
        (Kind::Info, "Reassigned tickets are listed below"),
        (Kind::Request, "please assign this"),
        (Kind::Answer, "ok\nwithdraws: question"),
        (Kind::Question, "closed: or not?"),
        (Kind::Info, "we dropped: nothing"),
        (Kind::Question, "dropped: or not?"),
    ] {
        assert_eq!(reserved_for(kind, text), None, "{text}");
    }
}

#[test]
fn golden_assignment_as_assign_and_reassign_write_it() {
    let body = assignment_body(
        "Fix it",
        "o/r#4",
        "PVTI_x",
        Some("CI green"),
        Some(true),
        "priority: P1\norder: 2\n",
    );
    assert_eq!(
        parse(Some(Kind::Request), &body),
        assigned(Some("CI green"), Some(true))
    );
    let required = assignment_body(
        "Fix it",
        "o/r#4",
        "PVTI_x",
        Some("a | b"),
        Some(false),
        "reason: r\n",
    );
    assert_eq!(
        parse(Some(Kind::Request), &required),
        assigned(Some("a | b"), Some(false))
    );
    // an assignment that an old binary wrote has neither line
    let old = assignment_body("Fix it", "o/r#4", "PVTI_x", None, None, "reason: r\n");
    assert_eq!(parse(Some(Kind::Request), &old), assigned(None, None));
    // an unknown `ci:` value is no signed value
    let odd = format!(
        "{}ci: maybe\n",
        assignment_body("T", "o/r#1", "x", Some("g"), None, "")
    );
    assert_eq!(parse(Some(Kind::Request), &odd), assigned(Some("g"), None));
}

#[test]
fn golden_assignment_needs_the_first_line_and_the_kind() {
    // a `gate:` line alone is no assignment
    assert_eq!(
        parse(Some(Kind::Request), "x\ngate: CI green\n"),
        Act::Other
    );
    let body = assignment_body("T", "o/r#1", "x", Some("g"), Some(true), "");
    assert_eq!(parse(Some(Kind::Info), &body), Act::Other);
    assert_eq!(parse(None, &body), Act::Other);
}

#[test]
fn golden_board_announce_as_the_creation_info_writes_it() {
    let body = info_body("PVTI_x", 15);
    assert_eq!(parse(Some(Kind::Info), &body), Act::BoardAnnounce);
    let adopting = info_body_adopting("PVTI_x", 15, "https://github.com/o/r/pull/9", "b");
    assert_eq!(parse(Some(Kind::Info), &adopting), Act::BoardAnnounce);
}

#[test]
fn golden_reassigned() {
    assert_eq!(
        parse(Some(Kind::Info), &reassigned_body(" stuck \n")),
        Act::Reassigned
    );
}

#[test]
fn a_reassign_notice_carries_the_earlier_work_and_stays_a_reassign() {
    let pr = EarlierWork {
        pr: Some("https://github.com/o/r/pull/9".into()),
        branch: "crew/a@h/5-x".into(),
    };
    let body = reassigned_body_with("dead", Some(&pr));
    assert_eq!(parse(Some(Kind::Info), &body), Act::Reassigned);
    assert_eq!(earlier_work_of(&body), Some(pr));
    let branch = EarlierWork {
        pr: None,
        branch: "crew/a@h/5-x".into(),
    };
    let body = reassigned_body_with("dead", Some(&branch));
    assert_eq!(parse(Some(Kind::Info), &body), Act::Reassigned);
    assert_eq!(earlier_work_of(&body), Some(branch));
    // no work, or a line without its value: nothing
    assert_eq!(earlier_work_of(&reassigned_body_with("dead", None)), None);
    assert_eq!(earlier_work_of("open-pr: https://x branch: "), None);
    assert_eq!(earlier_work_of("open-pr: \nearlier-branch:"), None);
}

#[test]
fn golden_every_close_notice() {
    let close = |body: &str| parse(Some(Kind::Info), body);
    assert_eq!(
        close(&closed_verified_body()),
        Act::Closed(CloseKind::Verified)
    );
    let merged = merged_body(
        "https://github.com/o/r/pull/1",
        "deadbeef",
        "squash",
        "abc123",
        "crew/x",
        None,
    );
    assert_eq!(close(&merged), Act::Closed(CloseKind::Merged));
    let pr = "https://github.com/o/r/pull/9";
    assert_eq!(
        close(&owner_merged_body(pr, Some("abc"), None)),
        Act::Closed(CloseKind::OwnerMerged)
    );
    assert_eq!(
        close(&owner_merged_body(pr, None, Some("why"))),
        Act::Closed(CloseKind::OwnerMerged)
    );
    assert_eq!(
        close(&dropped_body("superseded by o/r#12")),
        Act::Closed(CloseKind::Dropped)
    );
    assert_eq!(dropped_body("not needed"), "dropped: not needed\n");
    // the text of a close by judgment of an older build is still read
    assert_eq!(
        close("closed: superseded by o/r#12\n"),
        Act::Closed(CloseKind::Judgment)
    );
    // a drop in another kind, or by a line that only starts alike, is no close
    assert_eq!(parse(Some(Kind::Report), &dropped_body("x")), Act::Other);
    assert_eq!(close("droppedx: y"), Act::Other);
    assert_eq!(
        close(OWNER_MERGED_HEAD),
        Act::Closed(CloseKind::OwnerMerged)
    );
}

#[test]
fn golden_verify_result_as_verify_writes_it() {
    let ev = EvidenceMsg {
        pr: "https://github.com/o/r/pull/1".into(),
        sha: "abc".into(),
        ci: "success".into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    };
    let pass = verify_body(true, &ev, "abc", &[], "", None, None);
    let fail = verify_body(
        false,
        &ev,
        "def",
        &[VerifyFailure::PrClosed],
        "n",
        Some("c"),
        None,
    );
    assert_eq!(parse(Some(Kind::Report), &pass), Act::Verify { pass: true });
    assert_eq!(
        parse(Some(Kind::Report), &fail),
        Act::Verify { pass: false }
    );
    // a report without a result, and a result in another kind, is no verify
    assert_eq!(
        parse(Some(Kind::Report), "all green\nverify: pass\n"),
        Act::Other
    );
    assert_eq!(parse(Some(Kind::Info), &pass), Act::Other);
}

#[test]
fn golden_withdraws_question() {
    let body = withdraw_body("reassigned: new agent asks again");
    assert_eq!(parse(Some(Kind::Answer), &body), Act::WithdrawsQuestion);
    assert_eq!(parse(Some(Kind::Answer), "a plain answer"), Act::Other);
    assert_eq!(parse(Some(Kind::Info), &body), Act::Other);
}

#[test]
fn golden_parked_for() {
    assert_eq!(
        parse(Some(Kind::Blocked), &park_body(49, "P0 first")),
        Act::Parked { for_issue: 49 }
    );
    assert_eq!(
        parse(Some(Kind::Blocked), "parked for #7: x"),
        Act::Parked { for_issue: 7 }
    );
    for bad in [
        "parked for #: x",
        "parked for #12 x",
        "parked for #x: y",
        "waiting",
        "parked for #99999999999999999999999: z",
    ] {
        assert_eq!(parse(Some(Kind::Blocked), bad), Act::Other, "{bad}");
    }
    assert_eq!(parse(Some(Kind::Info), &park_body(3, "x")), Act::Other);
}

#[test]
fn golden_left_marker() {
    assert_eq!(parse(Some(Kind::Info), &left_body("claude@a")), Act::Left);
    assert_eq!(
        parse(Some(Kind::Report), &left_body("claude@a")),
        Act::Other
    );
    assert_eq!(
        parse(Some(Kind::Info), "I left\nlifecycle: left now"),
        Act::Other
    );
}

#[test]
fn golden_stopping_notice() {
    assert_eq!(
        parse(Some(Kind::Info), &stopping_body("claude@a", "none")),
        Act::Stopping {
            name: "claude@a".into()
        }
    );
    // the text of 0.6.0 reads the same
    assert_eq!(
        parse(
            Some(Kind::Info),
            "claude@a is stopping (draining). Open tickets that the coordinator returns to Backlog: o/r#1 (Ready)"
        ),
        Act::Stopping {
            name: "claude@a".into()
        }
    );
    assert_eq!(
        parse(Some(Kind::Report), &stopping_body("claude@a", "none")),
        Act::Other
    );
    for bad in [
        " is stopping (draining). x",
        "two words is stopping (draining). x",
        "claude@a is stopping (State draining)",
    ] {
        assert_eq!(parse(Some(Kind::Info), bad), Act::Other, "{bad}");
    }
    assert_eq!(
        reserved_for(Kind::Info, &stopping_body("claude@a", "none")),
        Some("stop")
    );
}

#[test]
fn golden_late_report_as_the_late_note_writes_it() {
    let body = format!(
        "{}since: x\nnext: y\n",
        late_head("dead", "c@h", "dead:c@h:100")
    );
    assert_eq!(
        parse(Some(Kind::Info), &body),
        Act::Late {
            key: Some("dead:c@h:100".into())
        }
    );
    assert_eq!(parse(Some(Kind::Info), "no late: here"), Act::Other);
}
