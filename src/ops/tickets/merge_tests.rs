use super::*;
use crate::base::ids;
use crate::rules::evidence::CiSources;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PR_URL: &str = "https://github.com/a/b/pull/7";

fn pr() -> PrFacts {
    PrFacts {
        repo: "a/b".into(),
        number: 7,
        state: "open".into(),
        merged: false,
        head_sha: SHA.into(),
        head_ref: "crew/hermes@b/12-fix-it".into(),
        title: "Work".into(),
        body: "Does it.\n\nCloses #12\n".into(),
    }
}

fn green() -> CiFacts {
    CiFacts::Green {
        run_url: "https://github.com/a/b/actions/runs/1".into(),
        sources: CiSources::default(),
    }
}

fn allowed() -> Authority {
    Authority {
        coordinator_may_merge: true,
        needs_owner: vec![],
        ..Authority::default()
    }
}

fn judge(
    auth: &Authority,
    status: Status,
    verify: Option<bool>,
    pr: &PrFacts,
    ev_sha: &str,
    ci: &CiFacts,
) -> Result<Evidence, Refusal> {
    judge_open(auth, status, None, verify, pr, ev_sha, ci)
}

#[allow(clippy::too_many_arguments)]
fn judge_open(
    auth: &Authority,
    status: Status,
    question_open_since: Option<&str>,
    verify: Option<bool>,
    pr: &PrFacts,
    ev_sha: &str,
    ci: &CiFacts,
) -> Result<Evidence, Refusal> {
    let repos = vec!["a/b".to_string()];
    judge_merge(&MergeInput {
        auth,
        status,
        question_open_since,
        verify,
        pr,
        evidence_sha: ev_sha,
        ci,
        ci_none_declared: false,
        linked_repos: &repos,
        ticket_repo: "a/b",
        ticket_issue: 12,
        agent: "hermes@b",
        no_run: None,
        evidence_local_gate: None,
        coordinator_local_gate: None,
        adopted_branch: None,
        protected: &[],
        files_truncated: false,
        authority_path: "AUTHORITY.md",
    })
}

fn rule(r: Result<Evidence, Refusal>) -> &'static str {
    r.expect_err("must be refused").rule.as_str()
}

fn local_auth(local: bool) -> Authority {
    Authority {
        ci_mode: if local {
            crate::authority::CiMode::Local
        } else {
            crate::authority::CiMode::Ci
        },
        ..allowed()
    }
}

/// The merge judgement of a local-gate evidence: the dev's line, the coordinator's line.
fn judge_local(
    auth: &Authority,
    verify: Option<bool>,
    dev: Option<&str>,
    coordinator: Option<&str>,
    ci: &CiFacts,
) -> Result<Evidence, Refusal> {
    let repos = vec!["a/b".to_string()];
    judge_merge(&MergeInput {
        auth,
        status: Status::Review,
        question_open_since: None,
        verify,
        pr: &pr(),
        evidence_sha: SHA,
        ci,
        ci_none_declared: false,
        linked_repos: &repos,
        ticket_repo: "a/b",
        ticket_issue: 12,
        agent: "hermes@b",
        no_run: None,
        evidence_local_gate: dev,
        coordinator_local_gate: coordinator,
        adopted_branch: None,
        protected: &[],
        files_truncated: false,
        authority_path: "AUTHORITY.md",
    })
}

#[test]
fn a_local_gate_merges_in_local_mode_with_both_lines_whatever_the_ci_says() {
    let dev = Some("host-a: make test rc=0");
    let coord = Some("host-c: make test rc=0");
    for ci in [
        CiFacts::NotGreen(vec!["red".into()]),
        CiFacts::NoCi(vec![]),
        green(),
    ] {
        let e = judge_local(&local_auth(true), Some(true), dev, coord, &ci).unwrap();
        assert_eq!(e.ci, "local-gate");
        assert_eq!(e.local_gate.as_deref(), dev);
    }
}

#[test]
fn a_local_gate_does_not_merge_in_ci_mode_or_without_both_lines_or_without_a_passed_verify() {
    let dev = Some("host-a: make test rc=0");
    let coord = Some("host-c: make test rc=0");
    let ci = CiFacts::NoCi(vec![]);
    // ci mode: the local gate does not count, even with green CI facts
    for c in [&ci, &green()] {
        assert_eq!(
            rule(judge_local(&local_auth(false), Some(true), dev, coord, c)),
            "local-gate-not-enabled"
        );
    }
    // no coordinator line, or a blank one
    for c in [None, Some(" ")] {
        assert_eq!(
            rule(judge_local(&local_auth(true), Some(true), dev, c, &ci)),
            "local-gate-missing"
        );
    }
    // no gate line in the done
    for d in [Some(""), Some("  ")] {
        assert_eq!(
            rule(judge_local(&local_auth(true), Some(true), d, coord, &ci)),
            "local-gate-missing"
        );
    }
    // the usual checks stay
    assert_eq!(
        rule(judge_local(&local_auth(true), None, dev, coord, &ci)),
        "merge-verify-required"
    );
    assert_eq!(
        rule(judge_local(&local_auth(true), Some(false), dev, coord, &ci)),
        "merge-verify-required"
    );
    // control: CI evidence in local mode is judged by CI as before
    assert_eq!(
        rule(judge_local(
            &local_auth(true),
            Some(true),
            None,
            None,
            &CiFacts::NotGreen(vec!["red".into()])
        )),
        "evidence-ci-not-green"
    );
    assert!(judge_local(&local_auth(true), Some(true), None, None, &green()).is_ok());
}

#[test]
fn all_good_passes_and_names_the_verified_sha() {
    let e = judge(&allowed(), Status::Review, Some(true), &pr(), SHA, &green()).unwrap();
    assert_eq!(e.sha, SHA);
}

#[test]
fn authority_that_denies_refuses_first() {
    // default: the flag is false (and `merge` is not in the default advice list)
    let a = Authority::default();
    assert_eq!(
        rule(judge(&a, Status::Review, Some(true), &pr(), SHA, &green())),
        "authority-merge-not-allowed"
    );
    // flag false, `merge` listed in needs_owner: the old entry still refuses
    let b = Authority {
        needs_owner: vec!["merge".into()],
        ..Authority::default()
    };
    assert_eq!(
        rule(judge(&b, Status::Review, Some(true), &pr(), SHA, &green())),
        "authority-needs-owner"
    );
    // flag true but `merge` still listed: the old entry wins
    let c = Authority {
        coordinator_may_merge: true,
        needs_owner: vec!["merge".into()],
        ..Authority::default()
    };
    assert_eq!(
        rule(judge(&c, Status::Review, Some(true), &pr(), SHA, &green())),
        "authority-needs-owner"
    );
    // control: flag true and an advice list without `merge` merges
    assert!(
        judge(
            &Authority {
                coordinator_may_merge: true,
                ..Authority::default()
            },
            Status::Review,
            Some(true),
            &pr(),
            SHA,
            &green()
        )
        .is_ok()
    );
}

fn judge_files(protected: &[String], truncated: bool) -> Result<Evidence, Refusal> {
    let repos = vec!["a/b".to_string()];
    judge_merge(&MergeInput {
        auth: &allowed(),
        status: Status::Review,
        question_open_since: None,
        verify: Some(true),
        pr: &pr(),
        evidence_sha: SHA,
        ci: &green(),
        ci_none_declared: false,
        linked_repos: &repos,
        ticket_repo: "a/b",
        ticket_issue: 12,
        agent: "hermes@b",
        no_run: None,
        evidence_local_gate: None,
        coordinator_local_gate: None,
        adopted_branch: None,
        protected,
        files_truncated: truncated,
        authority_path: "AUTHORITY.md",
    })
}

#[test]
fn a_protected_path_is_refused_whatever_the_authority_says() {
    // the flag is true and `needs_owner` is empty: nothing in the file stops the merge
    let a = allowed();
    assert!(a.coordinator_may_merge && a.needs_owner.is_empty());
    let r = judge_files(&[".github/workflows/ci.yml".to_string()], false).unwrap_err();
    assert_eq!(r.rule, "protected-path-owner-merge");
    assert!(
        r.message.contains(".github/workflows/ci.yml"),
        "{}",
        r.message
    );
    assert!(
        r.message
            .ends_with("crewboard close a/b#12 --why \"<text>\""),
        "{}",
        r.message
    );
}

#[test]
fn a_pr_without_a_protected_path_and_with_a_complete_list_passes() {
    assert!(judge_files(&[], false).is_ok());
    let r = judge_files(&[], true).unwrap_err();
    assert_eq!(r.rule, "protected-path-owner-merge");
    assert!(r.message.contains("more than 1000 files"), "{}", r.message);
}

#[test]
fn the_authority_refusal_comes_before_the_path_refusal() {
    let denied = Authority::default();
    let repos = vec!["a/b".to_string()];
    let r = judge_merge(&MergeInput {
        auth: &denied,
        status: Status::Review,
        question_open_since: None,
        verify: Some(true),
        pr: &pr(),
        evidence_sha: SHA,
        ci: &green(),
        ci_none_declared: false,
        linked_repos: &repos,
        ticket_repo: "a/b",
        ticket_issue: 12,
        agent: "hermes@b",
        no_run: None,
        evidence_local_gate: None,
        coordinator_local_gate: None,
        adopted_branch: None,
        protected: &["AUTHORITY.md".to_string()],
        files_truncated: false,
        authority_path: "AUTHORITY.md",
    })
    .unwrap_err();
    assert_eq!(r.rule, "authority-merge-not-allowed");
}

#[test]
fn a_head_that_moved_is_refused() {
    let other = "fedcba9876543210fedcba9876543210fedcba98";
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &pr(),
            other,
            &green()
        )),
        "evidence-head-moved"
    );
}

#[test]
fn ci_that_is_red_or_missing_is_refused() {
    let red = CiFacts::NotGreen(vec!["build: failure".into()]);
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &pr(),
            SHA,
            &red
        )),
        "evidence-ci-not-green"
    );
    let none = CiFacts::NoCi(vec![]);
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &pr(),
            SHA,
            &none
        )),
        "evidence-ci-missing"
    );
}

#[test]
fn merge_counts_only_push_and_pr_runs_like_done() {
    // only information runs (no counted run): the missing-CI refusal
    let info = CiFacts::OnlyInformation {
        info: vec!["5 (workflow_dispatch, success)".into()],
        unreadable: vec![],
    };
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &pr(),
            SHA,
            &info
        )),
        "evidence-ci-missing"
    );
    // a red counted run with a named green run: red
    let red = CiFacts::NotGreen(vec!["run 1 (ci) concluded failure".into()]);
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &pr(),
            SHA,
            &red
        )),
        "evidence-ci-not-green"
    );
    // a named green dispatch run alone: passes and the evidence names it
    let named = CiFacts::Green {
        run_url: "https://github.com/a/b/actions/runs/5".into(),
        sources: CiSources {
            counted: vec!["5 (workflow_dispatch, named, success)".into()],
            ..CiSources::default()
        },
    };
    let e = judge(&allowed(), Status::Review, Some(true), &pr(), SHA, &named).unwrap();
    assert_eq!(
        e.ci_runs.as_deref(),
        Some("counted: 5 (workflow_dispatch, named, success)")
    );
}

#[test]
fn no_passed_verify_wrong_status_or_a_closed_pr_is_refused() {
    for v in [None, Some(false)] {
        assert_eq!(
            rule(judge(&allowed(), Status::Review, v, &pr(), SHA, &green())),
            "merge-verify-required"
        );
    }
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::InProgress,
            Some(true),
            &pr(),
            SHA,
            &green()
        )),
        "merge-needs-review"
    );
    let mut merged = pr();
    merged.merged = true;
    merged.state = "closed".into();
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &merged,
            SHA,
            &green()
        )),
        "merge-pr-not-open"
    );
    let mut closed = pr();
    closed.state = "closed".into();
    assert_eq!(
        rule(judge(
            &allowed(),
            Status::Review,
            Some(true),
            &closed,
            SHA,
            &green()
        )),
        "merge-pr-not-open"
    );
}

#[test]
fn a_ticket_outside_review_is_not_merged() {
    let a = allowed();
    for s in [Status::Blocked, Status::InProgress, Status::Ready] {
        let r = judge(&a, s, Some(true), &pr(), SHA, &green()).unwrap_err();
        assert_eq!(r.rule, "merge-needs-review", "{s:?}");
        assert!(
            r.message.contains(&format!("this one is {}", s.name())),
            "{}",
            r.message
        );
    }
}

#[test]
fn a_review_ticket_with_an_open_question_is_not_merged() {
    let a = allowed();
    let r = judge_open(
        &a,
        Status::Review,
        Some("2026-10-20 10:00 UTC"),
        Some(true),
        &pr(),
        SHA,
        &green(),
    )
    .unwrap_err();
    assert_eq!(r.rule, "verify-owner-question-open");
    // control: the same ticket without an open question merges
    assert!(judge_open(&a, Status::Review, None, Some(true), &pr(), SHA, &green()).is_ok());
}

#[test]
fn the_merge_call_carries_the_verified_sha() {
    let (path, body) = merge_request("a/b", 7, SHA, "squash");
    assert_eq!(path, "repos/a/b/pulls/7/merge");
    assert_eq!(body["sha"], SHA);
    assert_eq!(body["merge_method"], "squash");
    assert!(parse_method("SQUASH").is_ok());
    assert_eq!(parse_method("ff").unwrap_err().rule, "merge-method-unknown");
}

#[test]
fn only_a_branch_under_the_dev_prefix_of_the_same_repo_is_deleted() {
    let a = allowed();
    let dev = "crew/hermes@b/12-fix-it";
    assert_eq!(
        branch_to_delete(&a, "a/b", Some("a/b"), dev).as_deref(),
        Some("repos/a/b/git/refs/heads/crew/hermes%40b/12-fix-it")
    );
    // a fork, an unknown head repo, a branch outside the prefix and a protected one stay
    assert_eq!(branch_to_delete(&a, "a/b", Some("x/b"), dev), None);
    assert_eq!(branch_to_delete(&a, "a/b", None, dev), None);
    assert_eq!(branch_to_delete(&a, "a/b", Some("a/b"), "feature/x"), None);
    assert_eq!(branch_to_delete(&a, "a/b", Some("a/b"), "main"), None);
}

#[test]
fn a_hostile_head_ref_cannot_make_the_delete_path_leave_refs_heads() {
    let a = allowed();
    for bad in [
        "crew/a@h/1-x/%2e%2e/main",
        "crew/a@h/1-x/%2e%2e/%2e%2e/%2e%2e/main",
        "crew/a@h/1-x/%2E%2E/main",
        "crew/a@h/1-x/../../main",
        "crew/a@h/1-x;id",
        "crew/a@h/1-x?y#z",
    ] {
        // Check what the HTTP client requests: reqwest parses the path with the `url` crate,
        // which folds a dot segment, also as %2e%2e.
        if let Some(path) = branch_to_delete(&a, "a/b", Some("a/b"), bad) {
            let url = reqwest::Url::parse(&format!("https://api.github.com/{path}")).unwrap();
            assert!(
                url.path().starts_with("/repos/a/b/git/refs/heads/crew/")
                    && !url.path().ends_with("/main")
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "{bad}: requests {url}"
            );
        }
        assert_eq!(branch_to_delete(&a, "a/b", Some("a/b"), bad), None, "{bad}");
    }
}

#[test]
fn a_good_head_ref_is_encoded_per_segment_in_the_delete_path() {
    let a = allowed();
    let path = branch_to_delete(&a, "a/b", Some("a/b"), "crew/hermes@b/12-v1.2_x").unwrap();
    assert_eq!(path, "repos/a/b/git/refs/heads/crew/hermes%40b/12-v1.2_x");
    let url = reqwest::Url::parse(&format!("https://api.github.com/{path}")).unwrap();
    assert_eq!(
        url.path(),
        "/repos/a/b/git/refs/heads/crew/hermes%40b/12-v1.2_x"
    );
}

#[test]
fn mergeable_state_mapping_both_directions() {
    for ok in ["CLEAN", "clean", "HAS_HOOKS", "UNSTABLE", "BLOCKED"] {
        assert_eq!(judge_mergeable(ok), Mergeable::Go, "{ok}");
    }
    assert_eq!(
        judge_mergeable("DIRTY"),
        Mergeable::Refuse {
            state: "DIRTY".into(),
            conflict: true
        }
    );
    assert_eq!(
        judge_mergeable("behind"),
        Mergeable::Refuse {
            state: "BEHIND".into(),
            conflict: false
        }
    );
    assert_eq!(
        judge_mergeable("DRAFT"),
        Mergeable::Refuse {
            state: "DRAFT".into(),
            conflict: false
        }
    );
    assert_eq!(judge_mergeable("UNKNOWN"), Mergeable::Unknown);
    assert_eq!(judge_mergeable(""), Mergeable::Unknown);
    assert_eq!(
        judge_mergeable("weird"),
        Mergeable::Refuse {
            state: "WEIRD".into(),
            conflict: false
        }
    );
}

#[test]
fn unknown_is_read_again_then_refused() {
    // still unknown after all reads: refused, and the wait ran between the reads only
    let mut reads = 0;
    let mut waits = 0;
    let r = resolve_mergeable::<()>(
        4,
        || waits += 1,
        || {
            reads += 1;
            Ok("UNKNOWN".to_string())
        },
    )
    .unwrap();
    let e = r.unwrap_err();
    assert_eq!(e.rule, "pr-not-mergeable");
    assert!(e.message.contains("state unknown, try again"));
    assert_eq!((reads, waits), (4, 3));
    // unknown, then known: the later answer counts
    let mut seq = ["UNKNOWN", "CLEAN"].into_iter();
    let r = resolve_mergeable::<()>(4, || {}, || Ok(seq.next().unwrap().to_string())).unwrap();
    assert_eq!(r, Ok(Mergeable::Go));
    let mut seq = ["", "DIRTY"].into_iter();
    let r = resolve_mergeable::<()>(4, || {}, || Ok(seq.next().unwrap().to_string()))
        .unwrap()
        .unwrap();
    assert!(matches!(r, Mergeable::Refuse { conflict: true, .. }));
}

#[test]
fn the_refusal_names_the_state_and_the_rule() {
    let e = not_mergeable("5", PR_URL, "BEHIND");
    assert_eq!(e.rule, "pr-not-mergeable");
    assert!(e.message.contains("BEHIND"));
    // The refusal names the re-delivery command of the dev agent
    assert!(
        e.message
            .contains(&format!("crewboard done 5 --pr {PR_URL} --sha <new head>")),
        "{}",
        e.message
    );
    let d = not_mergeable("5", PR_URL, "DRAFT");
    assert_eq!(d.rule, "pr-not-mergeable");
    assert!(
        d.message
            .contains("the PR is a draft; mark it ready for review")
    );
}

#[test]
fn the_rebase_request_names_the_files_and_the_base() {
    let files = vec!["src/a.rs".to_string(), "docs/b.md".to_string()];
    let b = rebase_request_body("a/b#5", PR_URL, "main", &files, true);
    assert!(b.contains("rebase onto main"));
    // The request names the re-delivery command, with the ticket argument
    assert!(
        b.contains(&format!(
            "crewboard done a/b#5 --pr {PR_URL} --sha <new head>"
        )),
        "{b}"
    );
    assert!(b.contains("- src/a.rs"));
    assert!(b.contains("- docs/b.md"));
    assert!(b.contains("files the PR changes"));
    assert!(!b.to_ascii_lowercase().contains("merged"));
    let c = rebase_request_body("5", PR_URL, "main", &files, false);
    assert!(c.contains("conflicting files:"));
    let none = rebase_request_body("5", PR_URL, "main", &[], true);
    assert!(none.contains("could not be listed"));
}

#[test]
fn githubs_own_refusal_maps_to_pr_not_mergeable() {
    let r = |s, m: &str| classify_merge_refusal(s, m, "a/b#7", SHA).unwrap();
    assert_eq!(
        r(405, "Pull Request is not mergeable").rule,
        "pr-not-mergeable"
    );
    assert_eq!(
        r(409, "Pull Request is not mergeable").rule,
        "pr-not-mergeable"
    );
    assert_eq!(
        r(
            405,
            "Base branch was modified. Review and try the merge again."
        )
        .rule,
        "pr-not-mergeable"
    );
    // the other refusals keep their rule
    assert_eq!(
        r(
            409,
            "Head branch was modified. Review and try the merge again."
        )
        .rule,
        "merge-head-moved"
    );
    assert_eq!(
        r(405, "Merge commits are not allowed on this repository.").rule,
        "merge-not-allowed"
    );
    assert_eq!(r(422, "Validation Failed").rule, "merge-not-allowed");
    assert!(classify_merge_refusal(500, "x", "a/b#7", SHA).is_none());
}

// ---- plan tests: the decisions of `merge` without a fake GitHub ----

fn evidence(ci: &str) -> EvidenceMsg {
    EvidenceMsg {
        pr: PR_URL.into(),
        sha: SHA.into(),
        ci: ci.into(),
        ci_sources: None,
        ci_runs: None,
        local_gate: None,
    }
}

fn evidence_rule(
    headless: bool,
    ev: &EvidenceMsg,
    ev_local: bool,
    ci_mode_local: bool,
) -> Option<&'static str> {
    check_evidence_for_merge(headless, ev, ev_local, ci_mode_local)
        .err()
        .map(|r| r.rule.as_str())
}

#[test]
fn a_headless_coordinator_leaves_evidence_without_a_ci_run_or_with_a_named_run() {
    let rule = ids::HEADLESS_SCOPE.as_str();
    for ci in ["none", "none (all files paths-ignored: docs/**)"] {
        assert_eq!(evidence_rule(true, &evidence(ci), false, false), Some(rule));
        // the interactive coordinator merges it
        assert_eq!(evidence_rule(false, &evidence(ci), false, false), None);
    }
    let named = EvidenceMsg {
        ci_runs: Some("counted: 9002 (push, named, success)".into()),
        ..evidence("https://github.com/a/b/actions/runs/9002")
    };
    assert_eq!(evidence_rule(true, &named, false, false), Some(rule));
    assert_eq!(evidence_rule(false, &named, false, false), None);
    let plain = evidence("https://github.com/a/b/actions/runs/9002");
    assert_eq!(evidence_rule(true, &plain, false, false), None);
}

#[test]
fn a_local_gate_is_for_the_interactive_coordinator_and_for_ci_mode_local_only() {
    let ev = EvidenceMsg {
        local_gate: Some("host-a: make test rc=0".into()),
        ..evidence("local-gate")
    };
    // headless: the headless rule, whatever the mode says
    assert_eq!(
        evidence_rule(true, &ev, true, true),
        Some(ids::HEADLESS_SCOPE.as_str())
    );
    // interactive: in ci mode the gate does not count any more
    assert_eq!(
        evidence_rule(false, &ev, true, false),
        Some("local-gate-not-enabled")
    );
    assert_eq!(evidence_rule(false, &ev, true, true), None);
    // evidence with CI is not touched by the mode
    assert_eq!(evidence_rule(false, &evidence("none"), false, false), None);
}

#[test]
fn the_message_of_a_headless_no_run_refusal_names_the_evidence_ci() {
    let r = check_evidence_for_merge(true, &evidence("none"), false, false).unwrap_err();
    assert!(
        r.message.contains("the evidence is 'ci: none'"),
        "{}",
        r.message
    );
    assert!(
        r.message
            .contains("Leave the merge for the interactive coordinator")
    );
}

#[test]
fn a_headless_coordinator_merges_only_on_green_ci() {
    let rule = |ci: &CiFacts| check_headless_ci(true, ci).err().map(|r| r.rule);
    assert_eq!(rule(&green()), None);
    for ci in [
        CiFacts::NoCi(vec![]),
        CiFacts::NotGreen(vec!["red".into()]),
        CiFacts::NotQueried("HTTP 500".into()),
    ] {
        assert_eq!(rule(&ci), Some(ids::HEADLESS_SCOPE));
        // the interactive coordinator goes on to the evidence judgement
        assert!(check_headless_ci(false, &ci).is_ok());
    }
}

fn step_facts<'a>(
    auth: &'a Authority,
    ev: &'a EvidenceMsg,
    head_ref: &'a str,
    head_repo: Option<&'a str>,
) -> StepFacts<'a> {
    StepFacts {
        auth,
        ticket_key: "a/b#12".into(),
        assignee: "hermes@b",
        method: "squash",
        ev,
        ev_local: false,
        coordinator_gate: None,
        pr_repo: "a/b",
        pr_number: 7,
        head_ref,
        head_repo,
        labelled_needs_owner: false,
        question_open: false,
    }
}

#[test]
fn the_steps_after_a_merge_delete_only_a_dev_branch_of_the_same_repo() {
    let auth = allowed();
    let ev = evidence("https://github.com/a/b/actions/runs/1");
    let s = merge_steps(&step_facts(
        &auth,
        &ev,
        "crew/hermes@b/12-fix-it",
        Some("a/b"),
    ));
    assert_eq!(
        s.branch,
        BranchStep::Delete("repos/a/b/git/refs/heads/crew/hermes%40b/12-fix-it".into())
    );
    // a fork branch, or a branch outside the dev prefix, stays and the line says why
    for (head_ref, head_repo) in [
        ("crew/hermes@b/12-fix-it", Some("fork/b")),
        ("crew/hermes@b/12-fix-it", None),
        ("feature/x", Some("a/b")),
    ] {
        let s = merge_steps(&step_facts(&auth, &ev, head_ref, head_repo));
        assert_eq!(
            s.branch,
            BranchStep::Keep(format!(
                "branch {head_ref} not deleted (not under crew/ in a/b, or protected)\n"
            ))
        );
    }
}

#[test]
fn the_notice_names_the_merge_commit_and_a_local_gate_adds_its_lines() {
    let auth = allowed();
    let ev = evidence("https://github.com/a/b/actions/runs/1");
    let s = merge_steps(&step_facts(
        &auth,
        &ev,
        "crew/hermes@b/12-fix-it",
        Some("a/b"),
    ));
    assert_eq!(
        s.notice_body("abc123"),
        format!(
            "Merged by the coordinator.\npr: {PR_URL}\nsha: {SHA}\nmethod: squash\nmerge-commit: abc123\nbranch: crew/hermes@b/12-fix-it"
        )
    );
    assert_eq!((s.assignee.as_str(), s.method), ("hermes@b", "squash"));
    let lg = EvidenceMsg {
        local_gate: Some("host-a: make test rc=0".into()),
        ..evidence("local-gate")
    };
    let mut f = step_facts(&auth, &lg, "crew/hermes@b/12-fix-it", Some("a/b"));
    f.ev_local = true;
    f.coordinator_gate = Some("host-c: make test rc=0");
    let body = merge_steps(&f).notice_body("abc123");
    assert!(body.ends_with(
        "branch: crew/hermes@b/12-fix-it\nci-mode: local\nlocal-gate: host-a: make test rc=0\ncoordinator-local-gate: host-c: make test rc=0"
    ), "{body}");
}

#[test]
fn the_needs_owner_label_goes_with_the_close_only_when_no_question_is_open() {
    let auth = allowed();
    let ev = evidence("https://github.com/a/b/actions/runs/1");
    for (labelled, open, goes) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
        (false, true, false),
    ] {
        let mut f = step_facts(&auth, &ev, "crew/hermes@b/12-fix-it", Some("a/b"));
        f.labelled_needs_owner = labelled;
        f.question_open = open;
        assert_eq!(merge_steps(&f).clear_needs_owner, goes, "{labelled} {open}");
    }
}
