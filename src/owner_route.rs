//! The owner route for workflow files (spec/tickets#workflow-files-the-owner-route).
//! Everything here is pure.
//!
//! The crew token has no `workflow` scope, on purpose: a token that can write workflows can
//! add one that reads the secrets of the repository. So a dev agent cannot push a change of
//! `.github/workflows/**`. Such a ticket goes the owner route:
//!
//! 1. The ticket is marked by the body line `Route: owner-workflow`. Nothing else marks it:
//!    a `Scope:` line that names `.github/workflows` may include or exclude them, and text
//!    cannot tell which. (`assign` shows the route; the signed assignment message carries it.)
//! 2. The dev agent pushes everything except the workflow files, and adds the workflow change
//!    to the PR as the patch file `docs/patches/<issue>-workflow.patch`.
//! 3. `done` finds the patch in the PR diff, moves the ticket to Review, sets the label
//!    `needs-owner` and sends one owner message that holds the exact apply command. No `block`
//!    is needed. The owner merges the PR by hand.
//!
//! Why a body line and not a `Needs:` line: `Needs:` names a capability of the agent, and
//! `assign` refuses an agent that lacks it. Every dev agent can write a patch file, so the
//! route is not a capability. Why the diff as safety net: a coordinator can forget the
//! mark. A patch file in the PR is proof enough that the dev agent chose the route.

use crate::base::ids;
use crate::rules::Refusal;
use crate::rules::text::shell_quote;
use crate::ticket::{VMsg, body_fields};
pub const ROUTE_VALUE: &str = "owner-workflow";
pub(crate) const WORKFLOW_DIR: &str = ".github/workflows";

/// Is this path in the workflow directory?
pub fn is_workflow_path(p: &str) -> bool {
    let p = p.trim_start_matches("./");
    p == WORKFLOW_DIR || p.starts_with(".github/workflows/")
}

/// Where the dev agent puts the workflow change in the PR.
pub fn patch_path(issue: u64) -> String {
    format!("docs/patches/{issue}-workflow.patch")
}

/// The ticket body marks the owner route with the line `Route: owner-workflow`. A `Scope:`
/// line never marks it: it may exclude the workflow files.
pub fn declared(body: &str) -> bool {
    body_fields(body, "Route").iter().any(|v| {
        v.trim_end_matches('.')
            .trim()
            .eq_ignore_ascii_case(ROUTE_VALUE)
    })
}

/// The lines for the assignment message and the output of `assign`: empty on the normal
/// route.
pub fn assign_lines(body: &str, issue: u64) -> (String, String) {
    if !declared(body) {
        return (String::new(), String::new());
    }
    let patch = patch_path(issue);
    (
        format!("route: {ROUTE_VALUE}\npatch: {patch}\n"),
        format!(
            "route: {ROUTE_VALUE} (the crew token has no workflow scope); the dev agent delivers the workflow change as {patch}\n"
        ),
    )
}

/// The route of one `done`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Normal,
    Owner { patch: String },
}

/// Decide the route from the ticket mark and the files of the PR.
///
/// - Workflow files in the PR: they are on the branch already (the owner pushed them, or the
///   token had the scope). Normal route.
/// - The patch file in the PR: owner route, marked or not.
/// - Marked, but no patch and no workflow file: refusal. The dev agent has not delivered.
/// - Otherwise normal.
pub fn decide(declared: bool, files: &[String], issue: u64) -> Result<Route, Refusal> {
    let patch = patch_path(issue);
    if files.iter().any(|f| is_workflow_path(f)) {
        return Ok(Route::Normal);
    }
    if files.iter().any(|f| f == &patch) {
        return Ok(Route::Owner { patch });
    }
    if declared {
        return Err(Refusal::new(
            ids::WORKFLOW_NEEDS_OWNER,
            format!(
                "this ticket changes workflow files, and the crew token cannot push them. Put the change in the PR as {patch} \
                 (git diff -- .github/workflows > {patch}), push everything else, then run done again. \
                 Do not block: the owner applies the patch"
            ),
        ));
    }
    Ok(Route::Normal)
}

/// The refusal for an owner-route `done` whose PR closes the ticket.
pub fn closing_keyword_refusal(issue: u64) -> Refusal {
    Refusal::new(
        ids::OWNER_ROUTE_CLOSING_KEYWORD,
        format!(
            "this PR holds the workflow change only as a patch, and its title or body has a closing keyword for #{issue}. \
             GitHub would close the ticket when the owner merges the PR, while the workflow change is not live. \
             Change the PR text to 'Refs #{issue}' and run done again. After the owner applied the patch, 'Closes #{issue}' is right again"
        ),
    )
}

/// The safety net: a merged PR of an owner-route `done` without a workflow file.
pub fn merged_without_workflow(merged: bool, files: &[String]) -> bool {
    merged && !files.iter().any(|f| is_workflow_path(f))
}

/// The workflow files that a patch names (the `+++ b/<path>` lines), in order, without repeats.
/// A patch that names none (or text that is no patch) gives an empty list. The paths are the
/// text of the agent: [`valid_workflow_path`] says which of them may be looked up.
pub fn patch_workflow_files(patch: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in patch.lines() {
        let Some(rest) = l.strip_prefix("+++ b/") else {
            continue;
        };
        let p = rest.split('\t').next().unwrap_or(rest).trim();
        if is_workflow_path(p) && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    out
}

/// May this path of a patch be looked up? Only a plain file path under `.github/workflows/`:
/// the characters `A-Za-z0-9._/-`, no `..`, no empty part, no trailing `/`. Any other path (a
/// `#`, a `?`, a `%`, a space) counts as unreadable: it could change what a request reads.
pub fn valid_workflow_path(p: &str) -> bool {
    let Some(rest) = p.strip_prefix(".github/workflows/") else {
        return false;
    };
    !rest.is_empty()
        && !p.contains("..")
        && p.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'))
        && rest.split('/').all(|part| !part.is_empty())
}

/// One hunk of a patch, as two sides in order. Context lines are on both sides, `-` lines only
/// on the old side, `+` lines only on the new side. Lines are kept without the trailing `\r`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hunk {
    pub old: Vec<String>,
    pub new: Vec<String>,
    /// The hunk has at least one `-` line.
    pub removes: bool,
}

/// The line counts of a hunk header `@@ -a[,b] +c[,d] @@`: `(b, d)`, 1 when a count is left out.
fn hunk_counts(header: &str) -> Option<(usize, usize)> {
    let mut parts = header.strip_prefix("@@ ")?.split(' ');
    let count = |part: Option<&str>, sign: char| -> Option<usize> {
        let range = part?.strip_prefix(sign)?;
        match range.split_once(',') {
            Some((start, n)) => {
                start.parse::<usize>().ok()?;
                n.parse().ok()
            }
            None => range.parse::<usize>().ok().map(|_| 1),
        }
    };
    let old = count(parts.next(), '-')?;
    let new = count(parts.next(), '+')?;
    (parts.next() == Some("@@")).then_some((old, new))
}

/// The hunks that a patch has for `file` (the hunks of its `+++ b/<file>` sections), in order.
/// The line counts of each hunk header say where the hunk ends, so a `+++` or `diff` line inside
/// a hunk is a line of the hunk. `Err(why)`: the patch is malformed (a header that cannot be
/// read, a hunk shorter or longer than its header says, a hunk line outside a hunk). Pure.
pub fn patch_hunks(patch: &str, file: &str) -> Result<Vec<Hunk>, String> {
    let mut out = Vec::new();
    let mut lines = patch.lines().map(|l| l.trim_end_matches('\r'));
    let mut current: Option<&str> = None;
    // after a `+++` line and before the next file: a `+`, `-` or ` ` line belongs to a hunk
    let mut section = false;
    while let Some(l) = lines.next() {
        if let Some(rest) = l.strip_prefix("+++ ") {
            current = rest
                .strip_prefix("b/")
                .map(|r| r.split('\t').next().unwrap_or(r).trim());
            section = true;
            continue;
        }
        if l.starts_with("diff ") || l.starts_with("--- ") {
            current = None;
            section = false;
            continue;
        }
        if !l.starts_with("@@") {
            if section && l.starts_with(['+', '-', ' ']) {
                return Err(format!("the line {l:?} is outside a hunk"));
            }
            continue;
        }
        let (mut old_left, mut new_left) =
            hunk_counts(l).ok_or_else(|| format!("the hunk header {l:?} cannot be read"))?;
        let mut h = Hunk::default();
        while old_left > 0 || new_left > 0 {
            let Some(body) = lines.next() else {
                return Err(format!("the hunk under {l:?} ends before its header says"));
            };
            // An empty line is an empty context line (some tools strip the space).
            let mark = body.chars().next().unwrap_or(' ');
            let text = body.get(1..).unwrap_or("").to_string();
            match mark {
                ' ' if old_left > 0 && new_left > 0 => {
                    old_left -= 1;
                    new_left -= 1;
                    h.old.push(text.clone());
                    h.new.push(text);
                }
                '-' if old_left > 0 => {
                    old_left -= 1;
                    h.removes = true;
                    h.old.push(text);
                }
                '+' if new_left > 0 => {
                    new_left -= 1;
                    h.new.push(text);
                }
                '\\' => {}
                _ => {
                    return Err(format!(
                        "the hunk under {l:?} does not match its line counts"
                    ));
                }
            }
        }
        if current == Some(file) {
            out.push(h);
        }
    }
    Ok(out)
}

/// Is the change of these hunks in `text`? This mirrors `git apply -R --check` and fails
/// closed. For each hunk: its new side (context and added lines, in order) is one block of
/// lines of `text`, and if the hunk removes lines, its old side (context and removed lines, in
/// order) is no block of `text`. So a real patch on the unchanged file is never applied. A
/// removal-only hunk is judged by the same two rules. A hunk with an empty new side has no
/// place in the file, so it is not applied. Trailing white space is ignored. `Err(why)`: not
/// applied. Pure.
pub fn hunks_applied(text: &str, hunks: &[Hunk]) -> Result<(), String> {
    if hunks.is_empty() {
        return Err("the patch has no hunk for the file".into());
    }
    let file: Vec<&str> = text.lines().map(str::trim_end).collect();
    let block = |side: &[String]| {
        (0..file.len()).any(|p| {
            p + side.len() <= file.len()
                && side.iter().zip(&file[p..]).all(|(a, b)| a.trim_end() == *b)
        })
    };
    for (n, h) in hunks.iter().enumerate() {
        let which = format!("hunk {} of {}", n + 1, hunks.len());
        if h.new.is_empty() {
            return Err(format!(
                "{which} keeps no line, so it has no place in the file"
            ));
        }
        if !block(&h.new) {
            return Err(format!("{which} is not in the file"));
        }
        if h.removes && block(&h.old) {
            return Err(format!(
                "the lines that {which} removes are still in the file"
            ));
        }
    }
    Ok(())
}

/// Was the question of the safety net for this PR answered? An owner message or a coordinator
/// answer with `withdraws: question` follows it. Then the finding is settled: the net does not
/// reopen the ticket again, whatever happens to the board item later.
pub fn finding_settled(msgs: &[VMsg], pr_url: &str) -> bool {
    let marker = merged_marker(pr_url);
    msgs.iter()
        .rposition(|x| {
            x.kind == Some(crate::protocol::names::Kind::Question) && x.body.starts_with(&marker)
        })
        .is_some_and(|at| crate::ticket::answered_after(msgs, at))
}

/// The first line of the question that the safety net sends, once per PR.
pub fn merged_marker(pr_url: &str) -> String {
    format!("owner-route-merged: {pr_url}")
}

/// The question of the safety net: the PR was merged with the patch only.
pub fn merged_message(repo: &str, issue: u64, base: &str, pr_url: &str, patch: &str) -> String {
    format!(
        "{}\n\
         The PR {pr_url} of {repo}#{issue} was merged, but it holds the workflow change only as the patch {patch}. The workflow files on {base} are unchanged, so the change is not live. The ticket is open again and carries the label needs-owner.\n\
         Apply the patch and push it to a branch; do NOT treat the merged PR as the delivery. Run this with your own credentials:\n\
         git fetch origin {base_q} && git switch -c owner/{issue}-workflow {origin_base} && git apply {patch} && git add .github/workflows && git commit -m \"Apply the workflow change of #{issue}\" && git push origin owner/{issue}-workflow\n\
         Then open a PR from that branch with 'Closes #{issue}' and answer here. The dev agent resumes.",
        merged_marker(pr_url),
        base_q = shell_quote(base),
        origin_base = shell_quote(&format!("origin/{base}")),
    )
}

/// Was a question with this first line sent already? Looks at the signed messages.
pub fn question_sent(msgs: &[VMsg], marker: &str) -> bool {
    msgs.iter().any(|x| {
        x.kind == Some(crate::protocol::names::Kind::Question) && x.body.starts_with(marker)
    })
}

/// The first line of the owner message. `already_sent` looks for it.
fn marker(patch: &str, sha: &str) -> String {
    format!("owner-route: {patch} sha={sha}")
}

/// The one message to the owner, with the exact apply command. The branch is text that the dev
/// agent controls, so it is quoted for the shell (the patch path is built from the issue number).
pub fn owner_message(repo: &str, issue: u64, branch: &str, patch: &str, sha: &str) -> String {
    format!(
        "{}\n\
         Apply the patch to the branch and push. Do NOT merge the PR as it is: the ticket would close while the workflow change is not live. The ticket is in Review; after your merge the coordinator closes it.\n\
         Ticket {repo}#{issue} changes workflow files. The crew token has no workflow scope, so the change waits as a patch in the PR.\n\
         Apply it (you run this, with your own credentials):\n\
         git fetch origin {branch} && git switch {branch} && git apply {patch} && git add .github/workflows && git commit -m \"Apply the workflow change of #{issue}\" && git push origin {branch}\n\
         Then answer here. The dev agent resumes and proves the Gate on the pushed head.",
        marker(patch, sha),
        branch = shell_quote(branch),
    )
}

/// Was this owner message sent already? Looks at the signed messages of the ticket, so a
/// retry of `done` does not send it twice.
pub fn already_sent(msgs: &[VMsg], patch: &str, sha: &str) -> bool {
    let m = marker(patch, sha);
    msgs.iter()
        .any(|x| x.kind == Some(crate::protocol::names::Kind::Question) && x.body.starts_with(&m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_a_route_line_marks_the_owner_route() {
        // owner route
        assert!(declared("Goal.\nRoute: owner-workflow\nGate: x\n"));
        assert!(declared("**Route:** Owner-Workflow.\n"));
        // normal route: a Scope line never marks, whether it includes or excludes workflows
        assert!(!declared("Scope: src/ci.rs, .github/workflows/ci.yml\n"));
        assert!(!declared(
            "Scope: docs only; out of scope: .github/workflows/\nGate: x\n"
        ));
        // a scope that names the workflows only as out of scope: assign stays on the normal route
        assert_eq!(
            assign_lines("Scope: docs. Out of scope: .github/workflows/\n", 4),
            (String::new(), String::new())
        );
        assert!(!declared("Scope: src/ci.rs\nGate: x\n"));
        assert!(!declared("Route: normal\n"));
        assert!(!declared(
            "The text mentions .github/workflows but has no Scope line.\n"
        ));
    }

    #[test]
    fn a_patch_names_its_workflow_files_once() {
        let p = "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n\
                 --- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1 +1 @@\n\
                 --- a/src/main.rs\n+++ b/src/main.rs\n+++ b/.github/workflows/rel.yml\t2026\n";
        assert_eq!(
            patch_workflow_files(p),
            vec![".github/workflows/ci.yml", ".github/workflows/rel.yml"]
        );
        assert!(patch_workflow_files("not a patch").is_empty());
    }

    #[test]
    fn only_a_plain_workflow_path_may_be_looked_up() {
        for ok in [
            ".github/workflows/ci.yml",
            ".github/workflows/a/b-c_d.1.yaml",
        ] {
            assert!(valid_workflow_path(ok), "{ok}");
        }
        for bad in [
            ".github/workflows",
            ".github/workflows/",
            ".github/workflows/ci.yml#",
            ".github/workflows/ci.yml?x=1",
            ".github/workflows/ci.yml&sha=x",
            ".github/workflows/a b.yml",
            ".github/workflows/%2e%2e/x",
            ".github/workflows/../../etc/x",
            ".github/workflows//ci.yml",
            "./.github/workflows/ci.yml",
            "src/ci.yml",
        ] {
            assert!(!valid_workflow_path(bad), "{bad}");
        }
    }

    /// A patch for `file` with these hunks (each `(header, body)`), and a hunk for `src/a.rs`.
    fn patch(file: &str, hunks: &[(&str, &str)]) -> String {
        let mut p = format!("diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n");
        for (h, b) in hunks {
            p.push_str(&format!("{h}\n{b}"));
        }
        p + "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+other\n"
    }

    const CI: &str = ".github/workflows/ci.yml";

    #[test]
    fn the_hunks_of_one_file_are_its_old_and_new_sides() {
        let p = patch(
            CI,
            &[(
                "@@ -1,2 +1,3 @@",
                "-old\n+new\n ctx\n+second\r\n\\ No newline at end of file\n",
            )],
        );
        let h = patch_hunks(&p, CI).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].old, vec!["old", "ctx"]);
        assert_eq!(h[0].new, vec!["new", "ctx", "second"]);
        assert!(h[0].removes);
        let other = patch_hunks(&p, "src/a.rs").unwrap();
        assert_eq!(other[0].new, vec!["other"]);
        assert!(
            patch_hunks(&p, ".github/workflows/x.yml")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_header_line_inside_a_hunk_is_a_line_of_the_hunk() {
        // the counts say the hunk has three lines; the `+++ b/` line is an added line
        let p = patch(CI, &[("@@ -1 +1,3 @@", " a\n++++ b/x.yml\n+-- a\n")]);
        let h = patch_hunks(&p, CI).unwrap();
        assert_eq!(h[0].new, vec!["a", "+++ b/x.yml", "-- a"]);
        assert!(!h[0].removes);
    }

    #[test]
    fn a_malformed_hunk_is_an_error() {
        for (h, b) in [
            ("@@ -1 +1,2 @@", " a\n"),
            ("@@ -1 +1 @@", " a\n+b\n"),
            ("@@ -x +1 @@", " a\n"),
            ("@@ -1 +1", " a\n"),
            ("@@ -1 +1 @@", "?a\n"),
            ("@@ -1 +1 @@", "\u{e9}a\n"),
        ] {
            assert!(patch_hunks(&patch(CI, &[(h, b)]), CI).is_err(), "{h} {b:?}");
        }
    }

    fn applied(file: &str, hunks: &[(&str, &str)]) -> Result<(), String> {
        hunks_applied(file, &patch_hunks(&patch(CI, hunks), CI).unwrap())
    }

    #[test]
    fn a_patch_of_common_lines_is_not_applied_to_an_unchanged_file() {
        // every added line is somewhere in the file, but not as the block of the hunk
        let before = "on:\n  push:\njobs:\n  a:\n    steps:\n      - uses: actions/checkout@v4\n";
        let hunk = (
            "@@ -3,2 +3,4 @@",
            " jobs:\n+    steps:\n+      - uses: actions/checkout@v4\n   a:\n",
        );
        assert!(applied(before, &[hunk]).is_err());
        let after = "on:\n  push:\njobs:\n    steps:\n      - uses: actions/checkout@v4\n  a:\n";
        assert_eq!(applied(after, &[hunk]), Ok(()));
    }

    #[test]
    fn a_change_is_applied_only_after_it_was_made() {
        let hunk = (
            "@@ -1,3 +1,3 @@",
            " name: ci\n-on: push\n+on: pull_request\n jobs:\n",
        );
        assert!(applied("name: ci\non: push\njobs:\n", &[hunk]).is_err());
        assert_eq!(
            applied("x\nname: ci\non: pull_request  \njobs:\ny\n", &[hunk]),
            Ok(())
        );
    }

    #[test]
    fn a_removal_only_patch_is_applied_after_the_removal_and_not_before() {
        let hunk = ("@@ -1,3 +1,2 @@", " name: ci\n-env: OLD\n jobs:\n");
        assert!(applied("name: ci\nenv: OLD\njobs:\n", &[hunk]).is_err());
        assert_eq!(applied("name: ci\njobs:\n", &[hunk]), Ok(()));
        // the new side is in the file, but so is the old side: the file is unchanged
        let hunk = ("@@ -1,3 +1,2 @@", " a\n-a\n a\n");
        assert!(applied("a\na\na\n", &[hunk]).is_err());
        assert_eq!(applied("a\na\n", &[hunk]), Ok(()));
    }

    #[test]
    fn every_hunk_must_be_in_the_file() {
        let one = ("@@ -1,2 +1,3 @@", " name: ci\n+on: push\n jobs:\n");
        let two = ("@@ -9,2 +10,3 @@", " steps:\n+  - run: make\n end\n");
        let both = "name: ci\non: push\njobs:\nsteps:\n  - run: make\nend\n";
        assert_eq!(applied(both, &[one, two]), Ok(()));
        let e = applied("name: ci\non: push\njobs:\nsteps:\nend\n", &[one, two]).unwrap_err();
        assert!(e.contains("hunk 2 of 2"), "{e}");
    }

    #[test]
    fn a_hunk_without_a_kept_line_or_no_hunk_is_not_applied() {
        let hunk = ("@@ -1 +0,0 @@", "-on: push\n");
        assert!(applied("name: ci\n", &[hunk]).is_err());
        assert!(hunks_applied("name: ci\n", &[]).is_err());
    }

    #[test]
    fn workflow_paths_are_recognised() {
        assert!(is_workflow_path(".github/workflows/ci.yml"));
        assert!(is_workflow_path("./.github/workflows/a/b.yaml"));
        assert!(!is_workflow_path(".github/dependabot.yml"));
        assert!(!is_workflow_path("docs/.github/workflows-notes.md"));
    }

    #[test]
    fn a_marked_ticket_with_the_patch_goes_the_owner_route() {
        let f = files(&["src/a.rs", "docs/patches/16-workflow.patch"]);
        assert_eq!(
            decide(true, &f, 16).unwrap(),
            Route::Owner {
                patch: "docs/patches/16-workflow.patch".into()
            }
        );
    }

    #[test]
    fn an_unmarked_ticket_with_the_patch_goes_the_owner_route_by_the_diff() {
        let f = files(&["docs/patches/16-workflow.patch"]);
        assert!(matches!(decide(false, &f, 16), Ok(Route::Owner { .. })));
    }

    #[test]
    fn a_ticket_without_workflow_files_goes_the_normal_route() {
        assert_eq!(
            decide(false, &files(&["src/a.rs"]), 16).unwrap(),
            Route::Normal
        );
        // a patch of another issue is not this ticket's patch
        assert_eq!(
            decide(false, &files(&["docs/patches/17-workflow.patch"]), 16).unwrap(),
            Route::Normal
        );
    }

    #[test]
    fn workflow_files_already_in_the_pr_go_the_normal_route_even_when_marked() {
        let f = files(&[".github/workflows/ci.yml", "docs/patches/16-workflow.patch"]);
        assert_eq!(decide(true, &f, 16).unwrap(), Route::Normal);
    }

    #[test]
    fn a_marked_ticket_without_the_patch_is_refused_with_the_route_in_the_message() {
        let e = decide(true, &files(&["src/a.rs"]), 16).unwrap_err();
        assert_eq!(e.rule, "workflow-needs-owner");
        let m = e.to_string();
        assert!(m.contains("docs/patches/16-workflow.patch"), "{m}");
        assert!(m.contains("owner applies"), "{m}");
        assert!(m.contains("Do not block"), "{m}");
    }

    #[test]
    fn assign_shows_the_route_only_for_a_marked_ticket() {
        let (msg, out) = assign_lines("Route: owner-workflow\n", 16);
        assert!(msg.contains("route: owner-workflow") && msg.contains("16-workflow.patch"));
        assert!(out.contains("no workflow scope"));
        assert_eq!(
            assign_lines("Scope: src\n", 16),
            (String::new(), String::new())
        );
    }

    #[test]
    fn the_owner_message_holds_the_apply_command() {
        let m = owner_message(
            "a/b",
            16,
            "crew/x@y/16-w",
            "docs/patches/16-workflow.patch",
            "abc123",
        );
        assert!(m.starts_with("owner-route: docs/patches/16-workflow.patch sha=abc123\n"));
        assert!(m.contains("git apply docs/patches/16-workflow.patch"));
        assert!(m.contains("git switch 'crew/x@y/16-w'"), "{m}");
        assert!(m.contains("git commit") && m.contains("git push origin 'crew/x@y/16-w'"));
    }

    #[test]
    fn the_branch_in_the_apply_command_is_one_quoted_word() {
        let m = owner_message("a/b", 16, "crew/x@y/16-w;id", "p.patch", "abc");
        assert!(m.contains("git switch 'crew/x@y/16-w;id' &&"), "{m}");
        let m = owner_message("a/b", 16, "x'; id; '", "p.patch", "abc");
        assert!(m.contains(r"'x'\''; id; '\'''"), "{m}");
    }

    #[test]
    fn the_base_in_the_merged_command_is_quoted() {
        let m = merged_message(
            "a/b",
            16,
            "main;id",
            "https://github.com/a/b/pull/3",
            "p.patch",
        );
        assert!(m.contains("git fetch origin 'main;id' &&"), "{m}");
        assert!(m.contains("'origin/main;id'"), "{m}");
    }

    fn msg(kind: Option<crate::protocol::names::Kind>, body: &str) -> VMsg {
        let dev = crate::testkit::agent("dev@x", crate::rules::Role::Dev);
        crate::testkit::msg(1, dev, "owner", kind, body)
    }

    #[test]
    fn the_owner_message_is_sent_once_per_patch_and_head() {
        use crate::protocol::names::Kind;
        let body = owner_message("a/b", 16, "br", "docs/patches/16-workflow.patch", "abc");
        let sent = vec![msg(Some(Kind::Question), &body)];
        // already sent: a retry sends nothing
        assert!(already_sent(&sent, "docs/patches/16-workflow.patch", "abc"));
        // not sent: nothing there, another head, or the same text from another kind
        assert!(!already_sent(&[], "docs/patches/16-workflow.patch", "abc"));
        assert!(!already_sent(
            &sent,
            "docs/patches/16-workflow.patch",
            "def"
        ));
        assert!(!already_sent(
            &[msg(Some(Kind::Info), &body)],
            "docs/patches/16-workflow.patch",
            "abc"
        ));
    }

    #[test]
    fn the_net_needs_a_merged_pr_without_workflow_files() {
        let patch = files(&["docs/patches/5-workflow.patch", "src/a.rs"]);
        assert!(merged_without_workflow(true, &patch));
        assert!(!merged_without_workflow(false, &patch));
        let applied = files(&["docs/patches/5-workflow.patch", ".github/workflows/ci.yml"]);
        assert!(!merged_without_workflow(true, &applied));
    }

    #[test]
    fn both_owner_questions_say_apply_the_patch_and_do_not_merge_in_their_first_lines() {
        let q = owner_message(
            "a/b",
            5,
            "crew/x/5-w",
            "docs/patches/5-workflow.patch",
            "abc",
        );
        let head: Vec<&str> = q.lines().take(2).collect();
        assert_eq!(
            head[0],
            "owner-route: docs/patches/5-workflow.patch sha=abc"
        );
        assert!(
            head[1].contains("Apply the patch to the branch and push"),
            "{q}"
        );
        assert!(head[1].contains("Do NOT merge the PR as it is"), "{q}");
        let m = merged_message(
            "a/b",
            5,
            "main",
            "https://github.com/a/b/pull/9",
            "docs/patches/5-workflow.patch",
        );
        assert_eq!(
            m.lines().next().unwrap(),
            merged_marker("https://github.com/a/b/pull/9")
        );
        assert!(m.contains("git apply docs/patches/5-workflow.patch"), "{m}");
        assert!(m.contains("do NOT treat the merged PR"), "{m}");
    }
}
