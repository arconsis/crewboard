//! Is the default branch of the main repo protected against a direct push of the crew account
//! (spec/board#authority, docs/guides/security.md)? Pure: the caller reads the answers.
//!
//! The crew account has Write on the repo, and AUTHORITY.md and `.crewboard.toml` are read from
//! the default branch. Without a rule that requires a pull request, an agent can push a change
//! of either file with no review. crewboard cannot enforce the rule, it only reads it and
//! warns. The check never warns on a guess: what the token cannot read is `unknown`.
//!
//! The crew token reads `protected` of `GET repos/{r}/branches/{b}`. It is true when ANY rule
//! applies to the branch, a classic branch protection rule or a ruleset (checked against public
//! repos: a branch with a ruleset only has `protected: true` and `protection.enabled: false`).
//! The pull request requirement itself needs admin rights (`.../protection`, and a 404 there is
//! ambiguous), so a protected branch is [`Guard::Protected`]: not a warning, and not a proof.
//! `protected: false` means nothing applies: [`Guard::Open`].

use serde_json::Value;

/// What the readable answer says about the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guard {
    /// GitHub reports a rule on the branch. The token cannot read whether the rule requires a
    /// pull request.
    Protected,
    /// Nothing protects the branch: a direct push of the crew account is accepted.
    Open,
    /// The answer cannot tell (the reason is the text).
    Unknown(String),
}

/// An HTTP error of a read: status and message.
pub type ReadError = (u16, String);

/// The verdict. `branch`: the answer of `GET repos/{r}/branches/{b}`.
pub fn judge(branch_name: &str, branch: Result<&Value, &ReadError>) -> Guard {
    match branch {
        Ok(v) => match v.get("protected").and_then(Value::as_bool) {
            Some(true) => Guard::Protected,
            Some(false) => Guard::Open,
            None => Guard::Unknown(format!(
                "the answer for branch {branch_name} has no `protected`"
            )),
        },
        Err((s, m)) => Guard::Unknown(format!(
            "branch {branch_name} cannot be read (HTTP {s}: {m})"
        )),
    }
}

/// Does the answer of `GET repos/{r}/rules/branches/{b}` show a rule? The answer lists the
/// active rules of all rulesets that apply to the branch; an empty list means none. A branch
/// whose `protected` is false can still have a ruleset (private repos), so [`Guard::Open`] asks
/// this too. Anything but a non-empty list is "no rule seen".
pub fn rules_protect(rules: &Value) -> bool {
    rules.as_array().is_some_and(|a| !a.is_empty())
}

/// The verdict after the rules read, for a branch whose `protected` is false (`prior` is
/// [`Guard::Open`]; any other prior stands). A rule shows: [`Guard::Protected`]. A read that
/// fails, or an answer that is no list, is [`Guard::Unknown`]: it is not a "no". One failure
/// is a "no": a plan that has no rules at all. GitHub answers 403 "Upgrade to GitHub Pro or
/// make this repository public" for a private repo on the free plan. No rule can apply there.
pub fn judge_rules(prior: Guard, branch_name: &str, rules: Result<&Value, &ReadError>) -> Guard {
    if prior != Guard::Open {
        return prior;
    }
    match rules {
        Ok(v) if v.is_array() => {
            if rules_protect(v) {
                Guard::Protected
            } else {
                Guard::Open
            }
        }
        Ok(_) => Guard::Unknown(format!(
            "the rules of branch {branch_name} have an unexpected answer"
        )),
        Err((403, m))
            if m.contains("Upgrade to GitHub") || m.contains("make this repository public") =>
        {
            Guard::Open
        }
        Err((s, m)) => Guard::Unknown(format!(
            "the rules of branch {branch_name} cannot be read (HTTP {s}: {m})"
        )),
    }
}

/// The fix, as a sentence.
const FIX: &str = "require a pull request for the branch, with owner or CODEOWNERS review for AUTHORITY.md and .crewboard.toml (docs/guides/security.md)";

/// The line that `start` and `doctor --board` print; the warning only for [`Guard::Open`]. The
/// second value is true for a warning.
pub fn line(repo: &str, branch: &str, g: &Guard) -> (String, bool) {
    match g {
        Guard::Protected => (
            format!(
                "default branch: {branch} of {repo} is protected; whether its rule requires a pull request is not readable with this token (check it: {FIX})\n"
            ),
            false,
        ),
        Guard::Open => (
            format!(
                "warning: the default branch {branch} of {repo} is not protected: the crew account can push to it and change AUTHORITY.md or .crewboard.toml without a pull request. Fix: {FIX}\n"
            ),
            true,
        ),
        Guard::Unknown(why) => (
            format!(
                "default branch: protection of {repo} unknown ({why}); check that the branch requires a pull request\n"
            ),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_branch_that_nothing_protects_is_open() {
        let free = json!({"name": "main", "protected": false});
        assert_eq!(judge("main", Ok(&free)), Guard::Open);
    }

    #[test]
    fn a_protected_branch_is_protected_whatever_kind_of_rule_it_has() {
        // a classic rule
        let classic = json!({"name": "main", "protected": true, "protection": {"enabled": true}});
        assert_eq!(judge("main", Ok(&classic)), Guard::Protected);
        // a ruleset only: `protection.enabled` is false, `protected` is true
        let ruleset = json!({"name": "main", "protected": true, "protection": {"enabled": false}});
        assert_eq!(judge("main", Ok(&ruleset)), Guard::Protected);
    }

    #[test]
    fn only_a_non_empty_rule_list_counts_as_a_rule() {
        assert!(rules_protect(&json!([{"type": "pull_request"}])));
        for v in [
            json!([]),
            json!({}),
            json!(null),
            json!({"message": "Not Found"}),
        ] {
            assert!(!rules_protect(&v), "{v}");
        }
    }

    #[test]
    fn what_cannot_be_read_is_unknown_and_never_open() {
        for e in [
            (
                403,
                "Resource not accessible by personal access token".to_string(),
            ),
            (404, "Not Found".to_string()),
            (500, "boom".to_string()),
        ] {
            let Guard::Unknown(why) = judge("main", Err(&e)) else {
                panic!("{e:?} must be unknown");
            };
            assert!(
                why.contains(&format!("branch main cannot be read (HTTP {}", e.0)),
                "{why}"
            );
        }
        // a shape without `protected` (also `null`) is not a "no"
        for v in [
            json!({"name": "main"}),
            json!({"protected": null}),
            json!([]),
        ] {
            assert!(matches!(judge("main", Ok(&v)), Guard::Unknown(_)), "{v}");
        }
    }

    /// The logic before this fix: every failed or odd rules read left the guard open.
    fn old_rules_verdict(prior: Guard, rules: Result<&Value, &ReadError>) -> Guard {
        match rules {
            Ok(v) if prior == Guard::Open && rules_protect(v) => Guard::Protected,
            _ => prior,
        }
    }

    #[test]
    fn a_failed_rules_read_is_unknown_and_the_plan_limit_is_open() {
        let plan = (
            403,
            "Upgrade to GitHub Pro or make this repository public to enable this feature."
                .to_string(),
        );
        assert_eq!(judge_rules(Guard::Open, "main", Err(&plan)), Guard::Open);
        let failing = [
            (403, "Resource not accessible by integration".to_string()),
            (404, "Not Found".to_string()),
            (429, "secondary rate limit".to_string()),
            (500, "boom".to_string()),
            (0, "network".to_string()),
        ];
        for e in &failing {
            let g = judge_rules(Guard::Open, "main", Err(e));
            let Guard::Unknown(why) = g else {
                panic!("{e:?} must be unknown");
            };
            assert!(
                why.contains(&format!(
                    "rules of branch main cannot be read (HTTP {}",
                    e.0
                )),
                "{why}"
            );
            // the old logic said open (a false warning)
            assert_eq!(old_rules_verdict(Guard::Open, Err(e)), Guard::Open);
        }
        let none = json!([]);
        let some = json!([{"type": "pull_request"}]);
        let odd = json!({});
        assert_eq!(judge_rules(Guard::Open, "main", Ok(&none)), Guard::Open);
        assert_eq!(
            judge_rules(Guard::Open, "main", Ok(&some)),
            Guard::Protected
        );
        assert!(matches!(
            judge_rules(Guard::Open, "main", Ok(&odd)),
            Guard::Unknown(_)
        ));
        assert_eq!(old_rules_verdict(Guard::Open, Ok(&odd)), Guard::Open);
        // another prior stands, whatever the rules read says
        let e = (500, "boom".to_string());
        assert_eq!(
            judge_rules(Guard::Protected, "main", Err(&e)),
            Guard::Protected
        );
        let u = Guard::Unknown("x".into());
        assert_eq!(judge_rules(u.clone(), "main", Ok(&some)), u);
    }

    #[test]
    fn only_an_open_branch_gives_a_warning_line() {
        let all = [Guard::Protected, Guard::Open, Guard::Unknown("why".into())];
        let warned: Vec<bool> = all.iter().map(|g| line("o/r", "main", g).1).collect();
        assert_eq!(warned, [false, true, false]);
        let (w, _) = line("o/r", "main", &Guard::Open);
        assert!(w.starts_with("warning: the default branch main of o/r is not protected: "));
        assert!(
            w.contains("pull request") && w.contains("CODEOWNERS"),
            "{w}"
        );
        assert_eq!(w.matches('\n').count(), 1, "one line");
        let (u, _) = line("o/r", "main", &all[2]);
        assert!(u.contains("protection of o/r unknown (why)"), "{u}");
        assert_eq!(line("o/r", "main", &all[0]).0.matches('\n').count(), 1);
    }
}
