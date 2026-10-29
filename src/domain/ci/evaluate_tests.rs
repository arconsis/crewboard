use super::*;
use serde_json::json;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn fx(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/ci/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn pending_and_failed_reasons_are_told_apart() {
    let r = |runs: &str| reasons(eval(runs, "checks-empty.json", None));
    let running = r("runs-in-progress.json");
    assert!(not_green_is_pending(&running), "{running:?}");
    let failed = r("runs-failure.json");
    assert!(!not_green_is_pending(&failed), "{failed:?}");
    let mixed: Vec<String> = running.iter().chain(failed.iter()).cloned().collect();
    assert!(not_green_is_pending(&mixed));
    assert!(!not_green_is_pending(&[]));
}

fn eval(runs: &str, checks: &str, required: Option<&[String]>) -> CiFacts {
    evaluate_ci(SHA, &fx(runs), &fx(checks), required).unwrap()
}

fn reasons(f: CiFacts) -> Vec<String> {
    match f {
        CiFacts::NotGreen(r) => r,
        other => panic!("expected NotGreen, got {other:?}"),
    }
}

#[test]
fn an_empty_listing_recorded_from_the_sandbox_is_no_ci() {
    assert_eq!(
        eval("runs-empty.json", "checks-empty.json", None),
        CiFacts::NoCi(vec![])
    );
}

#[test]
fn a_completed_successful_run_is_green_and_names_its_url() {
    match eval("runs-success.json", "checks-empty.json", None) {
        CiFacts::Green { run_url, .. } => assert_eq!(
            run_url,
            "https://github.com/example-org/app/actions/runs/9002"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failed_run_is_not_green_and_the_reason_names_the_run() {
    let r = reasons(eval("runs-failure.json", "checks-empty.json", None));
    assert!(
        r.iter()
            .any(|x| x.contains("run 9003") && x.contains("failure")),
        "{r:?}"
    );
}

#[test]
fn a_run_that_is_still_in_progress_is_not_green() {
    let r = reasons(eval("runs-in-progress.json", "checks-empty.json", None));
    assert!(r.iter().any(|x| x.contains("in_progress")), "{r:?}");
}

#[test]
fn a_green_run_of_another_commit_does_not_count() {
    assert_eq!(
        eval("runs-other-sha.json", "checks-empty.json", None),
        CiFacts::NoCi(vec![])
    );
    // control: the same run is accepted when it is the commit asked for
    let v = evaluate_ci(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &fx("runs-other-sha.json"),
        &fx("checks-empty.json"),
        None,
    )
    .unwrap();
    assert!(matches!(v, CiFacts::Green { .. }), "{v:?}");
}

#[test]
fn judgement_is_by_conclusion_never_by_name() {
    let runs = json!({"workflow_runs": []});
    let named_fail_but_success = json!({"check_runs": [
        {"id": 1, "name": "fail", "head_sha": SHA, "status": "completed", "conclusion": "success", "html_url": "https://x/1"}
    ]});
    assert!(matches!(
        evaluate_ci(SHA, &runs, &named_fail_but_success, None).unwrap(),
        CiFacts::Green { .. }
    ));
    let named_ok_but_failure = json!({"check_runs": [
        {"id": 2, "name": "ok", "head_sha": SHA, "status": "completed", "conclusion": "failure", "html_url": "https://x/2"}
    ]});
    assert!(matches!(
        evaluate_ci(SHA, &runs, &named_ok_but_failure, None).unwrap(),
        CiFacts::NotGreen(_)
    ));
    for c in ["skipped", "neutral", "cancelled", "timed_out"] {
        let b = json!({"check_runs": [
            {"id": 3, "name": "x", "head_sha": SHA, "status": "completed", "conclusion": c, "html_url": "https://x/3"}
        ]});
        assert!(
            matches!(
                evaluate_ci(SHA, &runs, &b, None).unwrap(),
                CiFacts::NotGreen(_)
            ),
            "{c}"
        );
    }
}

#[test]
fn required_checks_must_all_have_run_and_succeeded() {
    let req = vec!["build".to_string(), "test".to_string()];
    let ok = eval("runs-empty.json", "checks-required-ok.json", Some(&req));
    assert!(matches!(ok, CiFacts::Green { .. }), "{ok:?}");
    let more = vec!["build".to_string(), "test".to_string(), "lint".to_string()];
    let r = reasons(eval(
        "runs-empty.json",
        "checks-required-ok.json",
        Some(&more),
    ));
    assert!(r.iter().any(|x| x.contains("'lint' has no run")), "{r:?}");
    let r = reasons(eval(
        "runs-empty.json",
        "checks-required-red.json",
        Some(&req),
    ));
    assert!(
        r.iter()
            .any(|x| x.contains("test") && x.contains("failure")),
        "{r:?}"
    );
    let r = reasons(eval("runs-empty.json", "checks-empty.json", Some(&req)));
    assert_eq!(r.len(), 2, "{r:?}");
}

fn check(id: u64, name: &str, status: &str, conclusion: &str) -> Value {
    json!({"id": id, "name": name, "head_sha": SHA, "status": status,
           "conclusion": conclusion, "html_url": format!("https://x/{id}")})
}

fn eval_checks(list: Vec<Value>, required: Option<&[String]>) -> CiFacts {
    evaluate_ci(
        SHA,
        &json!({"workflow_runs": []}),
        &json!({ "check_runs": list }),
        required,
    )
    .unwrap()
}

// ---- spec/ci#jobs-that-ci_ignore-skips: `ci_ignore` of AUTHORITY.md ----

fn suite_check(id: u64, suite: u64, name: &str, conclusion: &str) -> Value {
    let mut c = check(id, name, "completed", conclusion);
    c["check_suite"] = json!({ "id": suite });
    c
}

fn eval_ignoring(
    runs: Vec<Value>,
    checks: Vec<Value>,
    statuses: Option<(Value, Value)>,
    ignore: &[&str],
) -> CiFacts {
    let ignore: Vec<String> = ignore.iter().map(|s| s.to_string()).collect();
    let (r, c) = (
        json!({ "workflow_runs": runs }),
        json!({ "check_runs": checks }),
    );
    evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: Some(&r),
            checks: Some(&c),
            statuses: statuses.as_ref().map(|(a, b)| (a, b)),
            named: &[],
            ignore: &ignore,
        },
        &[],
        None,
    )
    .unwrap()
}

fn push_run(id: u64, conclusion: &str) -> Value {
    let mut r = run(id, "push", conclusion, SHA);
    r["status"] = json!("completed");
    r
}

#[test]
fn a_red_listed_check_does_not_block_and_is_named() {
    let f = eval_ignoring(
        vec![],
        vec![
            check(1, "build", "completed", "success"),
            check(2, "deploy-preview", "completed", "failure"),
        ],
        None,
        &["deploy-preview"],
    );
    match f {
        CiFacts::Green { sources, .. } => {
            assert_eq!(sources.config_ignored, vec!["deploy-preview = failure"]);
            assert_eq!(
                sources.runs_text().unwrap(),
                "ignored (AUTHORITY.md ci_ignore): deploy-preview = failure"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_red_unlisted_check_still_blocks_and_nothing_is_ignored_by_default() {
    let checks = vec![
        check(1, "build", "completed", "success"),
        check(2, "deploy-preview", "completed", "failure"),
    ];
    let f = eval_ignoring(vec![], checks.clone(), None, &["deploy-docs"]);
    assert!(reasons(f)[0].contains("(deploy-preview) concluded failure"));
    let f = eval_ignoring(vec![], checks, None, &[]);
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn a_prefix_entry_matches_by_prefix_and_an_exact_entry_does_not() {
    let checks = vec![
        check(1, "build", "completed", "success"),
        check(2, "deploy-preview", "completed", "failure"),
    ];
    let f = eval_ignoring(vec![], checks.clone(), None, &["deploy-*"]);
    assert!(matches!(f, CiFacts::Green { .. }), "{f:?}");
    // an exact entry is no prefix
    let f = eval_ignoring(vec![], checks, None, &["deploy"]);
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn a_run_is_judged_by_its_other_jobs_when_a_job_of_its_suite_is_listed() {
    let checks = vec![
        suite_check(1, 70, "build", "success"),
        suite_check(2, 70, "deploy-preview", "failure"),
    ];
    let f = eval_ignoring(
        vec![push_run(7, "failure")],
        checks.clone(),
        None,
        &["deploy-preview"],
    );
    assert!(matches!(f, CiFacts::Green { .. }), "{f:?}");
    // the same red run without the list is red
    let f = eval_ignoring(vec![push_run(7, "failure")], checks, None, &[]);
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
    // a red run of another suite stays red
    let f = eval_ignoring(
        vec![push_run(7, "failure"), push_run(8, "failure")],
        vec![
            suite_check(1, 70, "build", "success"),
            suite_check(2, 70, "deploy-preview", "failure"),
            suite_check(3, 80, "lint", "failure"),
        ],
        None,
        &["deploy-preview"],
    );
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn when_every_job_is_ignored_the_ci_is_not_green() {
    let f = eval_ignoring(
        vec![push_run(7, "failure")],
        vec![suite_check(2, 70, "deploy-preview", "failure")],
        None,
        &["deploy-preview"],
    );
    assert!(reasons(f)[0].contains("every CI job is ignored"));
    // also when the ignored job is green
    let f = eval_ignoring(
        vec![],
        vec![check(1, "deploy-preview", "completed", "success")],
        None,
        &["deploy-*"],
    );
    assert!(reasons(f)[0].contains("at least one counted job must be green"));
}

#[test]
fn a_listed_status_context_is_not_judged_and_the_combined_state_is_not_either() {
    let statuses = Some((
        json!({"sha": SHA, "total_count": 1, "state": "failure"}),
        json!([{"id": 1, "context": "ext/deploy", "state": "failure"}]),
    ));
    let checks = vec![check(1, "build", "completed", "success")];
    let f = eval_ignoring(vec![], checks.clone(), statuses.clone(), &["ext/deploy"]);
    match f {
        CiFacts::Green { sources, .. } => {
            assert_eq!(sources.config_ignored, vec!["ext/deploy = failure"])
        }
        other => panic!("{other:?}"),
    }
    let f = eval_ignoring(vec![], checks, statuses, &[]);
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn a_list_without_a_hit_leaves_the_evidence_text_as_before() {
    let f = eval_ignoring(
        vec![],
        vec![check(1, "build", "completed", "success")],
        None,
        &["deploy-*"],
    );
    match f {
        CiFacts::Green { sources, .. } => {
            assert!(sources.config_ignored.is_empty());
            assert_eq!(sources.runs_text(), None);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_skipped_check_next_to_a_success_is_ignored_and_named() {
    let f = eval_checks(
        vec![
            check(1, "test", "completed", "success"),
            check(2, "desktop-app", "completed", "skipped"),
        ],
        None,
    );
    match f {
        CiFacts::Green { sources, .. } => {
            assert_eq!(sources.ignored, vec!["desktop-app (skipped)"]);
            assert_eq!(
                sources.runs_text().unwrap(),
                "ignored: desktop-app (skipped)"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_neutral_check_next_to_a_success_is_ignored() {
    let f = eval_checks(
        vec![
            check(1, "test", "completed", "success"),
            check(2, "y", "completed", "neutral"),
        ],
        None,
    );
    assert!(matches!(f, CiFacts::Green { .. }), "{f:?}");
}

#[test]
fn nothing_ignored_leaves_the_evidence_text_as_before() {
    let f = eval_checks(vec![check(1, "test", "completed", "success")], None);
    match f {
        CiFacts::Green { sources, .. } => {
            assert!(sources.ignored.is_empty());
            assert_eq!(sources.runs_text(), None);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_cancelled_or_failed_check_next_to_a_success_stays_not_green() {
    for c in [
        "cancelled",
        "timed_out",
        "action_required",
        "failure",
        "stale",
    ] {
        let f = eval_checks(
            vec![
                check(1, "test", "completed", "success"),
                check(2, "e2e", "completed", c),
                check(3, "desktop-app", "completed", "skipped"),
            ],
            None,
        );
        let r = reasons(f);
        assert!(
            r.iter().any(|x| x.contains("e2e") && x.contains(c)),
            "{c}: {r:?}"
        );
    }
}

#[test]
fn a_pending_check_next_to_a_success_is_not_green() {
    for st in ["in_progress", "queued"] {
        let f = eval_checks(
            vec![
                check(1, "test", "completed", "success"),
                json!({"id": 2, "name": "x", "head_sha": SHA, "status": st,
                       "conclusion": null, "html_url": "https://x/2"}),
            ],
            None,
        );
        let r = reasons(f);
        assert!(r.iter().any(|x| x.contains(st)), "{st}: {r:?}");
    }
}

#[test]
fn only_skipped_and_neutral_checks_stay_not_green() {
    let f = eval_checks(
        vec![
            check(1, "desktop-app", "completed", "skipped"),
            check(2, "y", "completed", "neutral"),
        ],
        None,
    );
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn with_required_contexts_a_skipped_required_check_is_still_not_green() {
    let req = vec!["desktop-app".to_string()];
    let f = eval_checks(
        vec![
            check(1, "test", "completed", "success"),
            check(2, "desktop-app", "completed", "skipped"),
        ],
        Some(&req),
    );
    let r = reasons(f);
    assert!(
        r.iter()
            .any(|x| x.contains("desktop-app") && x.contains("skipped")),
        "{r:?}"
    );
}

#[test]
fn a_rerun_check_is_judged_by_its_latest_run() {
    let req = vec!["test".to_string()];
    let runs = json!({"workflow_runs": []});
    let b = json!({"check_runs": [
        {"id": 11, "name": "test", "head_sha": SHA, "status": "completed", "conclusion": "failure", "html_url": "https://x/11"},
        {"id": 12, "name": "test", "head_sha": SHA, "status": "completed", "conclusion": "success", "html_url": "https://x/12"}
    ]});
    assert!(matches!(
        evaluate_ci(SHA, &runs, &b, Some(&req)).unwrap(),
        CiFacts::Green { .. }
    ));
    let b2 = json!({"check_runs": [
        {"id": 11, "name": "test", "head_sha": SHA, "status": "completed", "conclusion": "success", "html_url": "https://x/11"},
        {"id": 12, "name": "test", "head_sha": SHA, "status": "completed", "conclusion": "failure", "html_url": "https://x/12"}
    ]});
    assert!(matches!(
        evaluate_ci(SHA, &runs, &b2, Some(&req)).unwrap(),
        CiFacts::NotGreen(_)
    ));
}

#[test]
fn a_failed_workflow_run_makes_it_red_even_when_the_checks_are_green() {
    let v = evaluate_ci(
        SHA,
        &fx("runs-failure.json"),
        &fx("checks-required-ok.json"),
        None,
    )
    .unwrap();
    assert!(matches!(v, CiFacts::NotGreen(_)), "{v:?}");
}

#[test]
fn answers_of_the_wrong_shape_are_errors_not_an_empty_ci() {
    let ok = json!({"workflow_runs": []});
    let checks = json!({"check_runs": []});
    for bad in [json!({"message": "Not Found"}), json!([]), Value::Null] {
        assert!(evaluate_ci(SHA, &bad, &checks, None).is_err(), "{bad}");
        assert!(evaluate_ci(SHA, &ok, &bad, None).is_err(), "{bad}");
    }
    assert_eq!(
        evaluate_ci(SHA, &ok, &checks, None).unwrap(),
        CiFacts::NoCi(vec![])
    );
}

#[test]
fn required_contexts_read_both_shapes_and_refuse_an_unknown_one() {
    assert_eq!(
        required_contexts(&fx("required-checks.json")).unwrap(),
        vec!["build".to_string(), "test".to_string()]
    );
    assert!(required_contexts(&json!({"message": "Branch not protected"})).is_err());
    assert_eq!(
        required_contexts(&json!({"contexts": [], "checks": []})).unwrap(),
        Vec::<String>::new()
    );
}

// ---- commit statuses --------------------------------------------------------

fn stat(name: &str) -> (Value, Value) {
    let mut combined = fx(&format!("status-{name}.json"));
    // The live recording is of another commit than the SHA of these tests: point it at ours
    // (the guard against an answer for another commit is tested on its own below).
    combined["sha"] = json!(SHA);
    (combined, fx(&format!("statuses-{name}.json")))
}

#[test]
fn a_combined_status_for_another_commit_is_a_shape_error() {
    let c = fx("status-empty.json");
    assert_ne!(c["sha"], SHA);
    let l = fx("statuses-empty.json");
    let e = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: None,
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap_err();
    assert!(e.to_string().contains("not for"), "{e}");
}

/// Only the statuses source can be read; the two others are unreadable.
fn only_statuses(name: &str) -> CiFacts {
    let (c, l) = stat(name);
    evaluate_sources(
        "example-org/app",
        SHA,
        &CiBodies {
            runs: None,
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[
            (SRC_RUNS, "HTTP 403, token lacks actions=read".into()),
            (SRC_CHECKS, "HTTP 403, token lacks checks=read".into()),
        ],
        None,
    )
    .unwrap()
}

#[test]
fn green_commit_statuses_are_accepted_as_ci_and_the_sources_are_recorded() {
    match only_statuses("success") {
        CiFacts::Green { run_url, sources } => {
            assert!(
                run_url.starts_with("https://ci.example.org/run/"),
                "{run_url}"
            );
            assert_eq!(sources.seen, vec!["statuses".to_string()]);
            assert_eq!(
                sources.text(),
                "statuses; unreadable: workflow-runs, check-runs"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failing_status_is_refused_with_its_context_named() {
    let r = reasons(only_statuses("failure"));
    assert!(
        r.iter()
            .any(|x| x.contains("ci/test") && x.contains("failure")),
        "{r:?}"
    );
    // the green one of the same commit is not what is reported
    assert!(!r.iter().any(|x| x.contains("ci/build")), "{r:?}");
}

#[test]
fn an_error_status_is_red() {
    let r = reasons(only_statuses("error"));
    assert!(
        r.iter()
            .any(|x| x.contains("ci/test") && x.contains("error")),
        "{r:?}"
    );
}

#[test]
fn a_pending_status_is_refused_as_not_completed() {
    let r = reasons(only_statuses("pending"));
    assert!(
        r.iter()
            .any(|x| x.contains("ci/test") && x.contains("pending")),
        "{r:?}"
    );
}

#[test]
fn a_commit_without_any_status_is_no_ci_although_github_says_pending() {
    // recorded live: state "pending" and total_count 0
    let (c, l) = stat("empty");
    assert_eq!(c["state"], "pending");
    assert_eq!(c["total_count"], 0);
    assert_eq!(
        only_statuses("empty"),
        CiFacts::NoCi(vec!["workflow-runs".into(), "check-runs".into()])
    );
    // control: with one status, "pending" is not the empty case
    assert!(matches!(only_statuses("pending"), CiFacts::NotGreen(_)));
    let _ = l;
}

#[test]
fn the_latest_status_of_a_context_wins_whatever_the_order() {
    let c = json!({"state": "success", "sha": SHA, "total_count": 2, "statuses": []});
    let rerun_green = json!([
        {"id": 5, "context": "ci/x", "state": "success", "target_url": "https://x/5"},
        {"id": 3, "context": "ci/x", "state": "failure", "target_url": "https://x/3"}
    ]);
    let rerun_red = json!([
        {"id": 3, "context": "ci/x", "state": "success", "target_url": "https://x/3"},
        {"id": 5, "context": "ci/x", "state": "failure", "target_url": "https://x/5"}
    ]);
    let ev = |l: &Value, state: &str| {
        let mut c = c.clone();
        c["state"] = json!(state);
        evaluate_sources(
            "o/r",
            SHA,
            &CiBodies {
                runs: None,
                checks: None,
                statuses: Some((&c, l)),
                named: &[],
                ignore: &[],
            },
            &[],
            None,
        )
        .unwrap()
    };
    assert!(matches!(ev(&rerun_green, "success"), CiFacts::Green { .. }));
    assert!(matches!(ev(&rerun_red, "failure"), CiFacts::NotGreen(_)));
}

#[test]
fn the_combined_state_must_agree_with_the_list() {
    // the list shows only a success (it is capped at 100 entries) but the combined state
    // is failure: red wins
    let c = json!({"state": "failure", "sha": SHA, "total_count": 120, "statuses": []});
    let l = json!([{"id": 5, "context": "ci/x", "state": "success"}]);
    let r = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: None,
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap();
    let r = reasons(r);
    assert!(
        r.iter()
            .any(|x| x.contains("combined commit status is failure")),
        "{r:?}"
    );
}

#[test]
fn statuses_without_a_target_url_name_the_commit_page_as_the_ci_url() {
    let c = json!({"state": "success", "sha": SHA, "total_count": 1, "statuses": []});
    let l = json!([{"id": 5, "context": "ci/x", "state": "success", "target_url": null}]);
    let f = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: None,
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap();
    match f {
        CiFacts::Green { run_url, .. } => {
            assert_eq!(run_url, format!("https://github.com/o/r/commit/{SHA}"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_required_context_can_be_met_by_a_commit_status() {
    let (c, l) = stat("success");
    let req = vec!["ci/build".to_string(), "ci/test".to_string()];
    let b = CiBodies {
        runs: None,
        checks: None,
        statuses: Some((&c, &l)),
        named: &[],
        ignore: &[],
    };
    assert!(matches!(
        evaluate_sources("o/r", SHA, &b, &[], Some(&req)).unwrap(),
        CiFacts::Green { .. }
    ));
    // a required context without any check or status is red, and names the gap
    let more = vec!["ci/lint".to_string()];
    let r = reasons(
        evaluate_sources("o/r", SHA, &b, &[(SRC_CHECKS, "x".into())], Some(&more)).unwrap(),
    );
    assert!(
        r.iter()
            .any(|x| x.contains("'ci/lint'") && x.contains("unreadable: check-runs")),
        "{r:?}"
    );
}

#[test]
fn check_runs_unreadable_workflow_runs_green_is_accepted_and_the_sources_are_recorded() {
    // the case of a fine-grained token: actions=read and statuses=read, no checks=read
    let runs = fx("runs-success.json");
    let (c, l) = stat("empty");
    let g = evaluate_sources(
        "example-org/app",
        SHA,
        &CiBodies {
            runs: Some(&runs),
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[(SRC_CHECKS, "HTTP 403, token lacks checks=read".into())],
        None,
    )
    .unwrap();
    match g {
        CiFacts::Green { run_url, sources } => {
            assert_eq!(
                run_url,
                "https://github.com/example-org/app/actions/runs/9002"
            );
            assert_eq!(
                sources.text(),
                "workflow-runs, statuses; unreadable: check-runs"
            );
        }
        other => panic!("{other:?}"),
    }
    // the same with a failed run is refused (control: the check can say no)
    let bad = fx("runs-failure.json");
    let r = evaluate_sources(
        "example-org/app",
        SHA,
        &CiBodies {
            runs: Some(&bad),
            checks: None,
            statuses: Some((&c, &l)),
            named: &[],
            ignore: &[],
        },
        &[(SRC_CHECKS, "x".into())],
        None,
    )
    .unwrap();
    assert!(matches!(r, CiFacts::NotGreen(_)), "{r:?}");
}

// ---- spec/ci#which-ci-runs-count: which runs count ----

const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn run(id: u64, event: &str, conclusion: &str, sha: &str) -> Value {
    json!({
        "id": id,
        "name": "ci",
        "head_sha": sha,
        "event": event,
        "status": "completed",
        "conclusion": conclusion,
        "check_suite_id": id * 10,
        "html_url": format!("https://github.com/o/r/actions/runs/{id}"),
    })
}

fn judge_runs(runs: Vec<Value>, named: Vec<Value>) -> CiFacts {
    let body = json!({ "workflow_runs": runs });
    let checks = json!({ "check_runs": [] });
    evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: Some(&body),
            checks: Some(&checks),
            statuses: None,
            named: &named,
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap()
}

fn green_runs(f: CiFacts) -> (String, CiSources) {
    match f {
        CiFacts::Green { run_url, sources } => (run_url, sources),
        other => panic!("expected Green, got {other:?}"),
    }
}

#[test]
fn a_green_push_run_or_pull_request_run_counts_and_passes() {
    for event in ["push", "pull_request"] {
        let (url, s) = green_runs(judge_runs(vec![run(1, event, "success", SHA)], vec![]));
        assert!(url.ends_with("/runs/1"));
        assert_eq!(s.counted, vec![format!("1 ({event}, success)")]);
        assert!(s.info.is_empty());
    }
}

#[test]
fn a_red_push_run_or_pull_request_run_is_red() {
    for event in ["push", "pull_request"] {
        let r = reasons(judge_runs(vec![run(1, event, "failure", SHA)], vec![]));
        assert!(r[0].contains("concluded failure"), "{r:?}");
    }
}

#[test]
fn only_a_green_dispatch_run_is_no_counted_run_and_is_listed_as_information() {
    match judge_runs(vec![run(5, "workflow_dispatch", "success", SHA)], vec![]) {
        CiFacts::OnlyInformation { info, .. } => {
            assert_eq!(info, vec!["5 (workflow_dispatch, success)"]);
        }
        other => panic!("expected OnlyInformation, got {other:?}"),
    }
}

#[test]
fn a_red_schedule_run_is_ignored_when_the_push_run_is_green() {
    let (url, s) = green_runs(judge_runs(
        vec![
            run(1, "push", "success", SHA),
            run(2, "schedule", "failure", SHA),
            run(3, "workflow_dispatch", "failure", SHA),
        ],
        vec![],
    ));
    assert!(url.ends_with("/runs/1"));
    assert_eq!(s.counted, vec!["1 (push, success)"]);
    assert_eq!(
        s.info,
        vec!["2 (schedule, failure)", "3 (workflow_dispatch, failure)"]
    );
    assert_eq!(
        s.runs_text().unwrap(),
        "counted: 1 (push, success); information only: 2 (schedule, failure), 3 (workflow_dispatch, failure)"
    );
}

#[test]
fn a_named_green_run_never_hides_a_red_push_run() {
    let r = reasons(judge_runs(
        vec![
            run(1, "push", "failure", SHA),
            run(5, "workflow_dispatch", "success", SHA),
        ],
        vec![run(5, "workflow_dispatch", "success", SHA)],
    ));
    assert!(
        r.iter()
            .any(|x| x.contains("run 1") && x.contains("failure")),
        "{r:?}"
    );
    // a red PR run too
    let r = reasons(judge_runs(
        vec![run(1, "pull_request", "failure", SHA)],
        vec![run(5, "workflow_dispatch", "success", SHA)],
    ));
    assert!(r.iter().any(|x| x.contains("run 1")), "{r:?}");
}

#[test]
fn a_named_green_dispatch_run_passes_without_a_push_run_and_is_named() {
    let d = run(5, "workflow_dispatch", "success", SHA);
    let (url, s) = green_runs(judge_runs(vec![d.clone()], vec![d]));
    assert!(url.ends_with("/runs/5"));
    assert_eq!(s.counted, vec!["5 (workflow_dispatch, named, success)"]);
    assert!(s.info.is_empty());
    // the named run is not in the listing (older than the page): still counted
    let (_, s) = green_runs(judge_runs(
        vec![run(1, "push", "success", SHA)],
        vec![run(5, "workflow_dispatch", "success", SHA)],
    ));
    assert_eq!(
        s.counted,
        vec!["1 (push, success)", "5 (workflow_dispatch, named, success)"]
    );
}

#[test]
fn a_named_red_run_is_red() {
    let r = reasons(judge_runs(
        vec![run(1, "push", "success", SHA)],
        vec![run(5, "workflow_dispatch", "failure", SHA)],
    ));
    assert!(
        r.iter()
            .any(|x| x.contains("named run 5") && x.contains("failure")),
        "{r:?}"
    );
}

#[test]
fn the_jobs_of_an_information_run_do_not_count_as_check_runs() {
    let runs = json!({ "workflow_runs": [
        run(1, "push", "success", SHA),
        run(2, "workflow_dispatch", "failure", SHA),
    ]});
    let check = |id: u64, suite: u64, c: &str| {
        json!({"id": id, "name": "timing", "head_sha": SHA, "status": "completed",
               "conclusion": c, "html_url": "https://x/c", "check_suite": {"id": suite}})
    };
    // suite 20 belongs to run 2 (dispatch): its red job is information
    let checks = json!({ "check_runs": [check(7, 10, "success"), check(8, 20, "failure")] });
    let f = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: Some(&runs),
            checks: Some(&checks),
            statuses: None,
            named: &[],
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap();
    green_runs(f);
    // a red job of the push run (suite 10) is red
    let checks = json!({ "check_runs": [check(7, 10, "failure")] });
    let f = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: Some(&runs),
            checks: Some(&checks),
            statuses: None,
            named: &[],
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap();
    assert!(matches!(f, CiFacts::NotGreen(_)));
}

#[test]
fn a_red_commit_status_stays_red_whatever_runs_are_named() {
    let (c, l) = stat("failure");
    let runs = json!({ "workflow_runs": [run(1, "push", "success", SHA)] });
    let d = run(5, "workflow_dispatch", "success", SHA);
    let f = evaluate_sources(
        "o/r",
        SHA,
        &CiBodies {
            runs: Some(&runs),
            checks: None,
            statuses: Some((&c, &l)),
            named: std::slice::from_ref(&d),
            ignore: &[],
        },
        &[],
        None,
    )
    .unwrap();
    assert!(matches!(f, CiFacts::NotGreen(_)), "{f:?}");
}

#[test]
fn a_run_on_another_sha_is_refused() {
    let other = run(5, "workflow_dispatch", "success", OTHER);
    let e = check_named_run(&other, 5, SHA).unwrap_err();
    assert_eq!(e.rule, "evidence-run-wrong-sha");
    // the same run on the head passes
    assert!(check_named_run(&run(5, "workflow_dispatch", "success", SHA), 5, SHA).is_ok());
    // GitHub answered with another run than the one asked for
    assert_eq!(
        check_named_run(&run(6, "push", "success", SHA), 5, SHA)
            .unwrap_err()
            .rule,
        "evidence-run-ref"
    );
}

#[test]
fn a_run_in_a_foreign_repo_is_refused_and_a_run_in_the_initiative_is_read() {
    let repos = vec!["o/r".to_string(), "o/r2".to_string()];
    let e = parse_run_ref("https://github.com/x/other/actions/runs/9", "o/r", &repos).unwrap_err();
    assert_eq!(e.rule, "evidence-run-foreign-repo");
    assert_eq!(
        parse_run_ref(
            "https://github.com/o/r2/actions/runs/9/job/3",
            "o/r",
            &repos
        )
        .unwrap(),
        NamedRun {
            repo: "o/r2".into(),
            id: 9
        }
    );
    assert_eq!(
        parse_run_ref("123", "o/r", &repos).unwrap(),
        NamedRun {
            repo: "o/r".into(),
            id: 123
        }
    );
    for bad in [
        "",
        "abc",
        "https://github.com/o/r/pull/3",
        "https://example.com/o/r/actions/runs/1",
    ] {
        assert_eq!(
            parse_run_ref(bad, "o/r", &repos).unwrap_err().rule,
            "evidence-run-ref",
            "{bad}"
        );
    }
}

// ---- the named runs of the evidence count again ----

fn repos() -> Vec<String> {
    vec!["o/r".to_string(), "o/other".to_string()]
}

fn nr(repo: &str, id: u64) -> NamedRun {
    NamedRun {
        repo: repo.into(),
        id,
    }
}

#[test]
fn the_named_runs_of_a_ci_runs_line_are_read_and_only_those() {
    // the form that `done` writes (the usual case)
    let line =
        "counted: 39109920426 (workflow_dispatch, named, success); ignored: desktop-app (skipped)";
    assert_eq!(
        named_in_ci_runs(line, "o/r", &repos()),
        vec![nr("o/r", 39109920426)]
    );
    // a counted push run is no named run; information runs and ignored checks are not either
    let line = "counted: 1 (push, success), 5 (workflow_dispatch, named, success), https://github.com/o/other/actions/runs/6 (workflow_dispatch, named, success); information only: 7 (schedule, named, failure); ignored: named (skipped)";
    assert_eq!(
        named_in_ci_runs(line, "o/r", &repos()),
        vec![nr("o/r", 5), nr("o/other", 6)]
    );
    // no counted part, or nothing named: no run
    for line in [
        "",
        "information only: 7 (workflow_dispatch, failure)",
        "counted: 1 (push, success)",
        "ignored: x (skipped)",
    ] {
        assert!(named_in_ci_runs(line, "o/r", &repos()).is_empty(), "{line}");
    }
    // malformed entries never count and never panic; a foreign repo URL never counts
    for line in [
        "counted: abc (workflow_dispatch, named, success)",
        "counted: 5 (workflow_dispatch, named, success",
        "counted: (named)",
        "counted: ((((named",
        "counted: 5 (named)) 6 (",
        "counted: https://github.com/x/y/actions/runs/5 (workflow_dispatch, named, success)",
        "counted: -5 (workflow_dispatch, named, success)",
        "counted: 99999999999999999999999 (workflow_dispatch, named, success)",
        "counted: \u{e9}\u{e9} (named), \u{1f600} (",
    ] {
        assert!(named_in_ci_runs(line, "o/r", &repos()).is_empty(), "{line}");
    }
    // the headless check: any named entry, also one that does not parse
    assert!(names_runs(
        "counted: 39109920426 (workflow_dispatch, named, success); ignored: desktop-app (skipped)"
    ));
    assert!(names_runs(
        "counted: abc (workflow_dispatch, named, success)"
    ));
    assert!(!names_runs(
        "counted: 1 (push, success); information only: 7 (schedule, named, failure)"
    ));
    assert!(!names_runs(""));
    // a run that is named twice is one run
    assert_eq!(
        named_in_ci_runs(
            "counted: 5 (workflow_dispatch, named, success), 5 (workflow_dispatch, named, success)",
            "o/r",
            &repos()
        ),
        vec![nr("o/r", 5)]
    );
}

#[test]
fn the_runs_of_the_command_only_add_runs() {
    let r = CiRuns::new(
        vec![nr("o/r", 5)],
        vec![nr("O/R", 5), nr("o/r", 6), nr("o/r", 6)],
    );
    assert_eq!(r.evidence, vec![nr("o/r", 5)]);
    assert_eq!(r.extra, vec![nr("o/r", 6)]);
}
