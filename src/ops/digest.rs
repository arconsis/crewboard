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
        }
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
