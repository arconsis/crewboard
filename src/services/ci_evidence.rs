//! Reads the CI facts of one commit from GitHub and judges them (spec/tickets#evidence,
//! spec/tickets#verification-by-the-coordinator). [`fetch_ci`] does the GET calls; the judgement
//! is `domain::ci::evaluate::evaluate_sources`.

use crate::base::{GhError, Refusal};
use crate::domain::ci::evaluate::{
    CiBodies, NamedRun, SRC_CHECKS, SRC_RUNS, SRC_STATUSES, check_named_run, evaluate_sources,
    required_contexts,
};
use crate::error::CbResult;
use crate::github::Rest;
use crate::rules::evidence::CiFacts;
use serde_json::Value;

/// Why a 403 on a CI source is "unreadable": names the endpoint class and the permission
/// that GitHub says the token lacks (header `x-accepted-github-permissions`).
fn unreadable_why(status: u16, message: &str, perms: Option<&str>) -> String {
    match (status, perms) {
        (403, Some(p)) => format!("HTTP 403, token lacks {p}"),
        (403, None) => format!("HTTP 403, token lacks a permission ({message})"),
        _ => format!("HTTP {status}: {message}"),
    }
}

/// What to read: the commit `sha` of `repo`, `base` the base branch of the PR (for the required
/// checks), `declared` true when the ticket declares `CI: none`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CiQuery<'a> {
    pub repo: &'a str,
    pub sha: &'a str,
    pub base: &'a str,
    pub declared: bool,
    /// Runs named by `--run` of the command (spec/ci#which-ci-runs-count), without the runs of
    /// `evidence`; build both with `CiRuns::new`. A run that does not exist or is not a run of
    /// `sha` refuses.
    pub named: &'a [NamedRun],
    /// Runs named by the `ci-runs:` line of the evidence. A run with that problem
    /// does not count and comes back as a finding.
    pub evidence: &'a [NamedRun],
    /// Check names that AUTHORITY.md lists as ignored.
    pub ignore: &'a [String],
}

/// Read the CI facts of `q.sha` in `q.repo` over any GET function (the tests pass recorded
/// answers). The second value holds the findings about the runs of `q.evidence`.
///
/// Three sources are read, each on its own: workflow runs, check runs, commit statuses. Each
/// named run is read and checked (it must belong to `sha`) before anything is judged.
///
/// - HTTP 403 on a source (not a rate limit) makes that source *unreadable*. The other
///   sources still count. Only when none can be read is the result
///   [`CiFacts::NotQueried`] (refused as `evidence-ci-unreadable`, or "none" plus a warning
///   under `CI: none`). It is never a generic error and never a silent "no CI".
/// - HTTP 404 on a source is unreadable only with `declared`; otherwise an error.
/// - Every other HTTP error (5xx), a rate limit and every network error is returned (exit 3).
/// - A run of `q.named` that does not exist or is not a run of `sha` refuses, as in `done`. A
///   run of `q.evidence` with that problem does not count and comes back as a finding (the
///   second value): the evidence does not hold. The CI is judged without it.
pub fn fetch_ci(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    q: &CiQuery,
) -> CbResult<(CiFacts, Vec<Refusal>)> {
    let mut named: Vec<(&NamedRun, Value)> = Vec::new();
    let mut findings: Vec<Refusal> = Vec::new();
    for n in q.evidence {
        match read_named_run(get, n, q.sha) {
            Ok(b) => named.push((n, b)),
            Err(crate::error::CbError::Refused(r)) => findings.push(Refusal::new(
                r.rule,
                format!("named run of the evidence: {}", r.message),
            )),
            Err(e) => return Err(e),
        }
    }
    for n in q.named {
        let b = read_named_run(get, n, q.sha)?;
        named.push((n, b));
    }
    let facts = fetch_ci_with_bodies(get, q.repo, q.sha, q.base, q.declared, &named, q.ignore)?;
    Ok((facts, findings))
}

/// One read: a body, or the reason that the source is unreadable.
enum Got {
    Body(Value),
    Unreadable(String),
}

fn read_source(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    path: &str,
    declared: bool,
) -> CbResult<Got> {
    match get(path) {
        Ok(r) => Ok(Got::Body(r.body)),
        Err(GhError::Http {
            status,
            message,
            accepted_permissions,
        }) if (status == 403 && !message.starts_with("rate limited"))
            || (declared && status == 404) =>
        {
            Ok(Got::Unreadable(unreadable_why(
                status,
                &message,
                accepted_permissions.as_deref(),
            )))
        }
        Err(e) => Err(e.into()),
    }
}

/// Read one named run and check it ([`check_named_run`]): it must exist and belong to `sha`.
fn read_named_run(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    n: &NamedRun,
    sha: &str,
) -> CbResult<Value> {
    let path = format!("repos/{}/actions/runs/{}", n.repo, n.id);
    let body = match get(&path) {
        Ok(r) => r.body,
        Err(GhError::Http { status: 404, .. }) => {
            return Err(Refusal::new(
                "evidence-run-ref",
                format!("run {} does not exist in {}", n.id, n.repo),
            )
            .into());
        }
        Err(e) => return Err(e.into()),
    };
    check_named_run(&body, n.id, sha)?;
    Ok(body)
}

/// The rest of [`fetch_ci`]: the named runs are read and checked already.
fn fetch_ci_with_bodies(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    repo: &str,
    sha: &str,
    base: &str,
    declared: bool,
    named: &[(&NamedRun, Value)],
    ignore: &[String],
) -> CbResult<CiFacts> {
    let named_bodies: Vec<Value> = named.iter().map(|(_, b)| b.clone()).collect();
    let facts = fetch_ci_sources(get, repo, sha, base, declared, &named_bodies, ignore)?;
    Ok(with_run_urls(facts, repo, named))
}

/// A counted named run of another repo than `repo` is named by its URL in the `ci-runs:` text,
/// so that a later judgement reads it in its own repo: `https://github.com/<owner>/
/// <repo>/actions/runs/5 (workflow_dispatch, named, success)`. A run of `repo` keeps its id.
fn with_run_urls(facts: CiFacts, repo: &str, named: &[(&NamedRun, Value)]) -> CiFacts {
    let CiFacts::Green {
        run_url,
        mut sources,
    } = facts
    else {
        return facts;
    };
    for (n, _) in named {
        if n.repo.eq_ignore_ascii_case(repo) {
            continue;
        }
        let prefix = format!("{} (", n.id);
        for c in sources.counted.iter_mut() {
            if c.starts_with(&prefix) && c.contains(", named, ") {
                *c = format!(
                    "https://github.com/{}/actions/runs/{}{}",
                    n.repo,
                    n.id,
                    &c[n.id.to_string().len()..]
                );
            }
        }
    }
    CiFacts::Green { run_url, sources }
}

/// The default CI sources of `sha` in `repo`, judged with the named run bodies.
fn fetch_ci_sources(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    repo: &str,
    sha: &str,
    base: &str,
    declared: bool,
    named_bodies: &[Value],
    ignore: &[String],
) -> CbResult<CiFacts> {
    let runs = read_source(
        get,
        &format!("repos/{repo}/actions/runs?head_sha={sha}&per_page=100"),
        declared,
    )?;
    let checks = read_source(
        get,
        &format!("repos/{repo}/commits/{sha}/check-runs?per_page=100"),
        declared,
    )?;
    let combined = read_source(get, &format!("repos/{repo}/commits/{sha}/status"), declared)?;
    let listing = read_source(
        get,
        &format!("repos/{repo}/commits/{sha}/statuses?per_page=100"),
        declared,
    )?;
    let mut unreadable: Vec<(&str, String)> = Vec::new();
    let runs_b = match runs {
        Got::Body(b) => Some(b),
        Got::Unreadable(w) => {
            unreadable.push((SRC_RUNS, w));
            None
        }
    };
    let checks_b = match checks {
        Got::Body(b) => Some(b),
        Got::Unreadable(w) => {
            unreadable.push((SRC_CHECKS, w));
            None
        }
    };
    let statuses_b = match (combined, listing) {
        (Got::Body(c), Got::Body(l)) => Some((c, l)),
        (Got::Unreadable(w), _) | (_, Got::Unreadable(w)) => {
            unreadable.push((SRC_STATUSES, w));
            None
        }
    };
    let bodies = CiBodies {
        runs: runs_b.as_ref(),
        checks: checks_b.as_ref(),
        statuses: statuses_b.as_ref().map(|(c, l)| (c, l)),
        named: named_bodies,
        ignore,
    };
    if bodies.runs.is_none()
        && bodies.checks.is_none()
        && bodies.statuses.is_none()
        && named_bodies.is_empty()
    {
        return Ok(evaluate_sources(repo, sha, &bodies, &unreadable, None)?);
    }
    // Branch protection: 404 (not protected) and 403 (token cannot read it) both mean
    // "no list of required contexts": then every check run and status of the commit is judged.
    let required = match get(&format!(
        "repos/{repo}/branches/{base}/protection/required_status_checks"
    )) {
        Ok(r) => Some(required_contexts(&r.body)?),
        Err(GhError::Http {
            status: 403 | 404, ..
        }) => None,
        Err(e) => return Err(e.into()),
    };
    Ok(evaluate_sources(
        repo,
        sha,
        &bodies,
        &unreadable,
        required.as_deref(),
    )?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ci::evaluate::{CiRuns, named_in_ci_runs};
    use crate::rules::evidence::CiSources;
    use serde_json::json;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn fx(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/ci/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn reasons(f: CiFacts) -> Vec<String> {
        match f {
            CiFacts::NotGreen(r) => r,
            other => panic!("expected NotGreen, got {other:?}"),
        }
    }

    /// The facts of `repo` at `SHA` (base `main`), without named runs.
    fn facts_of(
        get: &dyn Fn(&str) -> Result<Rest, GhError>,
        repo: &str,
        declared: bool,
    ) -> CbResult<CiFacts> {
        let q = CiQuery {
            repo,
            sha: SHA,
            base: "main",
            declared,
            ..CiQuery::default()
        };
        fetch_ci(get, &q).map(|(facts, _)| facts)
    }

    /// The facts of `o/r` at `SHA` with the runs of `--run`.
    fn facts_named(
        get: &dyn Fn(&str) -> Result<Rest, GhError>,
        named: &[NamedRun],
    ) -> CbResult<CiFacts> {
        let q = CiQuery {
            repo: "o/r",
            sha: SHA,
            base: "main",
            named,
            ..CiQuery::default()
        };
        fetch_ci(get, &q).map(|(facts, _)| facts)
    }

    /// The facts of `o/r` at `SHA` and the findings, for the runs of the evidence and of `--run`.
    fn fetch_runs(
        get: &dyn Fn(&str) -> Result<Rest, GhError>,
        runs: &CiRuns,
    ) -> CbResult<(CiFacts, Vec<Refusal>)> {
        let q = CiQuery {
            repo: "o/r",
            sha: SHA,
            base: "main",
            named: &runs.extra,
            evidence: &runs.evidence,
            ..CiQuery::default()
        };
        fetch_ci(get, &q)
    }

    fn rest(body: Value) -> Rest {
        Rest {
            etag: None,
            link_next: None,
            body,
        }
    }

    fn http(status: u16, message: &str, perms: Option<&str>) -> GhError {
        GhError::Http {
            status,
            message: message.into(),
            accepted_permissions: perms.map(str::to_string),
        }
    }

    /// check-runs answers `check_runs_answer`; workflow runs are empty; statuses and the
    /// branch protection answer 403 (the token of the sandbox: no statuses, no admin).
    fn fetch(check_runs_answer: Result<Rest, GhError>, declared: bool) -> CbResult<CiFacts> {
        let get = |p: &str| -> Result<Rest, GhError> {
            if p.contains("/actions/runs") {
                Ok(rest(fx("runs-empty.json")))
            } else if p.contains("/check-runs") {
                check_runs_answer.clone()
            } else if p.contains("/status") {
                Err(http(403, "no", Some("statuses=read")))
            } else {
                Err(http(
                    403,
                    "Upgrade to GitHub Pro",
                    Some("administration=read"),
                ))
            }
        };
        facts_of(&get, "o/r", declared)
    }

    /// Every source answers from `answers` by path part; a part that is not there is 403.
    fn fetch_with(answers: &[(&str, Result<Rest, GhError>)], declared: bool) -> CbResult<CiFacts> {
        let get = |p: &str| -> Result<Rest, GhError> {
            // `/statuses` contains `/status`: look for the longer part first.
            let mut keyed: Vec<&(&str, Result<Rest, GhError>)> = answers.iter().collect();
            keyed.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
            for (k, r) in keyed {
                if p.contains(k) {
                    return r.clone();
                }
            }
            Err(http(403, "not accessible", Some("x=read")))
        };
        facts_of(&get, "o/r", declared)
    }

    #[test]
    fn with_workflow_runs_readable_a_403_on_check_runs_is_not_unreadable_ci() {
        // A fine-grained token cannot get checks=read. One readable source is enough to
        // judge, so this is not `NotQueried`.
        for declared in [false, true] {
            let e = http(
                403,
                "Resource not accessible by personal access token",
                Some("checks=read"),
            );
            let got = fetch(Err(e), declared).unwrap();
            assert!(
                matches!(&got, CiFacts::NoCi(u) if u.contains(&"check-runs".to_string())),
                "{declared}: {got:?}"
            );
        }
    }

    #[test]
    fn when_every_source_answers_403_ci_is_unreadable_and_names_each_permission() {
        for declared in [false, true] {
            let r = fetch_with(
                &[
                    ("/actions/runs", Err(http(403, "no", Some("actions=read")))),
                    ("/check-runs", Err(http(403, "no", Some("checks=read")))),
                    ("/status", Err(http(403, "no", Some("statuses=read")))),
                ],
                declared,
            )
            .unwrap();
            match r {
                CiFacts::NotQueried(w) => {
                    for want in [
                        "workflow-runs: HTTP 403, token lacks actions=read",
                        "check-runs: HTTP 403, token lacks checks=read",
                        "statuses: HTTP 403, token lacks statuses=read",
                    ] {
                        assert!(w.contains(want), "{w}");
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_readable_source_never_hides_a_red_one_and_an_unreadable_one_never_makes_green() {
        // workflow runs unreadable, check runs readable and red: red
        let r = fetch_with(
            &[
                ("/actions/runs", Err(http(403, "no", Some("actions=read")))),
                ("/check-runs", Ok(rest(json!({"check_runs": [
                    {"id": 2, "name": "ok", "head_sha": SHA, "status": "completed", "conclusion": "failure", "html_url": "https://x/2"}
                ]})))),
                ("/status", Err(http(403, "no", Some("statuses=read")))),
            ],
            false,
        )
        .unwrap();
        assert!(matches!(r, CiFacts::NotGreen(_)), "{r:?}");
        // control: the same wiring with a green check run is green and names the gaps
        let g = fetch_with(
            &[
                ("/actions/runs", Err(http(403, "no", Some("actions=read")))),
                ("/check-runs", Ok(rest(json!({"check_runs": [
                    {"id": 2, "name": "ok", "head_sha": SHA, "status": "completed", "conclusion": "success", "html_url": "https://x/2"}
                ]})))),
                ("/status", Err(http(403, "no", Some("statuses=read")))),
            ],
            false,
        )
        .unwrap();
        match g {
            CiFacts::Green { sources, .. } => {
                assert_eq!(
                    sources.text(),
                    "check-runs; unreadable: workflow-runs, statuses"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_403_on_workflow_runs_alone_is_named_in_the_sources_not_an_error() {
        let get = |p: &str| -> Result<Rest, GhError> {
            if p.contains("/actions/runs") {
                Err(http(403, "no", Some("actions=read")))
            } else if p.contains("/status") {
                Err(http(403, "no", Some("statuses=read")))
            } else if p.contains("/check-runs") {
                Ok(rest(fx("checks-empty.json")))
            } else {
                Err(http(404, "Branch not protected", None))
            }
        };
        assert_eq!(
            facts_of(&get, "o/r", false).unwrap(),
            CiFacts::NoCi(vec!["workflow-runs".into(), "statuses".into()])
        );
    }

    #[test]
    fn a_500_or_a_rate_limit_stays_an_error_in_both_modes() {
        for declared in [false, true] {
            for e in [
                http(500, "boom", None),
                http(502, "bad gateway", None),
                http(403, "rate limited; retry after 90 s", None),
                GhError::Network("timeout".into()),
            ] {
                assert!(fetch(Err(e.clone()), declared).is_err(), "{e:?} {declared}");
                // also on the statuses source
                let r = fetch_with(&[("/status", Err(e.clone()))], declared);
                assert!(r.is_err(), "statuses {e:?} {declared}");
            }
        }
    }

    #[test]
    fn a_404_is_unreadable_only_with_the_declaration() {
        assert!(fetch(Err(http(404, "Not Found", None)), false).is_err());
        // with the declaration the source is unreadable; the others (empty) say NoCi
        assert_eq!(
            fetch(Err(http(404, "Not Found", None)), true).unwrap(),
            CiFacts::NoCi(vec!["check-runs".into(), "statuses".into()])
        );
    }

    #[test]
    fn readable_empty_ci_is_still_no_ci() {
        // control: the same wiring without a 403 gives NoCi, so evidence-ci-missing stays.
        let f = fetch(Ok(rest(fx("checks-empty.json"))), false).unwrap();
        assert_eq!(f, CiFacts::NoCi(vec!["statuses".into()]));
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
    fn fetch_reads_all_three_sources_and_judges_workflow_runs_and_statuses_together() {
        let (c, l) = stat("success");
        let get = |p: &str| -> Result<Rest, GhError> {
            if p.contains("/actions/runs") {
                Ok(rest(fx("runs-success.json")))
            } else if p.contains("/check-runs") {
                Err(http(403, "no", Some("checks=read")))
            } else if p.ends_with("/statuses?per_page=100") {
                Ok(rest(l.clone()))
            } else if p.ends_with("/status") {
                Ok(rest(c.clone()))
            } else {
                Err(http(404, "Branch not protected", None))
            }
        };
        match facts_of(&get, "example-org/app", false).unwrap() {
            CiFacts::Green { sources, .. } => assert_eq!(
                sources.text(),
                "workflow-runs, statuses; unreadable: check-runs"
            ),
            other => panic!("{other:?}"),
        }
        // control: a failing status in the same wiring is refused
        let (cf, lf) = stat("failure");
        let get2 = |p: &str| -> Result<Rest, GhError> {
            if p.ends_with("/statuses?per_page=100") {
                Ok(rest(lf.clone()))
            } else if p.ends_with("/status") {
                Ok(rest(cf.clone()))
            } else {
                get(p)
            }
        };
        assert!(matches!(
            facts_of(&get2, "example-org/app", false).unwrap(),
            CiFacts::NotGreen(_)
        ));
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

    fn green_runs(f: CiFacts) -> (String, CiSources) {
        match f {
            CiFacts::Green { run_url, sources } => (run_url, sources),
            other => panic!("expected Green, got {other:?}"),
        }
    }

    #[test]
    fn fetch_reads_a_named_run_checks_its_sha_and_refuses_a_foreign_one_before_judging() {
        let named = vec![NamedRun {
            repo: "o/r".into(),
            id: 5,
        }];
        let get_with = |sha: &'static str| {
            move |p: &str| -> Result<Rest, GhError> {
                if p.ends_with("/actions/runs/5") {
                    Ok(rest(run(5, "workflow_dispatch", "success", sha)))
                } else if p.contains("/actions/runs") {
                    Ok(rest(json!({"workflow_runs": []})))
                } else if p.contains("/check-runs") {
                    Ok(rest(json!({"check_runs": []})))
                } else {
                    Err(http(403, "no", Some("x=read")))
                }
            }
        };
        let ok = facts_named(&get_with(SHA), &named).unwrap();
        let (url, s) = green_runs(ok);
        assert!(url.ends_with("/runs/5"));
        assert_eq!(s.counted, vec!["5 (workflow_dispatch, named, success)"]);
        let err = facts_named(&get_with(OTHER), &named).unwrap_err();
        assert!(err.to_string().contains("evidence-run-wrong-sha"), "{err}");
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

    /// A fake GitHub: no listed run and no check run for `SHA`, statuses unreadable; the runs of
    /// `named` answer `GET actions/runs/{id}`, every other id 404.
    fn get_named(named: Vec<Value>) -> impl Fn(&str) -> Result<Rest, GhError> {
        move |p: &str| -> Result<Rest, GhError> {
            if let Some(id) = p.split("/actions/runs/").nth(1) {
                return named
                    .iter()
                    .find(|r| r.get("id").and_then(Value::as_u64).unwrap_or(0).to_string() == id)
                    .map(|r| rest(r.clone()))
                    .ok_or_else(|| http(404, "Not Found", None));
            }
            if p.contains("/actions/runs") {
                Ok(rest(json!({"workflow_runs": []})))
            } else if p.contains("/check-runs") {
                Ok(rest(json!({"check_runs": []})))
            } else {
                Err(http(403, "no", Some("x=read")))
            }
        }
    }

    #[test]
    fn a_named_run_of_the_evidence_is_judged_again_and_a_problem_is_a_finding() {
        let ev = |ids: &[u64]| CiRuns::new(ids.iter().map(|i| nr("o/r", *i)).collect(), vec![]);
        // green dispatch run of the head: green, no finding, named in the runs text
        let get = get_named(vec![run(5, "workflow_dispatch", "success", SHA)]);
        let (f, found) = fetch_runs(&get, &ev(&[5])).unwrap();
        assert!(found.is_empty(), "{found:?}");
        let (_, s) = green_runs(f);
        assert_eq!(s.counted, vec!["5 (workflow_dispatch, named, success)"]);
        // red now: not green
        let get = get_named(vec![run(5, "workflow_dispatch", "failure", SHA)]);
        let (f, found) = fetch_runs(&get, &ev(&[5])).unwrap();
        assert!(found.is_empty());
        assert!(
            reasons(f).iter().any(|x| x.contains("named run 5")),
            "a red named run must be red"
        );
        // a run of another sha: a finding, and the run does not count
        let get = get_named(vec![run(5, "workflow_dispatch", "success", OTHER)]);
        let (f, found) = fetch_runs(&get, &ev(&[5])).unwrap();
        assert_eq!(
            found.iter().map(|r| r.rule).collect::<Vec<_>>(),
            vec!["evidence-run-wrong-sha"]
        );
        assert!(!matches!(f, CiFacts::Green { .. }), "{f:?}");
        // a run that is gone: a finding
        let (_, found) = fetch_runs(&get_named(vec![]), &ev(&[5])).unwrap();
        assert_eq!(found[0].rule, "evidence-run-ref");
        // the same problems with a run of the command refuse, as in `done`
        let get = get_named(vec![run(6, "workflow_dispatch", "success", OTHER)]);
        let err = fetch_runs(&get, &CiRuns::new(vec![], vec![nr("o/r", 6)])).unwrap_err();
        assert!(err.to_string().contains("evidence-run-wrong-sha"), "{err}");
    }

    #[test]
    fn a_counted_named_run_of_another_repo_is_named_by_its_url() {
        let get = get_named(vec![run(6, "workflow_dispatch", "success", SHA)]);
        let runs = CiRuns::new(vec![], vec![nr("o/other", 6)]);
        let (f, _) = fetch_runs(&get, &runs).unwrap();
        let (_, s) = green_runs(f);
        let text = s.runs_text().unwrap();
        assert_eq!(
            text,
            "counted: https://github.com/o/other/actions/runs/6 (workflow_dispatch, named, success)"
        );
        // and the next judgement reads it back in its own repo
        assert_eq!(
            named_in_ci_runs(&text, "o/r", &repos()),
            vec![nr("o/other", 6)]
        );
    }
}
