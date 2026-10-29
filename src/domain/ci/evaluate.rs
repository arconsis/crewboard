//! Pure judgement of the CI facts of one commit (spec/tickets#evidence,
//! spec/tickets#verification-by-the-coordinator).
//!
//! [`evaluate_sources`] takes the GitHub answers that could be read (workflow runs for
//! the SHA, check runs of the SHA, commit statuses, required status checks of the base branch) and judges them by
//! **conclusion**. A job that is called "fail" but concluded `success` is green; a job
//! called "ok" that concluded `failure` is not. Names are used for one thing only: to say
//! which check run belongs to a required context of the branch protection.
//!
//! The module reads no answer itself, and has no client, clock or environment. The reads are in
//! `services::ci_evidence`. The tests of this file run `evaluate_ci` on
//! response fixtures in `tests/fixtures/ci/` (see the README there for how each was made).

use crate::base::{GhError, Refusal};
use crate::rules::evidence::{CiFacts, CiSources};
use serde_json::Value;

/// Events whose workflow runs count as CI (spec/ci#which-ci-runs-count). A run of any other event
/// (`schedule`, `workflow_dispatch`, `repository_dispatch`, `workflow_run`, ...) is information
/// only, unless `--run` names it. `pull_request_target` counts as a PR event: the code counted
/// every run before, and a PR run is a PR run.
pub const COUNTED_EVENTS: [&str; 3] = ["push", "pull_request", "pull_request_target"];

fn run_id(r: &Value) -> u64 {
    r.get("id").and_then(Value::as_u64).unwrap_or(0)
}

fn counts(r: &Value) -> bool {
    text(r, "event").is_some_and(|e| COUNTED_EVENTS.contains(&e))
}

/// `9002 (push, success)`; a run that did not complete shows its status.
fn describe_run(r: &Value, named: bool) -> String {
    let status = text(r, "status").unwrap_or("?");
    let outcome = if status == "completed" {
        text(r, "conclusion").unwrap_or("none")
    } else {
        status
    };
    format!(
        "{} ({}{}, {outcome})",
        run_id(r),
        text(r, "event").unwrap_or("?"),
        if named { ", named" } else { "" }
    )
}

/// A CI run named with `--run`: the repo and the run id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedRun {
    pub repo: String,
    pub id: u64,
}

/// Read one `--run` value: a run id (the run is in `pr_repo`) or a run URL
/// `https://github.com/<owner>/<repo>/actions/runs/<id>`. A URL of a repo outside the
/// initiative is refused before any call to that repo.
pub fn parse_run_ref(arg: &str, pr_repo: &str, initiative: &[String]) -> Result<NamedRun, Refusal> {
    let bad = || {
        Refusal::new(
            "evidence-run-ref",
            format!(
                "'{arg}' is not a run id or a run URL (https://github.com/<owner>/<repo>/actions/runs/<id>)"
            ),
        )
    };
    let arg = arg.trim();
    if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_digit()) {
        let id = arg.parse::<u64>().map_err(|_| bad())?;
        return Ok(NamedRun {
            repo: pr_repo.to_string(),
            id,
        });
    }
    let rest = arg.strip_prefix("https://github.com/").ok_or_else(bad)?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 5 || parts[2] != "actions" || parts[3] != "runs" {
        return Err(bad());
    }
    let id = parts[4]
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .parse::<u64>()
        .map_err(|_| bad())?;
    let repo = format!("{}/{}", parts[0], parts[1]);
    if !initiative.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
        return Err(Refusal::new(
            "evidence-run-foreign-repo",
            format!(
                "run {id} is in {repo}, which is not a repo of the initiative ({})",
                initiative.join(", ")
            ),
        ));
    }
    Ok(NamedRun { repo, id })
}

/// A named run must be the run it says and belong to the head `sha`.
pub fn check_named_run(body: &Value, want_id: u64, sha: &str) -> Result<(), Refusal> {
    if run_id(body) != want_id {
        return Err(Refusal::new(
            "evidence-run-ref",
            format!(
                "GitHub answered with run {} for run {want_id}",
                run_id(body)
            ),
        ));
    }
    let head = text(body, "head_sha").unwrap_or("?");
    if !head.eq_ignore_ascii_case(sha) {
        return Err(Refusal::new(
            "evidence-run-wrong-sha",
            format!(
                "run {want_id} ran on {head}, not on the PR head {sha}; a named run must belong to the head"
            ),
        ));
    }
    Ok(())
}

fn shape(m: impl Into<String>) -> GhError {
    GhError::Shape(m.into())
}

fn list<'a>(body: &'a Value, key: &str, what: &str) -> Result<&'a Vec<Value>, GhError> {
    body.get(key).and_then(Value::as_array).ok_or_else(|| {
        shape(format!(
            "{what} answer has no `{key}` list (got {})",
            match body {
                Value::Object(o) => format!(
                    "object with keys {:?}",
                    o.keys().take(5).collect::<Vec<_>>()
                ),
                other => other.to_string().chars().take(60).collect(),
            }
        ))
    })
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The contexts that the branch protection requires, from the answer of
/// `GET /repos/{r}/branches/{b}/protection/required_status_checks`.
pub fn required_contexts(body: &Value) -> Result<Vec<String>, GhError> {
    if body.get("contexts").is_none() && body.get("checks").is_none() {
        return Err(shape(
            "required_status_checks answer has neither `contexts` nor `checks`",
        ));
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = body.get("contexts").and_then(Value::as_array) {
        out.extend(a.iter().filter_map(|c| c.as_str().map(str::to_string)));
    }
    if let Some(a) = body.get("checks").and_then(Value::as_array) {
        out.extend(
            a.iter()
                .filter_map(|c| text(c, "context").map(str::to_string)),
        );
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The run is `completed` with exactly this conclusion.
fn is_done_with(v: &Value, conclusion: &str) -> bool {
    text(v, "status") == Some("completed") && text(v, "conclusion") == Some(conclusion)
}

/// `failure`, or the status of a run that did not complete.
fn outcome_word(v: &Value) -> String {
    match text(v, "status") {
        Some("completed") => text(v, "conclusion").unwrap_or("none").to_string(),
        Some(s) => s.to_string(),
        None => "?".to_string(),
    }
}

fn judge(kind: &str, v: &Value, reasons: &mut Vec<String>) {
    let id = v.get("id").and_then(Value::as_u64).unwrap_or(0);
    let name = text(v, "name").unwrap_or("?");
    let status = text(v, "status").unwrap_or("?");
    let conclusion = text(v, "conclusion").unwrap_or("none");
    if status != "completed" {
        reasons.push(format!("{kind} {id} ({name}) is {status}, not completed"));
    } else if conclusion != "success" {
        reasons.push(format!("{kind} {id} ({name}) concluded {conclusion}"));
    }
}

/// Is a [`CiFacts::NotGreen`] reason list "not finished yet" (a run, check or status still
/// queued or running, or a required check without a run) and not "failed"? A list with ONE
/// such reason is pending, even when another item is red: the CI is finished only when every
/// item is completed. The words come from `judge` and `judge_status`.
pub fn not_green_is_pending(reasons: &[String]) -> bool {
    reasons.iter().any(|r| {
        r.ends_with(", not completed")
            || r.contains("has no run for this commit")
            || r.ends_with(" is pending")
    })
}

/// Source names, as they appear in `ci-sources:` of the evidence.
pub const SRC_RUNS: &str = "workflow-runs";
pub const SRC_CHECKS: &str = "check-runs";
pub const SRC_STATUSES: &str = "statuses";
pub const SRC_NAMED: &str = "named-runs";

/// The answers that could be read. `None` = not read (unreadable, or not asked).
#[derive(Debug, Clone, Copy, Default)]
pub struct CiBodies<'a> {
    /// `GET actions/runs?head_sha=` (needs `actions=read`).
    pub runs: Option<&'a Value>,
    /// `GET commits/{sha}/check-runs` (needs `checks=read`; fine-grained tokens cannot get it).
    pub checks: Option<&'a Value>,
    /// `GET commits/{sha}/status` (combined) and `GET commits/{sha}/statuses` (list), both
    /// needed for the source `statuses` (needs `statuses=read`).
    pub statuses: Option<(&'a Value, &'a Value)>,
    /// `GET actions/runs/{id}` of each run named with `--run` (spec/ci#which-ci-runs-count),
    /// already checked with [`check_named_run`].
    pub named: &'a [Value],
    /// `ci_ignore` of AUTHORITY.md on the default branch: job names (check runs, status
    /// contexts) that are not counted, and named in the evidence (spec/ci#jobs-that-ci_ignore-skips).
    pub ignore: &'a [String],
}

/// The latest state of each commit status context (the list is newest first, but the highest
/// id wins whatever the order). `(context, state, target_url)`.
fn latest_statuses(list: &[Value]) -> Vec<(String, String, Option<String>)> {
    let mut best: std::collections::BTreeMap<String, (u64, String, Option<String>)> =
        std::collections::BTreeMap::new();
    for s in list {
        let ctx = text(s, "context").unwrap_or("default").to_string();
        let id = s.get("id").and_then(Value::as_u64).unwrap_or(0);
        let state = text(s, "state").unwrap_or("?").to_string();
        let url = text(s, "target_url").map(str::to_string);
        if best.get(&ctx).is_none_or(|(i, _, _)| id >= *i) {
            best.insert(ctx, (id, state, url));
        }
    }
    best.into_iter().map(|(c, (_, s, u))| (c, s, u)).collect()
}

fn judge_status(ctx: &str, state: &str, reasons: &mut Vec<String>) {
    match state {
        "success" => {}
        "pending" => reasons.push(format!("status {ctx} is pending, not completed")),
        other => reasons.push(format!("status {ctx} is {other}")),
    }
}

/// Judge the CI of `sha` from the sources that could be read.
///
/// - A workflow run counts only if its `head_sha` is `sha` (a run of another commit is
///   ignored, never accepted). Every counted run must be `completed` with conclusion
///   `success`.
/// - Check runs of `sha`: with required contexts each must have a completed check run with
///   conclusion `success` (the latest wins on a rerun), or a `success` commit status of that
///   context; without them every check run is judged, except that `skipped` and `neutral`
///   check runs are ignored (and named in the evidence) next to a successful one
///   (spec/ci#which-ci-runs-count).
/// - Commit statuses (external CI): the latest status of each context must be `success`
///   (`failure`, `error` and `pending` are red). The combined state must agree. GitHub
///   answers `pending` for a commit with no status at all (`total_count` 0): that is "no
///   status", not "running".
/// - A red item in ANY readable source makes it [`CiFacts::NotGreen`], also when another
///   source is unreadable (an unreadable source never turns red into green).
/// - If NO source was read ([`CiBodies`] all `None`): [`CiFacts::NotQueried`] with
///   `unreadable` as the reason. Otherwise, with nothing found in the sources that were
///   read, [`CiFacts::NoCi`]; the caller words the refusal with the unreadable list.
/// - Green carries [`CiSources`]: which sources were seen and which were not.
pub fn evaluate_sources(
    repo: &str,
    sha: &str,
    b: &CiBodies<'_>,
    unreadable: &[(&str, String)],
    required: Option<&[String]>,
) -> Result<CiFacts, GhError> {
    let mut sources = seen_sources(b, unreadable);
    if sources.seen.is_empty() {
        return Ok(CiFacts::NotQueried(not_queried_reason(unreadable)));
    }
    let cand = Candidates::select(b, sha)?;
    sources.counted = cand
        .judged
        .iter()
        .map(|r| describe_run(r, cand.named_only(r)))
        .collect();
    sources.info = cand.info.iter().map(|r| describe_run(r, false)).collect();
    let st = Statuses::read(b, sha)?;
    let required: &[String] = required.unwrap_or(&[]);
    let nothing =
        cand.judged.is_empty() && cand.checks.is_empty() && st.latest.is_empty() && st.total == 0;
    if nothing && required.is_empty() {
        return Ok(if sources.info.is_empty() {
            CiFacts::NoCi(sources.unreadable.clone())
        } else {
            CiFacts::OnlyInformation {
                info: sources.info.clone(),
                unreadable: sources.unreadable.clone(),
            }
        });
    }
    let left = drop_ignored(b.ignore, &cand, st.latest.clone(), required);
    sources.config_ignored = left.ignored.clone();
    if !left.ignored.is_empty()
        && left.judged.is_empty()
        && left.checks.is_empty()
        && left.statuses.is_empty()
        && left.required.is_empty()
    {
        return Ok(CiFacts::NotGreen(vec![format!(
            "every CI job is ignored by AUTHORITY.md ci_ignore ({}); at least one counted job must be green",
            left.ignored.join(", ")
        )]));
    }
    let mut reasons: Vec<String> = Vec::new();
    for r in &left.judged {
        let kind = if cand.named_only(r) {
            "named run"
        } else {
            "run"
        };
        judge(kind, r, &mut reasons);
    }
    if left.required.is_empty() {
        judge_unrequired(&left, &st, &mut sources, &mut reasons);
    } else {
        judge_required(&left, &sources.unreadable, &mut reasons);
    }
    if !reasons.is_empty() {
        if !sources.info.is_empty() {
            reasons.push(format!(
                "information only, not counted: {}",
                sources.info.join(", ")
            ));
        }
        return Ok(CiFacts::NotGreen(reasons));
    }
    let run_url = green_url(repo, sha, &left)?;
    Ok(CiFacts::Green { run_url, sources })
}

/// Which of the four sources were read, and which were not.
fn seen_sources(b: &CiBodies<'_>, unreadable: &[(&str, String)]) -> CiSources {
    let mut sources = CiSources::default();
    if b.runs.is_some() {
        sources.seen.push(SRC_RUNS.into());
    }
    if b.checks.is_some() {
        sources.seen.push(SRC_CHECKS.into());
    }
    if b.statuses.is_some() {
        sources.seen.push(SRC_STATUSES.into());
    }
    if !b.named.is_empty() {
        sources.seen.push(SRC_NAMED.into());
    }
    sources.unreadable = unreadable.iter().map(|(n, _)| (*n).to_string()).collect();
    sources
}

/// The reason of `NotQueried`: why each source could not be read.
fn not_queried_reason(unreadable: &[(&str, String)]) -> String {
    let why: Vec<String> = unreadable
        .iter()
        .map(|(n, w)| format!("{n}: {w}"))
        .collect();
    if why.is_empty() {
        "no CI source was read".to_string()
    } else {
        why.join("; ")
    }
}

/// The workflow runs and check runs of the commit, sorted by what they count for.
struct Candidates<'a> {
    /// The runs that count (spec/ci#which-ci-runs-count): push and pull_request runs and the
    /// named runs, also a named run that the listing lacks.
    judged: Vec<&'a Value>,
    /// The runs of other events: information only.
    info: Vec<&'a Value>,
    /// The runs named with `--run` or by the evidence.
    named: Vec<&'a Value>,
    /// The check runs of the commit, without those of an information run.
    checks: Vec<&'a Value>,
}

impl<'a> Candidates<'a> {
    /// A run of the commit counts only if its `head_sha` is `sha`; a named run is judged once,
    /// also when the listing has it.
    fn select(b: &'a CiBodies<'_>, sha: &str) -> Result<Candidates<'a>, GhError> {
        let own = |v: &&Value| text(v, "head_sha").is_some_and(|h| h.eq_ignore_ascii_case(sha));
        let all_runs: Vec<&Value> = match b.runs {
            Some(r) => list(r, "workflow_runs", "workflow runs")?
                .iter()
                .filter(own)
                .collect(),
            None => Vec::new(),
        };
        let named: Vec<&Value> = b.named.iter().collect();
        for n in &named {
            if !own(n) {
                return Err(shape(format!(
                    "named run {} is not a run of {sha}",
                    run_id(n)
                )));
            }
        }
        let is_named = |r: &Value| named.iter().any(|n| run_id(n) == run_id(r));
        let runs: Vec<&Value> = all_runs
            .iter()
            .copied()
            .filter(|r| counts(r) || is_named(r))
            .collect();
        let info: Vec<&Value> = all_runs
            .iter()
            .copied()
            .filter(|r| !counts(r) && !is_named(r))
            .collect();
        let extra_named: Vec<&Value> = named
            .iter()
            .copied()
            .filter(|n| !all_runs.iter().any(|r| run_id(r) == run_id(n)))
            .collect();
        let judged: Vec<&Value> = runs.iter().copied().chain(extra_named).collect();
        // The jobs of an information run show up as check runs too: they are information as well.
        let info_suites: Vec<u64> = info
            .iter()
            .filter_map(|r| r.get("check_suite_id").and_then(Value::as_u64))
            .collect();
        let checks: Vec<&Value> = match b.checks {
            Some(c) => list(c, "check_runs", "check runs")?
                .iter()
                .filter(own)
                .filter(|c| {
                    c.pointer("/check_suite/id")
                        .and_then(Value::as_u64)
                        .is_none_or(|s| !info_suites.contains(&s))
                })
                .collect(),
            None => Vec::new(),
        };
        Ok(Candidates {
            judged,
            info,
            named,
            checks,
        })
    }

    /// A run that counts only because it was named.
    fn named_only(&self, r: &Value) -> bool {
        self.named.iter().any(|n| run_id(n) == run_id(r)) && !counts(r)
    }
}

/// The commit statuses: the latest state per context, and the combined answer that must agree.
struct Statuses {
    /// `(context, state, target_url)`.
    latest: Vec<(String, String, Option<String>)>,
    total: u64,
    combined: Option<String>,
}

impl Statuses {
    fn read(b: &CiBodies<'_>, sha: &str) -> Result<Statuses, GhError> {
        let Some((combined, listing)) = b.statuses else {
            return Ok(Statuses {
                latest: Vec::new(),
                total: 0,
                combined: None,
            });
        };
        if let Some(s) = text(combined, "sha")
            && !s.eq_ignore_ascii_case(sha)
        {
            return Err(shape(format!(
                "combined status answer is for {s}, not for {sha}"
            )));
        }
        let total = combined
            .get("total_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| shape("combined status answer has no `total_count`"))?;
        let state = text(combined, "state")
            .ok_or_else(|| shape("combined status answer has no `state`"))?
            .to_string();
        let arr = listing.as_array().ok_or_else(|| {
            shape(format!(
                "statuses answer is not a list ({})",
                listing.to_string().chars().take(60).collect::<String>()
            ))
        })?;
        Ok(Statuses {
            latest: latest_statuses(arr),
            total,
            combined: Some(state),
        })
    }
}

/// What is left to judge after `ci_ignore` (spec/ci#jobs-that-ci_ignore-skips).
struct Remaining<'a> {
    judged: Vec<&'a Value>,
    checks: Vec<&'a Value>,
    statuses: Vec<(String, String, Option<String>)>,
    required: Vec<String>,
    /// The jobs of the team's list, named in the evidence (`name = outcome`).
    ignored: Vec<String>,
    /// An ignored context had a commit status.
    ignored_status: bool,
}

/// The jobs of the `ci_ignore` list are named and not judged. A workflow run with such a job is
/// judged by its other jobs (its own conclusion holds the ignored job). What is left must hold
/// a counted job.
fn drop_ignored<'a>(
    ignore: &[String],
    cand: &Candidates<'a>,
    statuses: Vec<(String, String, Option<String>)>,
    required: &[String],
) -> Remaining<'a> {
    let mut ignored: Vec<String> = Vec::new();
    let mut covered_suites: Vec<u64> = Vec::new();
    let checks: Vec<&Value> = cand
        .checks
        .iter()
        .copied()
        .filter(|c| {
            let name = text(c, "name").unwrap_or("?");
            if !crate::authority::ci_ignored(ignore, name) {
                return true;
            }
            ignored.push(format!("{name} = {}", outcome_word(c)));
            if let Some(id) = c.pointer("/check_suite/id").and_then(Value::as_u64) {
                covered_suites.push(id);
            }
            false
        })
        .collect();
    let judged: Vec<&Value> = cand
        .judged
        .iter()
        .copied()
        .filter(|r| {
            r.get("check_suite_id")
                .and_then(Value::as_u64)
                .is_none_or(|s| !covered_suites.contains(&s))
        })
        .collect();
    let statuses_before = ignored.len();
    let statuses: Vec<(String, String, Option<String>)> = statuses
        .into_iter()
        .filter(|(ctx, state, _)| {
            if crate::authority::ci_ignored(ignore, ctx) {
                ignored.push(format!("{ctx} = {state}"));
                false
            } else {
                true
            }
        })
        .collect();
    let required: Vec<String> = required
        .iter()
        .filter(|r| {
            let hit = crate::authority::ci_ignored(ignore, r);
            if hit && !ignored.iter().any(|i| i.starts_with(&format!("{r} = "))) {
                ignored.push(format!("{r} = not run"));
            }
            !hit
        })
        .cloned()
        .collect();
    // Only the statuses count here: a required context that is ignored is named, but its status
    // (if any) is already out of `statuses`.
    let ignored_status = ignored[statuses_before..]
        .iter()
        .any(|i| !i.ends_with("= not run"));
    Remaining {
        judged,
        checks,
        statuses,
        required,
        ignored,
        ignored_status,
    }
}

/// Without required contexts every check run and status is judged
/// (spec/ci#which-ci-runs-count). A skipped or neutral check run next to a successful one is
/// ignored, unless another check run failed or is pending (that run is judged and makes it red).
/// Without any success the skipped runs stay judged: nothing ran.
fn judge_unrequired(
    left: &Remaining<'_>,
    st: &Statuses,
    sources: &mut CiSources,
    reasons: &mut Vec<String>,
) {
    let any_success = left.checks.iter().any(|c| is_done_with(c, "success"));
    for c in &left.checks {
        if any_success && (is_done_with(c, "skipped") || is_done_with(c, "neutral")) {
            let name = text(c, "name").unwrap_or("?");
            let conclusion = text(c, "conclusion").unwrap_or("?");
            sources.ignored.push(format!("{name} ({conclusion})"));
            continue;
        }
        judge("check", c, reasons);
    }
    for (ctx, state, _) in &left.statuses {
        judge_status(ctx, state, reasons);
    }
    // The combined state also holds an ignored context: with one, the list above decides.
    if st.total > 0 && st.combined.as_deref() != Some("success") && !left.ignored_status {
        let s = st.combined.as_deref().unwrap_or("?");
        let line = format!("the combined commit status is {s}");
        if !reasons.iter().any(|r| r.starts_with("status ")) {
            reasons.push(line);
        }
    }
}

/// With required contexts each needs a completed check run with conclusion `success` (the
/// latest wins on a rerun), or a `success` commit status of that context.
fn judge_required(left: &Remaining<'_>, unreadable: &[String], reasons: &mut Vec<String>) {
    for ctx in &left.required {
        let latest_check = left
            .checks
            .iter()
            .filter(|c| text(c, "name") == Some(ctx.as_str()))
            .max_by_key(|c| c.get("id").and_then(Value::as_u64).unwrap_or(0));
        let latest_status = left.statuses.iter().find(|(c, _, _)| c == ctx);
        match (latest_check, latest_status) {
            (Some(c), _) => judge("required check", c, reasons),
            (None, Some((c, s, _))) => judge_status(c, s, reasons),
            (None, None) => reasons.push(format!(
                "required check '{ctx}' has no run for this commit{}",
                if unreadable.is_empty() {
                    String::new()
                } else {
                    format!(" (unreadable: {})", unreadable.join(", "))
                }
            )),
        }
    }
}

/// The page that shows the green CI: the latest run, else a check run, else a status.
fn green_url(repo: &str, sha: &str, left: &Remaining<'_>) -> Result<String, GhError> {
    let latest_run = left
        .judged
        .iter()
        .max_by_key(|r| run_id(r))
        .and_then(|r| text(r, "html_url"));
    let status_url = left.statuses.iter().find_map(|(_, _, u)| u.clone());
    latest_run
        .map(str::to_string)
        .or_else(|| {
            left.checks
                .iter()
                .find_map(|c| text(c, "html_url").map(str::to_string))
        })
        .or(status_url)
        // statuses without a target_url: the commit page shows them
        .or_else(|| {
            (!left.statuses.is_empty()).then(|| format!("https://github.com/{repo}/commit/{sha}"))
        })
        .ok_or_else(|| shape("a green run has no html_url"))
}

/// [`evaluate_sources`] for workflow runs and check runs only (statuses not asked). Only the
/// tests of this file call it.
#[cfg(test)]
pub fn evaluate_ci(
    sha: &str,
    runs_body: &Value,
    checks_body: &Value,
    required: Option<&[String]>,
) -> Result<CiFacts, GhError> {
    evaluate_sources(
        "o/r",
        sha,
        &CiBodies {
            runs: Some(runs_body),
            checks: Some(checks_body),
            statuses: None,
            named: &[],
            ignore: &[],
        },
        &[],
        required,
    )
}

/// The runs that a judgement of evidence counts besides the default ones
/// (spec/ci#which-ci-runs-count): the runs that the `done` message names as `named`
/// in its `ci-runs:` line, and the runs of the command's own `--run`. A run is in one list only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CiRuns {
    /// From the `ci-runs:` line of the evidence. A problem with one of them is a finding about
    /// the evidence (see `fetch_ci` in `services::ci_evidence`).
    pub evidence: Vec<NamedRun>,
    /// From `--run` of the command, without the runs of `evidence`. A problem with one of them
    /// is a refusal of the command.
    pub extra: Vec<NamedRun>,
}

impl CiRuns {
    /// The two lists. `extra` loses each run that `evidence` has already (same repo and id).
    pub fn new(evidence: Vec<NamedRun>, extra: Vec<NamedRun>) -> CiRuns {
        let mut out = CiRuns {
            evidence,
            extra: Vec::new(),
        };
        for r in extra {
            if !out.has(&r) {
                out.extra.push(r);
            }
        }
        out
    }

    fn has(&self, r: &NamedRun) -> bool {
        self.evidence
            .iter()
            .chain(&self.extra)
            .any(|x| x.id == r.id && x.repo.eq_ignore_ascii_case(&r.repo))
    }
}

/// The named runs of the `ci-runs:` text of a `done` message (the part after `ci-runs: `).
/// Only the entries of the `counted:` part whose parentheses are
/// `(<event>, named, <outcome>)`: `counted: 9002 (push, success), 5 (workflow_dispatch, named, success)` gives
/// run 5. An entry is a run id (a run of `pr_repo`) or a run URL. An entry that does not parse,
/// or a URL of a repo outside `initiative`, is left out: it never counts. Pure; never panics.
pub fn named_in_ci_runs(ci_runs: &str, pr_repo: &str, initiative: &[String]) -> Vec<NamedRun> {
    let mut out: Vec<NamedRun> = Vec::new();
    for token in named_entries(ci_runs) {
        if let Ok(r) = parse_run_ref(token, pr_repo, initiative)
            && !out
                .iter()
                .any(|x| x.id == r.id && x.repo.eq_ignore_ascii_case(&r.repo))
        {
            out.push(r);
        }
    }
    out
}

/// Does the `ci-runs:` text name a run by hand (an entry `(<event>, named, <outcome>)` in its
/// `counted:` part)? Also true for an entry that does not parse as a run. The headless `merge`
/// and `tick` use it (spec/agents#headless-coordinator): such evidence is not CI only.
pub fn names_runs(ci_runs: &str) -> bool {
    !named_entries(ci_runs).is_empty()
}

/// The run texts (id or URL) of the named entries of the `counted:` part. Pure; never panics.
fn named_entries(ci_runs: &str) -> Vec<&str> {
    let Some(counted) = ci_runs
        .split(';')
        .find_map(|part| part.trim().strip_prefix("counted:"))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut rest = counted;
    while let Some(open) = rest.find('(') {
        let Some(len) = rest[open..].find(')') else {
            break;
        };
        let token = rest[..open].trim().trim_start_matches(',').trim();
        let inner = &rest[open + 1..open + len];
        rest = &rest[open + len + 1..];
        // `(<event>, named, <outcome>)`: the form of `describe_run` for a named run
        let words: Vec<&str> = inner.split(',').map(str::trim).collect();
        if words.len() == 3 && words[1] == "named" {
            out.push(token);
        }
    }
    out
}
#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;
