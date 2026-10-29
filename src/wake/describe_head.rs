//! Where the work of a ticket is, for the message of a wake that ended at its time budget: the
//! branch, the head SHA, whether the head is pushed, and the PR. The checkout is the
//! work dir, or a checkout right under it that is on the ticket branch (a work dir with one clone
//! per repo). The PR is the one that `mine` names for the ticket, else the open PR of the ticket
//! branch. `git` runs as a program; the PR calls go through [`Steps`].

use super::steps::Steps;
use crate::ops::mine::is_ticket_branch;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The first ticket of a `mine` text: the first line with `In Progress`, else the first ticket
/// line. Its first word, when that is `<repo>#<n>`.
pub fn first_ticket(mine: &str) -> Option<String> {
    let first_word = |l: &str| l.split_whitespace().next().map(str::to_string);
    // `<repo>#<n> ...`: a word of non-blanks that ends in `#` and digits, then a blank
    let ticket_line = |l: &str| {
        l.split_once(' ').is_some_and(|(w, _)| {
            w.rsplit_once('#').is_some_and(|(a, n)| {
                !a.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
            })
        })
    };
    let t = mine
        .lines()
        .find(|l| l.contains("In Progress"))
        .and_then(first_word)
        .or_else(|| mine.lines().find(|l| ticket_line(l)).and_then(first_word))?;
    // a `#` followed by a digit
    t.split('#')
        .skip(1)
        .any(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        .then_some(t)
}

/// The PR that `mine` names for `key`: the first `owner/repo#n` of the line
/// `  <key> work|wait: ...` that is not the ticket itself.
pub fn pr_ref(mine: &str, key: &str) -> Option<String> {
    let line = mine.lines().find(|l| {
        let mut w = l.split_whitespace();
        w.next() == Some(key) && matches!(w.next(), Some("work:" | "wait:"))
    })?;
    refs(line).into_iter().find(|r| r != key)
}

/// Every `owner/repo#n` in a text.
fn refs(text: &str) -> Vec<String> {
    let ok = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    let b: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !ok(b[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < b.len() && ok(b[i]) {
            i += 1;
        }
        if i < b.len() && b[i] == '/' {
            let owner_end = i;
            let mut j = i + 1;
            let rs = j;
            while j < b.len() && ok(b[j]) {
                j += 1;
            }
            if j > rs && j < b.len() && b[j] == '#' {
                let mut k = j + 1;
                let ds = k;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > ds && owner_end > s {
                    out.push(b[s..k].iter().collect());
                    i = k;
                    continue;
                }
            }
        }
    }
    out
}

/// The longest a `git` call may take: a hung file system must not keep the wake from ending and
/// from releasing its locks.
const GIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a `git` call gave no answer: it could not run (or hung), or it ran and failed.
#[derive(Debug)]
struct GitErr {
    ran: bool,
    msg: String,
}

impl std::fmt::Display for GitErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, GitErr> {
    git_within(dir, args, GIT_LIMIT)
}

/// `git -C DIR --no-optional-locks ARGS`: a read of the work tree must not take the index lock
/// from the agent's own git calls.
fn git_within(dir: &Path, args: &[&str], limit: std::time::Duration) -> Result<String, GitErr> {
    let o = crate::spawn::output_within(
        Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(dir)
            .args(args),
        limit,
    )
    .map_err(|e| GitErr {
        ran: false,
        msg: e.to_string(),
    })?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        let msg = String::from_utf8_lossy(&o.stderr);
        Err(GitErr {
            ran: true,
            msg: msg.lines().next().unwrap_or("").chars().take(200).collect(),
        })
    }
}

/// The checkouts to look at: the work dir, then each dir right under it.
fn candidates(wd: &Path) -> Vec<PathBuf> {
    let mut v = vec![wd.to_path_buf()];
    let mut subs: Vec<PathBuf> = std::fs::read_dir(wd)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        // a hidden dir (`.cache`, `.git` of a work dir) is no checkout of an agent
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    subs.sort();
    v.extend(subs);
    v
}

/// The git checkouts among the work dir and the dirs right under it that have uncommitted
/// changes (`git status --porcelain` is not empty). Nothing is changed: `git status` reads. A dir
/// that is no checkout of its own (not the top of a work tree) is not counted. When `git` cannot
/// run, or `status` fails in a checkout, `log` gets one line: the result is then "not known", not
/// "clean".
pub fn dirty_checkouts(work_dir: &Path, log: &mut dyn FnMut(&str)) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in candidates(work_dir) {
        let top = match git(&d, &["rev-parse", "--show-toplevel"]) {
            Ok(t) => t,
            Err(e) if !e.ran => {
                log(&format!(
                    "cannot look for uncommitted work in {}: git did not run: {e}",
                    d.display()
                ));
                continue;
            }
            // not a checkout: nothing to look at
            Err(_) => continue,
        };
        let own = matches!(
            (std::fs::canonicalize(&top), std::fs::canonicalize(&d)),
            (Ok(a), Ok(b)) if a == b
        );
        if !own {
            continue;
        }
        match git(&d, &["status", "--porcelain"]) {
            Ok(s) if !s.is_empty() => out.push(d),
            Ok(_) => {}
            Err(e) => log(&format!(
                "cannot look for uncommitted work in {}: git status failed: {e}",
                d.display()
            )),
        }
    }
    out
}

/// The phrase for the message. `log` gets the lines for the wake log. `key` is `<repo>#<n>`.
pub fn describe(
    key: &str,
    mine: &str,
    work_dir: &Path,
    agent: Option<&str>,
    steps: &mut dyn Steps,
    log: &mut dyn FnMut(&str),
) -> String {
    let (repo, n) = key.rsplit_once('#').unwrap_or((key, ""));
    let num: u64 = n.parse().unwrap_or(0);
    let on_ticket = |b: &str| agent.is_some_and(|a| num > 0 && is_ticket_branch(b, a, num));
    let mut cand: Option<PathBuf> = None;
    for d in candidates(work_dir) {
        let Ok(b) = git(&d, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
            continue;
        };
        if on_ticket(&b) {
            cand = Some(d);
            break;
        }
        if cand.is_none() && d == work_dir {
            cand = Some(d);
        }
    }
    let (mut br, mut hd, mut up) = (String::new(), String::new(), String::new());
    match &cand {
        Some(d) => {
            br = git(d, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
            hd = git(d, &["rev-parse", "HEAD"]).unwrap_or_default();
            up = git(d, &["rev-parse", "@{u}"]).unwrap_or_default();
        }
        None => {
            let why = match git(work_dir, &["rev-parse", "HEAD"]) {
                Ok(_) => String::new(),
                Err(e) => e.to_string(),
            };
            log(&format!(
                "no git checkout in the work dir {}: {why}",
                work_dir.display()
            ));
        }
    }
    // the PR that `mine` found, then the open PR of the ticket branch
    let (mut pr, mut prb, mut prh) = (String::new(), String::new(), String::new());
    if let Some(r) = pr_ref(mine, key)
        && let Some((rr, rn)) = r.rsplit_once('#')
        && let Ok(Some((b, h))) = steps.pr_head(rr, rn)
        && !b.is_empty()
        && !h.is_empty()
    {
        (pr, prb, prh) = (r.clone(), b, h);
    }
    if pr.is_empty()
        && agent.is_some()
        && let Ok(list) = steps.open_prs(repo)
        && let Some((pn, b, h)) = list.into_iter().find(|(_, b, _)| on_ticket(b))
    {
        (pr, prb, prh) = (format!("{repo}#{pn}"), b, h);
    }
    let on_branch = !hd.is_empty() && (on_ticket(&br) || (agent.is_none() && br != "HEAD"));
    let wd = work_dir.display();
    if on_branch && (pr.is_empty() || br == prb) {
        if !pr.is_empty() && hd == prh {
            format!("branch {br}, head {hd} (pushed, PR {pr})")
        } else if !pr.is_empty() {
            format!("branch {br}, head {hd} (not pushed; PR {pr} has {prh})")
        } else if hd == up {
            format!("branch {br}, head {hd} (pushed, no PR yet)")
        } else {
            format!("branch {br}, head {hd} (not pushed, no PR yet)")
        }
    } else if !pr.is_empty() {
        format!("branch {prb}, head {prh} (pushed, PR {pr}; the work dir is not on that branch)")
    } else if !hd.is_empty() {
        format!("no branch of the ticket and no PR yet (the work dir is on {br} at {hd})")
    } else {
        format!("no branch and no PR of the ticket found (work dir {wd})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::steps::StepRc;
    use std::collections::HashMap;

    /// A board that knows the heads of some PRs and the open PRs of every repo.
    #[derive(Default)]
    struct Prs {
        heads: HashMap<String, (String, String)>,
        open: Vec<(String, String, String)>,
        calls: Vec<String>,
    }

    impl Steps for Prs {
        fn pr_head(&mut self, repo: &str, n: &str) -> Result<Option<(String, String)>, StepRc> {
            self.calls.push(format!("pr_head {repo}#{n}"));
            Ok(self.heads.get(&format!("{repo}#{n}")).cloned())
        }
        fn open_prs(&mut self, repo: &str) -> Result<Vec<(String, String, String)>, StepRc> {
            self.calls.push(format!("open_prs {repo}"));
            Ok(self.open.clone())
        }
        fn poll(&mut self, _: bool, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn mine_quiet(&mut self) -> StepRc {
            unreachable!()
        }
        fn mine_text(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn say(&mut self, _: &str, _: &str, _: bool) -> StepRc {
            unreachable!()
        }
        fn tick(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_list(&mut self, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn heartbeat(&mut self) -> StepRc {
            unreachable!()
        }
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=t@x",
                "-c",
                "user.name=t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// A repo with one commit on `main`, then a branch.
    fn repo(dir: &Path, branch: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        sh(dir, &["init", "-q"]);
        sh(dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
        sh(dir, &["checkout", "-q", "-b", branch]);
        sh(dir, &["rev-parse", "HEAD"])
    }

    const MINE_PR: &str = "demo/app#12                        In Progress      P1  order 1\nnow: work to do\n  demo/app#12                        work: CI of demo/app#45 is green: deliver with `done`\n";

    fn describe_at(
        wd: &Path,
        mine: &str,
        agent: Option<&str>,
        prs: &mut Prs,
    ) -> (String, Vec<String>) {
        let mut lines = Vec::new();
        let t = describe("demo/app#12", mine, wd, agent, prs, &mut |l| {
            lines.push(l.to_string())
        });
        (t, lines)
    }

    #[test]
    fn first_ticket_prefers_in_progress_and_pr_ref_skips_the_ticket_itself() {
        assert_eq!(first_ticket(MINE_PR).as_deref(), Some("demo/app#12"));
        let two = "demo/app#8   Ready  P1  order 1\ndemo/app#9   In Progress  P1  order 2\n";
        assert_eq!(first_ticket(two).as_deref(), Some("demo/app#9"));
        let ready = "demo/app#8   Ready  P1  order 1\n";
        assert_eq!(first_ticket(ready).as_deref(), Some("demo/app#8"));
        assert_eq!(first_ticket("no owned ticket\n"), None);
        assert_eq!(first_ticket(""), None);
        assert_eq!(
            pr_ref(MINE_PR, "demo/app#12").as_deref(),
            Some("demo/app#45")
        );
        assert_eq!(pr_ref("demo/app#12  work: nothing\n", "demo/app#12"), None);
        assert_eq!(pr_ref(MINE_PR, "demo/app#13"), None);
    }

    #[test]
    fn tb4_a_pushed_ticket_branch_in_a_clone_under_the_work_dir_and_the_pr_of_mine() {
        let d = tempfile::tempdir().unwrap();
        let head = repo(&d.path().join("app"), "crew/dev@host-a/12-fix");
        let mut prs = Prs::default();
        prs.heads.insert(
            "demo/app#45".into(),
            ("crew/dev@host-a/12-fix".into(), head.clone()),
        );
        let (t, _) = describe_at(d.path(), MINE_PR, Some("dev@host-a"), &mut prs);
        assert_eq!(
            t,
            format!("branch crew/dev@host-a/12-fix, head {head} (pushed, PR demo/app#45)")
        );
        assert_eq!(prs.calls, ["pr_head demo/app#45"]);
        // twin: the PR has another head: not pushed
        prs.heads.insert(
            "demo/app#45".into(),
            ("crew/dev@host-a/12-fix".into(), "1111111".into()),
        );
        let (t, _) = describe_at(d.path(), MINE_PR, Some("dev@host-a"), &mut prs);
        assert_eq!(
            t,
            format!(
                "branch crew/dev@host-a/12-fix, head {head} (not pushed; PR demo/app#45 has 1111111)"
            )
        );
    }

    #[test]
    fn tb5_the_open_pr_of_the_ticket_branch_and_not_the_decoy_of_another_agent() {
        let d = tempfile::tempdir().unwrap();
        let nogit = d.path().join("nogit");
        std::fs::create_dir_all(&nogit).unwrap();
        let mut prs = Prs {
            open: vec![
                (
                    "46".into(),
                    "crew/other@host-b/12-fix".into(),
                    "1111111".into(),
                ),
                (
                    "45".into(),
                    "crew/dev@host-a/12-fix".into(),
                    "2222222".into(),
                ),
            ],
            ..Prs::default()
        };
        let mine = "demo/app#12                        In Progress      P1  order 1\n";
        let (t, lines) = describe_at(&nogit, mine, Some("dev@host-a"), &mut prs);
        assert_eq!(
            t,
            "branch crew/dev@host-a/12-fix, head 2222222 (pushed, PR demo/app#45; the work dir is not on that branch)"
        );
        assert!(
            lines.iter().any(|l| l.starts_with(&format!(
                "no git checkout in the work dir {}:",
                nogit.display()
            ))),
            "{lines:?}"
        );
        // without an agent the open PRs are not asked
        let mut prs2 = Prs {
            open: prs.open.clone(),
            ..Prs::default()
        };
        let (t, _) = describe_at(&nogit, mine, None, &mut prs2);
        assert!(prs2.calls.is_empty(), "{:?}", prs2.calls);
        assert_eq!(
            t,
            format!(
                "no branch and no PR of the ticket found (work dir {})",
                nogit.display()
            )
        );
    }

    #[test]
    fn tb6_no_ticket_branch_and_no_pr_says_so() {
        let d = tempfile::tempdir().unwrap();
        let head = repo(d.path(), "agent-branch");
        let mut prs = Prs::default();
        let mine = "demo/app#12                        In Progress      P1  order 1\n";
        let (t, _) = describe_at(d.path(), mine, Some("dev@host-a"), &mut prs);
        assert_eq!(
            t,
            format!(
                "no branch of the ticket and no PR yet (the work dir is on agent-branch at {head})"
            )
        );
        assert!(!t.contains("unknown"));
        // a ticket branch without a PR: pushed or not by its upstream
        let d2 = tempfile::tempdir().unwrap();
        let head2 = repo(d2.path(), "crew/dev@host-a/12-fix");
        let (t, _) = describe_at(d2.path(), mine, Some("dev@host-a"), &mut Prs::default());
        assert_eq!(
            t,
            format!("branch crew/dev@host-a/12-fix, head {head2} (not pushed, no PR yet)")
        );
    }

    #[test]
    fn the_checkout_on_the_ticket_branch_wins_over_the_work_dir() {
        let d = tempfile::tempdir().unwrap();
        repo(d.path(), "main-work");
        let head = repo(&d.path().join("sub"), "crew/dev@host-a/12-fix");
        let (t, _) = describe_at(
            d.path(),
            "demo/app#12  In Progress\n",
            Some("dev@host-a"),
            &mut Prs::default(),
        );
        assert!(
            t.starts_with(&format!("branch crew/dev@host-a/12-fix, head {head}")),
            "{t}"
        );
    }
}
