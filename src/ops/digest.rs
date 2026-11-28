//! The daily digest of the headless coordinator (spec/agents#headless-coordinator, 8a): one
//! project status update per day, with facts that the tick has read already. No model writes
//! it, and the health value follows a fixed rule.
//!
//! This file is the pure part: the facts, the rule, the text, and "is one due today".

use std::collections::BTreeMap;

/// What the tick knows at the end of its run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// The tickets on the board by their Status, for each Status with a ticket.
    pub by_status: BTreeMap<String, usize>,
    /// The Review tickets that the tick left for a person, each with the reason of its line.
    pub left: Vec<(String, String)>,
    /// The agents that are late and those that are dead (names).
    pub late: Vec<String>,
    pub dead: Vec<String>,
    /// The part about the unattended coordinator (spec/agents#unattended-coordinator). `None`:
    /// the text has no such part, and is the text without the key.
    pub unattended: Option<Unattended>,
}

/// What the tick knows about the unattended coordinator: the stamped messages of one UTC day
/// (the day before the digest), and the Review tickets of this tick that it verified. Ticket
/// keys and counts only: no text of a message and no text of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unattended {
    /// `unattended_coordinator` of AUTHORITY.md now.
    pub mode: &'static str,
    /// The UTC day of `report` (`YYYY-MM-DD`).
    pub day: String,
    /// `unattended_budget` of AUTHORITY.md now, for a day without a stamped message.
    pub budget: u32,
    /// The stamped messages of `day`, or why they could not be read (one line).
    pub report: Result<crate::unattended::DayReport, String>,
    /// The model wakes of `day`, when the tick runs in a unit that starts a model.
    pub wakes: Option<u32>,
    /// The Review tickets whose latest `done` has a passing verify result of the unattended
    /// coordinator and that are not merged.
    pub waiting: Vec<String>,
}

/// `<n>` and up to [`MAX_LEFT_LINES`] ticket keys: `2 (o/r#5, o/r#7)`; `0` without a key.
fn count_and_keys(keys: &[String]) -> String {
    if keys.is_empty() {
        return "0".to_string();
    }
    let shown: Vec<&str> = keys
        .iter()
        .take(MAX_LEFT_LINES)
        .map(String::as_str)
        .collect();
    let more = keys.len() - shown.len();
    format!(
        "{} ({}{})",
        keys.len(),
        shown.join(", "),
        if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        }
    )
}

/// The lines of the part about the unattended coordinator. Pure.
fn unattended_text(u: &Unattended) -> String {
    let mut s = format!(
        "\nUnattended coordinator (mode {}), messages of {} (UTC):\n",
        u.mode, u.day
    );
    match &u.report {
        Ok(r) => {
            s.push_str(&format!(
                "- verdicts: pass {}; fail {}; budget used {}/{}\n",
                count_and_keys(&r.passed),
                count_and_keys(&r.failed),
                r.used,
                r.budget.unwrap_or(u.budget)
            ));
            s.push_str(&format!(
                "- answers {}; assignments {}; questions to the owner {}\n",
                r.answers, r.assigns, r.asks
            ));
            s.push_str(&format!(
                "- merged after a pass of the unattended coordinator: {}\n",
                count_and_keys(&r.merged)
            ));
        }
        Err(why) => {
            let why: String = why.chars().take(140).collect();
            s.push_str(&format!("- the messages could not be read: {why}\n"));
        }
    }
    if let Some(n) = u.wakes {
        s.push_str(&format!("- model wakes: {n}\n"));
    }
    s.push_str(&format!(
        "Verified by the unattended coordinator, in Review and not merged: {}\n",
        count_and_keys(&u.waiting)
    ));
    s
}

/// The health of the update. `OFF_TRACK`: an agent is dead, so work stands still. `AT_RISK`:
/// an agent is late, or a ticket waits for a person. Else `ON_TRACK`. Pure.
pub fn health(f: &Facts) -> &'static str {
    if !f.dead.is_empty() {
        "OFF_TRACK"
    } else if !f.late.is_empty() || !f.left.is_empty() {
        "AT_RISK"
    } else {
        "ON_TRACK"
    }
}

/// How many lines of the tickets that wait for a person the text lists; the rest is a count.
pub const MAX_LEFT_LINES: usize = 15;

/// The text of the update (without the `from=` line). Pure. A reason is the text of a tick
/// line, written by the tool; it is cut, so one long line cannot fill the update.
pub fn text(f: &Facts, day: &str) -> String {
    let mut s = format!("Daily digest of {day} (written by `crewboard tick`, no model).\n\n");
    let order = [
        "Backlog",
        "Ready",
        "In Progress",
        "Blocked",
        "Review",
        "Done",
    ];
    let counts: Vec<String> = order
        .iter()
        .filter_map(|st| f.by_status.get(*st).map(|n| format!("{st} {n}")))
        .chain(
            f.by_status
                .iter()
                .filter(|(st, _)| !order.contains(&st.as_str()))
                .map(|(st, n)| format!("{st} {n}")),
        )
        .collect();
    s.push_str(&format!(
        "Tickets: {}\n",
        if counts.is_empty() {
            "none".to_string()
        } else {
            counts.join(", ")
        }
    ));
    let names = |v: &[String]| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(", ")
        }
    };
    s.push_str(&format!(
        "Agents: dead {}; late {}\n",
        names(&f.dead),
        names(&f.late)
    ));
    s.push_str(&format!(
        "In Review and waiting for a person: {}\n",
        f.left.len()
    ));
    for (key, why) in f.left.iter().take(MAX_LEFT_LINES) {
        let why: String = why.chars().take(140).collect();
        s.push_str(&format!("- {key}: {why}\n"));
    }
    if f.left.len() > MAX_LEFT_LINES {
        s.push_str(&format!(
            "- and {} more (`crewboard status` lists all)\n",
            f.left.len() - MAX_LEFT_LINES
        ));
    }
    if let Some(u) = &f.unattended {
        s.push_str(&unattended_text(u));
    }
    s
}

/// Is a digest due? One per UTC day: `last` is the day of the last one (`YYYY-MM-DD`). Pure.
pub fn due(last: Option<&str>, today: &str) -> bool {
    last != Some(today)
}

/// The UTC day of a timestamp text (`2026-11-27T08:15:00Z` -> `2026-11-27`).
pub fn day_of(ts: &str) -> &str {
    ts.split('T').next().unwrap_or(ts)
}

/// The tickets that a tick left, from its output: each line `left for the interactive
/// coordinator: <key>, <reason>`. `prefix` is that first part. Pure.
pub fn left_of(tick_text: &str, prefix: &str) -> Vec<(String, String)> {
    tick_text
        .lines()
        .filter_map(|l| l.strip_prefix(prefix)?.strip_prefix(": "))
        .filter_map(|rest| {
            let (key, why) = rest.split_once(", ")?;
            Some((key.to_string(), why.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            by_status: [("Review", 3), ("In Progress", 2), ("Done", 40), ("Odd", 1)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            left: vec![("o/r#7".into(), "the Gate is not CI only".into())],
            late: vec![],
            dead: vec![],
            unattended: None,
        }
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn the_part_of_the_unattended_coordinator_is_facts_only_and_absent_without_it() {
        let plain = text(&facts(), "2026-11-28");
        assert!(!plain.contains("Unattended"), "{plain}");
        let mut report = crate::unattended::DayReport {
            passed: keys(&["o/r#5", "o/r#9"]),
            failed: keys(&["o/r#7"]),
            answers: 1,
            assigns: 2,
            asks: 0,
            merged: keys(&["o/r#5"]),
            used: 3,
            budget: Some(4),
            ..Default::default()
        };
        let mut f = facts();
        f.unattended = Some(Unattended {
            mode: "verify-and-merge",
            day: "2026-11-27".into(),
            budget: 10,
            report: Ok(report.clone()),
            wakes: Some(2),
            waiting: keys(&["o/r#12"]),
        });
        let t = text(&f, "2026-11-28");
        // the text without the key, then the part
        assert_eq!(
            t,
            format!(
                "{plain}\nUnattended coordinator (mode verify-and-merge), messages of 2026-11-27 (UTC):\n- verdicts: pass 2 (o/r#5, o/r#9); fail 1 (o/r#7); budget used 3/4\n- answers 1; assignments 2; questions to the owner 0\n- merged after a pass of the unattended coordinator: 1 (o/r#5)\n- model wakes: 2\nVerified by the unattended coordinator, in Review and not merged: 1 (o/r#12)\n"
            )
        );
        // the part changes no health
        assert_eq!(health(&f), health(&facts()));
        // a day without a message, a tick without a count of wakes, a read that failed
        report = crate::unattended::DayReport::default();
        f.unattended = Some(Unattended {
            mode: "verify",
            day: "2026-11-27".into(),
            budget: 10,
            report: Ok(report),
            wakes: None,
            waiting: vec![],
        });
        let t = text(&f, "2026-11-28");
        assert!(
            t.ends_with("(mode verify), messages of 2026-11-27 (UTC):\n- verdicts: pass 0; fail 0; budget used 0/10\n- answers 0; assignments 0; questions to the owner 0\n- merged after a pass of the unattended coordinator: 0\nVerified by the unattended coordinator, in Review and not merged: 0\n"),
            "{t}"
        );
        f.unattended.as_mut().unwrap().report = Err("HTTP 502".into());
        let t = text(&f, "2026-11-28");
        assert!(
            t.contains("- the messages could not be read: HTTP 502\n"),
            "{t}"
        );
        // a long list of keys is cut
        let many: Vec<String> = (0..20).map(|i| format!("o/r#{i}")).collect();
        assert!(count_and_keys(&many).ends_with("o/r#14, and 5 more)"));
    }

    #[test]
    fn the_health_follows_the_facts() {
        let mut f = Facts::default();
        assert_eq!(health(&f), "ON_TRACK");
        f.left.push(("o/r#1".into(), "x".into()));
        assert_eq!(health(&f), "AT_RISK");
        f.left.clear();
        f.late.push("dev@host-a".into());
        assert_eq!(health(&f), "AT_RISK");
        f.dead.push("dev@host-b".into());
        assert_eq!(health(&f), "OFF_TRACK");
    }

    #[test]
    fn the_text_has_the_counts_the_agents_and_the_tickets_that_wait() {
        let t = text(&facts(), "2026-11-27");
        assert!(t.starts_with("Daily digest of 2026-11-27 "), "{t}");
        assert!(
            t.contains("Tickets: In Progress 2, Review 3, Done 40, Odd 1\n"),
            "{t}"
        );
        assert!(t.contains("Agents: dead none; late none\n"), "{t}");
        assert!(
            t.contains("In Review and waiting for a person: 1\n- o/r#7: the Gate is not CI only\n"),
            "{t}"
        );
        // a long list is cut, and a long reason too
        let mut f = facts();
        f.left = (0..20)
            .map(|i| (format!("o/r#{i}"), "y".repeat(400)))
            .collect();
        let t = text(&f, "d");
        assert!(t.contains("- and 5 more"), "{t}");
        assert!(t.lines().all(|l| l.len() < 200), "{t}");
        assert_eq!(
            text(&Facts::default(), "d").lines().nth(2),
            Some("Tickets: none")
        );
    }

    #[test]
    fn one_digest_per_day_and_the_left_lines_are_read_from_the_tick_text() {
        assert!(due(None, "2026-11-27"));
        assert!(due(Some("2026-11-26"), "2026-11-27"));
        assert!(!due(Some("2026-11-27"), "2026-11-27"));
        assert_eq!(day_of("2026-11-27T08:15:00Z"), "2026-11-27");
        let out = "tick: x\nleft for the interactive coordinator: o/r#7, the Gate is not CI only (signed Gate: 'a, b')\nverify PASS o/r#8\nleft for the interactive coordinator: PVTI_1, no ticket\n";
        assert_eq!(
            left_of(out, "left for the interactive coordinator"),
            [
                (
                    "o/r#7".to_string(),
                    "the Gate is not CI only (signed Gate: 'a, b')".to_string()
                ),
                ("PVTI_1".to_string(), "no ticket".to_string())
            ]
        );
    }
}
