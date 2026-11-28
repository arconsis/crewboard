//! The guard of a ticket: path globs that the change of the ticket must not touch
//! (spec/tickets#guarded-paths).
//!
//! A ticket that says "make the test pass" names the test, its fixtures and the check script
//! in `Guard:` lines of its body. The first assignment signs them as `guard:` lines; every
//! later assignment copies the signed lines. `done`, a verify PASS, `close --verified` and
//! `merge` refuse a PR that changes a file that a signed glob covers
//! ([`check_untouched`]). The readers take the signed lines only, never the body: any holder
//! of an agent token can edit the body.
//!
//! It is the per-ticket form of `protected_paths` of AUTHORITY.md, with the same globs and the
//! same matcher ([`crate::rules::protected`]). The difference: a protected path sends the PR to
//! the owner; a guarded path refuses the delivery, and no flag overrides that.
//!
//! Pure: the caller reads the body, the signed lines and the files of the PR.

use super::protected::{MAX_FILES, PrFileList, check_globs, glob_covers};
use super::{Refusal, Verdict, ids};

/// The most globs that one ticket guards.
pub const MAX_GLOBS: usize = 20;

/// The longest glob, in bytes.
pub const MAX_GLOB_BYTES: usize = 200;

/// The most files that the refusal names.
const SHOWN_FILES: usize = 5;

/// The end of the refusal: what the agent and the coordinator can do.
const WAY_OUT: &str = "No flag overrides a guard: the owner changes such a file in a separate change, or the work goes on in a new ticket without that guard";

/// The globs of the `Guard:` lines of a ticket body, in their order, each once. A `Guard:` line
/// without a value gives an empty glob, which [`check`] refuses.
pub fn of_body(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for g in super::text::body_field_lines(body, "Guard") {
        if !out.contains(&g) {
            out.push(g);
        }
    }
    out
}

/// `body` with one `Guard: <glob>` line for each glob of `add` that the body does not have
/// yet (`ticket new --guard`, and the guard that `ticket split` hands on). The lines go to the
/// end, after one empty line.
pub fn add_to_body(body: &str, add: &[String]) -> String {
    let have = of_body(body);
    let mut new: Vec<&str> = Vec::new();
    for g in add.iter().map(|g| g.trim()) {
        if !have.iter().any(|h| h == g) && !new.contains(&g) {
            new.push(g);
        }
    }
    if new.is_empty() {
        return body.to_string();
    }
    let mut out = body.trim_end().to_string();
    out.push_str(if out.is_empty() { "" } else { "\n\n" });
    for g in new {
        out.push_str(&format!("Guard: {g}\n"));
    }
    out
}

fn invalid(why: String) -> Refusal {
    Refusal::new(
        ids::TICKET_GUARD_INVALID,
        format!(
            "{why}. A guard is a relative path glob as in protected_paths of AUTHORITY.md (`*`, `**`, `?`, a trailing `/` for a directory), one per `Guard:` line; at most {MAX_GLOBS} globs of at most {MAX_GLOB_BYTES} bytes. Nothing was written"
        ),
    )
}

/// The limits of a guard: at most [`MAX_GLOBS`] globs, each one line of at most
/// [`MAX_GLOB_BYTES`] bytes, not empty, a relative path glob without a `..` part
/// ([`check_globs`]). So a signed `guard:` line is always one line that the matcher can read.
pub fn check(globs: &[String]) -> Verdict {
    if globs.len() > MAX_GLOBS {
        return Err(invalid(format!(
            "the ticket has {} guard globs",
            globs.len()
        )));
    }
    for g in globs {
        if g.trim().is_empty() {
            return Err(invalid("a guard glob is empty".to_string()));
        }
        if g.chars().any(char::is_control) {
            return Err(invalid(format!(
                "the guard glob '{}' has a line break or another control character",
                printable(g)
            )));
        }
        if g.len() > MAX_GLOB_BYTES {
            return Err(invalid(format!(
                "a guard glob has {} bytes ('{}...')",
                g.len(),
                g.chars().take(40).collect::<String>()
            )));
        }
        check_globs(std::slice::from_ref(g)).map_err(|e| invalid(format!("guard: {e}")))?;
    }
    Ok(())
}

/// A file name or a glob for a refusal text: a control character reads as `?`.
fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// The files among `files` that a glob of `guard` covers, each once and in the order of
/// `files`, with the first glob that covers it.
pub fn hits<'a>(files: &[String], guard: &'a [String]) -> Vec<(String, &'a str)> {
    let mut out: Vec<(String, &'a str)> = Vec::new();
    for f in files {
        if out.iter().any(|(seen, _)| seen == f) {
            continue;
        }
        if let Some(g) = guard.iter().find(|g| glob_covers(g, f)) {
            out.push((f.clone(), g.as_str()));
        }
    }
    out
}

/// The refusal `guarded-path-changed`: the PR `pr_url` of the ticket `ticket` changes a file
/// that a glob of `guard` covers. `guard` is the SIGNED guard of the ticket. `files` holds
/// every path that the PR adds, modifies or deletes, and both names of a renamed file. A file
/// list that is cut proves nothing, so it is refused too. A ticket without a guard passes
/// without a look at the files.
pub fn check_untouched(
    pr_url: &str,
    ticket: &str,
    files: &PrFileList,
    guard: &[String],
) -> Verdict {
    if guard.is_empty() {
        return Ok(());
    }
    if files.truncated {
        return Err(Refusal::new(
            ids::GUARDED_PATH_CHANGED,
            format!(
                "PR {pr_url} has more than {MAX_FILES} files; crewboard cannot prove that none is a guarded path of {ticket} ({}). {WAY_OUT}",
                guard.join(", ")
            ),
        ));
    }
    let hits = hits(&files.all(), guard);
    if hits.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = hits
        .iter()
        .take(SHOWN_FILES)
        .map(|(f, g)| format!("{} (guard: {g})", printable(f)))
        .collect();
    let more = if hits.len() > shown.len() {
        format!(", and {} more", hits.len() - shown.len())
    } else {
        String::new()
    };
    Err(Refusal::new(
        ids::GUARDED_PATH_CHANGED,
        format!(
            "PR {pr_url} changes {}{more}: the signed guard of {ticket} names {}, and the change of this ticket must not touch them (not add, change, delete or rename). Take the change of these files out of the PR. {WAY_OUT}",
            shown.join(", "),
            if hits.len() == 1 {
                "this path"
            } else {
                "these paths"
            }
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn list(names: &[&str], previous: &[&str]) -> PrFileList {
        PrFileList {
            names: v(names),
            previous: v(previous),
            truncated: false,
        }
    }

    const PR: &str = "https://github.com/o/r/pull/3";

    #[test]
    fn the_body_gives_its_guard_lines_in_order_and_once() {
        let body = "Goal: x\nGate: ci\nGuard: tests/**\n- **Guard:** golden/\nguard: tests/**\n";
        assert_eq!(of_body(body), v(&["tests/**", "golden/"]));
        assert!(of_body("Goal: x\nGate: ci\n").is_empty());
        // a line without a value is an empty glob (refused), not a line that is skipped
        assert_eq!(of_body("Guard:\n"), v(&[""]));
        assert!(check(&of_body("Guard:\n")).is_err());
        // the template comment is no guard
        assert!(of_body("<!-- optional: Guard: tests/** -->\n").is_empty());
    }

    #[test]
    fn new_guard_lines_go_to_the_end_of_the_body_and_none_is_written_twice() {
        assert_eq!(
            add_to_body(
                "Goal: x\nGate: ci\n",
                &v(&["tests/**", " ci/check.sh ", "tests/**"])
            ),
            "Goal: x\nGate: ci\n\nGuard: tests/**\nGuard: ci/check.sh\n"
        );
        let has = "Gate: ci\nGuard: tests/**\n";
        assert_eq!(add_to_body(has, &v(&["tests/**"])), has);
        assert_eq!(add_to_body(has, &[]), has);
        assert_eq!(
            add_to_body(has, &v(&["a"])),
            "Gate: ci\nGuard: tests/**\n\nGuard: a\n"
        );
        // what was added is what the body gives back
        assert_eq!(
            of_body(&add_to_body("Gate: ci\n", &v(&["a/**", "b"]))),
            v(&["a/**", "b"])
        );
    }

    #[test]
    fn a_valid_guard_passes_and_each_limit_refuses() {
        assert!(check(&[]).is_ok());
        assert!(check(&v(&["tests/**", "ci/*.sh", "golden/", "Makefile"])).is_ok());
        let twenty: Vec<String> = (0..MAX_GLOBS).map(|i| format!("f{i}")).collect();
        assert!(check(&twenty).is_ok());
        assert!(check(&v(&[&"a".repeat(MAX_GLOB_BYTES)])).is_ok());
        let mut many = twenty.clone();
        many.push("one-more".into());
        let long = "a".repeat(MAX_GLOB_BYTES + 1);
        for (bad, says) in [
            (many, "21 guard globs"),
            (v(&[""]), "is empty"),
            (v(&["  "]), "is empty"),
            (v(&[&long]), "201 bytes"),
            (v(&["/etc/passwd"]), "not a relative path glob"),
            (v(&["a/../b"]), "not a relative path glob"),
            (v(&[".."]), "not a relative path glob"),
            (v(&["a\\b"]), "not a relative path glob"),
            (v(&["a\nguard: b"]), "line break"),
            (v(&["a\rb"]), "line break"),
        ] {
            let r = check(&bad).unwrap_err();
            assert_eq!(r.rule, ids::TICKET_GUARD_INVALID, "{bad:?}");
            assert!(r.message.contains(says), "{bad:?}: {}", r.message);
            assert!(!r.message.contains('\n'), "{}", r.message);
        }
    }

    #[test]
    fn a_pr_that_stays_off_the_guard_passes() {
        let guard = v(&["tests/**", "ci/check.sh"]);
        let files = list(&["src/a.rs", "ci/other.sh", "testsuite/x"], &["src/old.rs"]);
        assert!(check_untouched(PR, "o/r#7", &files, &guard).is_ok());
        // no guard: every PR passes, also one whose list is cut
        let cut = PrFileList {
            truncated: true,
            ..list(&["tests/a.rs"], &[])
        };
        assert!(check_untouched(PR, "o/r#7", &cut, &[]).is_ok());
    }

    #[test]
    fn a_changed_added_deleted_or_renamed_guarded_file_is_refused() {
        let guard = v(&["tests/**", "ci/check.sh"]);
        for files in [
            // added, modified or deleted: the path is in the list
            list(&["src/a.rs", "tests/a.rs"], &[]),
            list(&["CI/Check.sh"], &[]),
            list(&["./tests/deep/b.rs"], &[]),
            // renamed out of the guard: the old name
            list(&["src/moved.rs"], &["tests/a.rs"]),
            // renamed into the guard: the new name
            list(&["tests/new.rs"], &["src/a.rs"]),
        ] {
            let r = check_untouched(PR, "o/r#7", &files, &guard).unwrap_err();
            assert_eq!(r.rule, ids::GUARDED_PATH_CHANGED, "{files:?}");
            assert!(
                r.message.contains("No flag overrides a guard"),
                "{}",
                r.message
            );
            assert!(!r.message.contains("src/a.rs"), "{}", r.message);
        }
        let r = check_untouched(PR, "o/r#7", &list(&["tests/a.rs"], &[]), &guard).unwrap_err();
        assert!(
            r.message.starts_with(
                "PR https://github.com/o/r/pull/3 changes tests/a.rs (guard: tests/**): the signed guard of o/r#7 names this path"
            ),
            "{}",
            r.message
        );
        assert!(
            r.message.ends_with(
                "the owner changes such a file in a separate change, or the work goes on in a new ticket without that guard"
            ),
            "{}",
            r.message
        );
    }

    #[test]
    fn the_refusal_names_five_files_and_counts_the_rest() {
        let names: Vec<String> = (0..8).map(|i| format!("tests/t{i}.rs")).collect();
        let files = PrFileList {
            names,
            ..PrFileList::default()
        };
        let r = check_untouched(PR, "7", &files, &v(&["tests/**"])).unwrap_err();
        assert!(
            r.message
                .contains("tests/t4.rs (guard: tests/**), and 3 more:"),
            "{}",
            r.message
        );
        assert!(!r.message.contains("tests/t5.rs"), "{}", r.message);
        // a control character of a file name is not printed
        let odd = list(&["tests/a\u{1b}[31m.rs"], &[]);
        let r = check_untouched(PR, "7", &odd, &v(&["tests/**"])).unwrap_err();
        assert!(r.message.contains("tests/a?[31m.rs"), "{}", r.message);
    }

    #[test]
    fn a_cut_file_list_is_refused_for_a_ticket_with_a_guard() {
        let cut = PrFileList {
            truncated: true,
            ..list(&["src/a.rs"], &[])
        };
        let r = check_untouched(PR, "o/r#7", &cut, &v(&["tests/**"])).unwrap_err();
        assert_eq!(r.rule, ids::GUARDED_PATH_CHANGED);
        assert!(r.message.contains("more than 1000 files"), "{}", r.message);
    }

    #[test]
    fn each_file_is_named_once_with_the_first_glob_that_covers_it() {
        let guard = v(&["tests/**", "tests/a.rs"]);
        let files = v(&["tests/a.rs", "src/x.rs", "tests/a.rs"]);
        assert_eq!(
            hits(&files, &guard),
            vec![("tests/a.rs".to_string(), "tests/**")]
        );
    }
}
