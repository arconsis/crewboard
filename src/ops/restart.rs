//! `resume` for an agent: the command to run after any restart of the session.
//!
//! It lifts a pause (never an owner pause), runs the checks of `start` (a `working` agent stays
//! `working`), and prints a short to-do block for the role plus the command that re-arms the
//! watch loop. A restart kills the watch loop and the CI watchers; this block says what to
//! re-arm and what waited meanwhile.
//!
//! The block builders are pure: they take the data that `run` reads from the board.

use super::lifecycle::{owner_commands, owner_paused};
use super::mine::select_mine;
use super::tickets::{load_world_cached, me_of};
use super::{AgentCtx, load_world};
use crate::config::LoopConfig;
use crate::cursor::Cursor;
use crate::error::CbResult;
use crate::rules::Role;
use crate::rules::evidence::CiFacts;
use crate::rules::fields::Field;
use crate::rules::lifecycle::{State, Who};
use crate::rules::status::Status;
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_index::discover_tickets;
use crate::services::ticket_loader::read_messages;
use crate::ticket::{TicketRow, needs_verify};
use serde_json::Value;

/// First words of the role brief that `start` prints last (see `start::brief`).
const BRIEF_MARK: &str = "role brief for ";

/// An open pull request of a crew branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLine {
    pub repo: String,
    pub number: u64,
    pub branch: String,
    /// `green`, `no run yet`, `not green: ...` or `unreadable`.
    pub ci: String,
}

/// The CI state of a PR head in one short phrase.
pub fn ci_word(f: &CiFacts) -> String {
    match f {
        CiFacts::Green { .. } => "green".into(),
        CiFacts::NoCi(_) => "no run yet".into(),
        // Only runs that do not count (spec/ci#which-ci-runs-count): for the to-do block this is
        // "no CI yet".
        CiFacts::OnlyInformation { .. } => "no counted run yet".into(),
        CiFacts::NotGreen(r) => {
            let t = r.join("; ");
            let t: String = t.chars().take(120).collect();
            format!("not green: {t}")
        }
        CiFacts::NotQueried(_) => "unreadable".into(),
    }
}

/// How the re-arm command finds the token and the config dir of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopEnv<'a> {
    /// The config dir has an `agent.env`: `crewboard --config-dir <dir> monitor` loads it. The
    /// dir is always named.
    AgentDir(&'a str),
    /// Plain `crewboard monitor`: `--config-dir` only when it is not the default, `--env-file`
    /// only when `<config-dir>/<initiative>.env` exists.
    Plain {
        config_dir: Option<&'a str>,
        env_file: Option<&'a str>,
    },
}

/// The command that re-arms the watch loop for the Monitor tool. The arguments come from the
/// `[loop]` table of the agent config; without it the defaults of `watch` apply.
pub fn rearm_command(
    initiative: &str,
    env: LoopEnv<'_>,
    cfg: Option<&LoopConfig>,
    default_log: &str,
) -> String {
    let timeout = cfg
        .and_then(|c| c.timeout.as_deref())
        .unwrap_or(super::watch::DEFAULT_TIMEOUT);
    let log = cfg.and_then(|c| c.log.as_deref()).unwrap_or(default_log);
    let mut s;
    match env {
        LoopEnv::AgentDir(d) => s = format!("crewboard --config-dir {d} monitor"),
        LoopEnv::Plain {
            config_dir,
            env_file,
        } => {
            s = "crewboard monitor".to_string();
            if let Some(e) = env_file {
                s.push_str(&format!(" --env-file {e}"));
            }
            if let Some(d) = config_dir {
                s.push_str(&format!(" --config-dir {d}"));
            }
        }
    }
    s.push_str(&format!(
        " --initiative {initiative} --timeout {timeout} --log {log}"
    ));
    if let Some(i) = cfg.and_then(|c| c.interval.as_deref()) {
        s.push_str(&format!(" -- --interval {i}"));
    }
    s
}

fn list_rows(title: &str, rows: &[&TicketRow], s: &mut String) {
    s.push_str(&format!("{title} ({}):", rows.len()));
    if rows.is_empty() {
        s.push_str(" none\n");
        return;
    }
    s.push('\n');
    for r in rows {
        s.push_str(&format!(
            "  {:<34} {}\n",
            r.key(),
            r.agent.as_deref().unwrap_or("-")
        ));
    }
}

fn with_status(rows: &[TicketRow], status: Status) -> Vec<&TicketRow> {
    let mut v: Vec<&TicketRow> = rows
        .iter()
        .filter(|r| r.status_board() == Some(status))
        .collect();
    v.sort_by(|a, b| (&a.repo, a.issue).cmp(&(&b.repo, b.issue)));
    v
}

fn events_line(events: usize) -> String {
    if events == 0 {
        "events since the cursor: none\n".to_string()
    } else {
        format!("events since the cursor: {events} (the watch loop delivers them; re-arm it now)\n")
    }
}

/// The to-do block of the coordinator. `unverified` are the keys of tickets whose latest
/// `done` has no verify result.
pub fn coordinator_block(
    rows: &[TicketRow],
    unverified: &[String],
    prs: &[PrLine],
    events: usize,
    rearm: &str,
) -> String {
    let mut s = String::from("to-do after the restart (coordinator):\n");
    let mut dones: Vec<&TicketRow> = rows
        .iter()
        .filter(|r| unverified.contains(&r.key()))
        .collect();
    dones.sort_by(|a, b| (&a.repo, a.issue).cmp(&(&b.repo, b.issue)));
    list_rows("done messages without a verify", &dones, &mut s);
    list_rows(
        "tickets in Review",
        &with_status(rows, Status::Review),
        &mut s,
    );
    list_rows(
        "tickets Blocked",
        &with_status(rows, Status::Blocked),
        &mut s,
    );
    s.push_str(&format!("open PRs of crew branches ({}):", prs.len()));
    if prs.is_empty() {
        s.push_str(" none\n");
    } else {
        s.push('\n');
        for p in prs {
            s.push_str(&format!(
                "  {}#{:<5} {:<40} ci: {}\n",
                p.repo, p.number, p.branch, p.ci
            ));
        }
    }
    s.push_str(&events_line(events));
    s.push_str(&format!(
        "re-arm the watch loop (Monitor, persistent):\n  {rearm}\n"
    ));
    s
}

/// The to-do block of a dev agent.
pub fn dev_block(rows: &[TicketRow], me: &str, events: usize, rearm: &str) -> String {
    let mut s = String::from("to-do after the restart (dev):\n");
    let mine = select_mine(rows, me);
    match mine.iter().find(|r| r.in_status(Status::InProgress)) {
        Some(r) => s.push_str(&format!("your In Progress ticket: {}\n", r.key())),
        None => s.push_str("your In Progress ticket: none\n"),
    }
    let ready: Vec<&TicketRow> = mine.iter().filter(|r| r.in_status(Status::Ready)).collect();
    s.push_str(&format!("your Ready queue, next first ({}):", ready.len()));
    if ready.is_empty() {
        s.push_str(" none\n");
    } else {
        s.push('\n');
        for (i, r) in ready.iter().enumerate() {
            s.push_str(&format!(
                "  {}. {:<34} order {}\n",
                i + 1,
                r.key(),
                r.order.map_or("-".to_string(), |o| o.to_string())
            ));
        }
    }
    let waiting: Vec<&TicketRow> = rows
        .iter()
        .filter(|r| r.is_agent(me) && r.status_board() == Some(Status::Blocked))
        .collect();
    s.push_str(&format!(
        "asks and blocks that wait for an answer ({}):",
        waiting.len()
    ));
    if waiting.is_empty() {
        s.push_str(" none\n");
    } else {
        s.push('\n');
        for r in waiting {
            s.push_str(&format!("  {:<34} {}\n", r.key(), Status::Blocked.name()));
        }
    }
    s.push_str(&events_line(events));
    s.push_str(&format!(
        "re-arm the watch loop (Monitor, persistent):\n  {rearm}\n"
    ));
    s
}

/// Open PRs of crew branches (head branch starts with `prefix`) in every repo of the
/// initiative, each with the CI state of its head. A read error becomes `unreadable`: the
/// to-do block must not fail because of one PR.
fn open_crew_prs(ac: &AgentCtx, prefix: &str) -> CbResult<Vec<PrLine>> {
    let mut out = Vec::new();
    for repo in &ac.ctx.cfg.repos {
        let r = ac
            .ctx
            .client
            .get(&format!("repos/{repo}/pulls?state=open&per_page=100"))?;
        let Some(list) = r.body.as_array() else {
            continue;
        };
        for pr in list {
            let s = |p: &str| pr.pointer(p).and_then(Value::as_str);
            let (Some(branch), Some(sha), Some(base), Some(n)) = (
                s("/head/ref"),
                s("/head/sha"),
                s("/base/ref"),
                pr.get("number").and_then(Value::as_u64),
            ) else {
                continue;
            };
            if !branch.starts_with(prefix) || out.len() >= 30 {
                continue;
            }
            let q = CiQuery {
                repo,
                sha,
                base,
                ..CiQuery::default()
            };
            let ci = match fetch_ci(&|p| ac.ctx.client.get(p), &q) {
                Ok((f, _)) => ci_word(&f),
                Err(_) => "unreadable".to_string(),
            };
            out.push(PrLine {
                repo: repo.clone(),
                number: n,
                branch: branch.to_string(),
                ci,
            });
        }
    }
    Ok(out)
}

fn default_config_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(
        std::path::PathBuf::from(home)
            .join(".config")
            .join("crewboard"),
    )
}

/// `resume` of an agent.
pub fn run(ac: &AgentCtx) -> CbResult<String> {
    let mut out = String::new();
    // 1. lift a pause (the owner's pause stays; `check_resume` refuses it)
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    if me.state == Some(State::Paused) {
        let cmds = owner_commands(&world.registry_comments, &ac.ctx.cfg.owner_login);
        let (_, text) = super::lifecycle::resume(
            ac,
            &world,
            me.state,
            Who::SelfAgent,
            owner_paused(&cmds, ac.name.as_str()),
        )?;
        out.push_str(&text);
    }
    // 2. the checks of `start`
    // `start` itself runs: this is a thin layer over it. Its role brief is cut off, because
    // the to-do block below replaces it.
    let started = super::start::run(ac)?;
    let text = started
        .find(BRIEF_MARK)
        .map_or(started.as_str(), |i| &started[..i]);
    out.push_str(text);
    let role = me_of(ac, &load_world_cached(ac)?)?.role;
    // 3. the board, read without moving the cursor
    let mut cache = ac.load_cache();
    let world = load_world(&ac.ctx, std::slice::from_ref(&ac.agent.item_id), &mut cache)?;
    cache.index.drop_foreign(&ac.ctx.cfg.repos);
    if !cache.index.tickets_scanned {
        discover_tickets(&ac.ctx.reader(), &world.trust, &mut cache.index)?;
    }
    let cursor = match world
        .items
        .get(&ac.agent.item_id)
        .and_then(|i| i.get(Field::Cursor))
        .map(Cursor::decode)
    {
        Some(Ok(c)) => c,
        _ => Cursor::at(crate::clock::now_epoch()),
    };
    let mut scratch = cache.clone();
    let collected = super::watch::collect(ac, &world, &cursor, &mut scratch, false)?;
    // tickets and their index are kept; ETags and snapshots of the scratch copy are dropped
    cache.index.tickets = scratch.index.tickets;
    cache.index.tickets_scanned = true;
    ac.save_cache(&mut cache);
    let rows = collected.rows;
    let events = collected.events.len() + collected.security.len();
    // 4. the re-arm command
    let cd = &ac.ctx.cd.0;
    let custom_dir = default_config_dir().is_none_or(|d| &d != cd);
    let env = cd.join(format!("{}.env", ac.ctx.cfg.initiative));
    let cd_s = cd.display().to_string();
    let config_dir = custom_dir.then(|| cd_s.clone());
    let env_file = env.is_file().then(|| env.display().to_string());
    // `exists` follows the link of `agent.env` to the env file.
    let loop_env = if cd.join("agent.env").exists() {
        LoopEnv::AgentDir(&cd_s)
    } else {
        LoopEnv::Plain {
            config_dir: config_dir.as_deref(),
            env_file: env_file.as_deref(),
        }
    };
    let rearm = rearm_command(
        &ac.ctx.cfg.initiative,
        loop_env,
        ac.agent.loop_cfg.as_ref(),
        &cd.join("watch-loop.log").display().to_string(),
    );
    // 5. the block of the role
    match role {
        Role::Coordinator => {
            let mut unverified = Vec::new();
            for r in rows.iter().filter(|r| r.in_status(Status::Review)) {
                let Some(agent) = r.agent.as_deref() else {
                    continue;
                };
                let (_, msgs) = read_messages(&ac.ctx.reader(), &world.trust, &r.repo, r.issue)?;
                if needs_verify(&msgs, agent, r.issue) {
                    unverified.push(r.key());
                }
            }
            let prefix = crate::ctx::load_authority_quiet(&ac.ctx)
                .unwrap_or_default()
                .branch_prefix();
            let prs = open_crew_prs(ac, &prefix)?;
            out.push_str(&coordinator_block(&rows, &unverified, &prs, events, &rearm));
        }
        Role::Dev => out.push_str(&dev_block(&rows, ac.name.as_str(), events, &rearm)),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::row;

    const REARM: &str = "bash loop.sh --initiative t";

    #[test]
    fn the_dev_block_lists_the_ready_queue_in_the_order_of_the_board() {
        // the lowest `Order` is the next ticket, not the lowest issue number
        let mut a = row(1, "Ready", Some("me@x"), None);
        a.order = Some(20);
        let mut b = row(2, "Ready", Some("me@x"), None);
        b.order = Some(10);
        let s = dev_block(&[a, b], "me@x", 0, REARM);
        let first = s.find("a/b#2").expect(&s);
        let second = s.find("a/b#1").expect(&s);
        assert!(first < second, "{s}");
    }

    #[test]
    fn the_coordinator_block_lists_a_done_without_verify_and_not_a_verified_one() {
        let rows = vec![
            row(12, "Review", Some("h@x"), None),
            row(13, "Review", Some("h@x"), None),
        ];
        let b = coordinator_block(&rows, &["a/b#12".to_string()], &[], 0, REARM);
        let done_part = b.split("tickets in Review").next().unwrap();
        assert!(done_part.contains("a/b#12"), "{b}");
        assert!(!done_part.contains("a/b#13"), "{b}");
        assert!(
            done_part.contains("done messages without a verify (1)"),
            "{b}"
        );
        // control: nothing unverified gives an explicit none
        let b = coordinator_block(&rows, &[], &[], 0, REARM);
        assert!(
            b.contains("done messages without a verify (0): none"),
            "{b}"
        );
    }

    #[test]
    fn the_coordinator_block_lists_blocked_and_review_but_not_done() {
        let rows = vec![
            row(1, "Blocked", Some("h@x"), None),
            row(2, "Needs Human", Some("h@x"), None),
            row(3, "Review", Some("h@x"), None),
            row(4, "Done", Some("h@x"), None),
            row(5, "In Progress", Some("h@x"), None),
            row(6, "Ready", Some("h@x"), None),
        ];
        let b = coordinator_block(&rows, &[], &[], 3, REARM);
        for k in ["a/b#1 ", "a/b#2 ", "a/b#3 "] {
            assert!(b.contains(k), "{k} missing: {b}");
        }
        for k in ["a/b#4 ", "a/b#5 ", "a/b#6 "] {
            assert!(!b.contains(k), "{k} must not be listed: {b}");
        }
        assert!(b.contains("events since the cursor: 3"), "{b}");
        assert!(b.contains(REARM), "{b}");
    }

    #[test]
    fn the_coordinator_block_lists_crew_prs_with_their_ci_state() {
        let prs = vec![PrLine {
            repo: "a/b".into(),
            number: 40,
            branch: "crew/h@x/12-foo".into(),
            ci: "not green: build".into(),
        }];
        let b = coordinator_block(&[], &[], &prs, 0, REARM);
        assert!(b.contains("open PRs of crew branches (1):"), "{b}");
        assert!(
            b.contains("a/b#40") && b.contains("ci: not green: build"),
            "{b}"
        );
        let b = coordinator_block(&[], &[], &[], 0, REARM);
        assert!(b.contains("open PRs of crew branches (0): none"), "{b}");
        assert!(b.contains("events since the cursor: none"), "{b}");
    }

    #[test]
    fn the_dev_block_shows_own_tickets_only_and_waiting_asks() {
        let rows = vec![
            row(1, "In Progress", Some("me@x"), None),
            row(2, "Ready", Some("me@x"), None),
            row(3, "Blocked", Some("me@x"), None),
            row(4, "Needs Human", Some("me@x"), None),
            row(5, "Ready", Some("other@x"), None),
            row(6, "Done", Some("me@x"), None),
        ];
        let b = dev_block(&rows, "me@x", 0, REARM);
        assert!(b.contains("your In Progress ticket: a/b#1"), "{b}");
        assert!(b.contains("1. a/b#2"), "{b}");
        assert!(b.contains("(2):"), "waiting asks: {b}");
        assert!(b.contains("a/b#3") && b.contains("a/b#4"), "{b}");
        assert!(!b.contains("a/b#5") && !b.contains("a/b#6"), "{b}");
        // control: an agent without tickets sees none of this
        let b = dev_block(&rows, "nobody@x", 0, REARM);
        assert!(b.contains("In Progress ticket: none"), "{b}");
        assert!(b.contains("(0): none"), "{b}");
    }

    #[test]
    fn the_rearm_command_uses_the_loop_table_and_the_defaults_when_it_is_absent() {
        let plain = LoopEnv::Plain {
            config_dir: None,
            env_file: None,
        };
        let d = rearm_command("demo", plain, None, "/l/w.log");
        assert_eq!(
            d,
            "crewboard monitor --initiative demo --timeout 30m --log /l/w.log"
        );
        let cfg = LoopConfig {
            interval: Some("20s".into()),
            timeout: Some("10m".into()),
            log: Some("/x/mine.log".into()),
        };
        let c = rearm_command(
            "demo",
            LoopEnv::Plain {
                config_dir: Some("/cfg"),
                env_file: Some("/cfg/demo.env"),
            },
            Some(&cfg),
            "/l/w.log",
        );
        assert_eq!(
            c,
            "crewboard monitor --env-file /cfg/demo.env --config-dir /cfg --initiative demo --timeout 10m --log /x/mine.log -- --interval 20s"
        );
        // control: the table changes the command, the absent table does not carry its values
        assert!(!d.contains("--interval") && !d.contains("--env-file"));
    }

    #[test]
    fn the_rearm_command_of_an_agent_dir_names_the_config_dir() {
        // an agent dir with agent.env: the line names the dir, never a token or an env file
        let a = rearm_command("demo", LoopEnv::AgentDir("/a/dev1"), None, "/a/dev1/w.log");
        assert_eq!(
            a,
            "crewboard --config-dir /a/dev1 monitor --initiative demo --timeout 30m --log /a/dev1/w.log"
        );
        assert!(!a.contains("--env-file"), "{a}");
        // control: the plain form puts the explicit config dir after `monitor`
        let p = rearm_command(
            "demo",
            LoopEnv::Plain {
                config_dir: Some("/a/dev1"),
                env_file: None,
            },
            None,
            "/a/dev1/w.log",
        );
        assert!(
            p.starts_with("crewboard monitor --config-dir /a/dev1"),
            "{p}"
        );
    }

    #[test]
    fn ci_words_are_short_and_distinct() {
        assert_eq!(ci_word(&CiFacts::NoCi(vec![])), "no run yet");
        assert_eq!(ci_word(&CiFacts::NotQueried("x".into())), "unreadable");
        assert_eq!(
            ci_word(&CiFacts::OnlyInformation {
                info: vec!["9010 (workflow_dispatch, failure)".into()],
                unreadable: vec![],
            }),
            "no counted run yet"
        );
        assert_eq!(
            ci_word(&CiFacts::NotGreen(vec!["a".into(), "b".into()])),
            "not green: a; b"
        );
    }
}

#[cfg(test)]
mod brief_tests {
    use super::*;

    #[test]
    fn the_brief_mark_is_the_start_of_the_brief_that_start_prints() {
        // if `start::brief` changes its first words, resume would print the brief twice
        let b = super::super::start::brief(Role::Dev, "a@x", "t");
        assert!(b.starts_with(BRIEF_MARK), "{b}");
        let c = super::super::start::brief(Role::Coordinator, "a@x", "t");
        assert!(c.starts_with(BRIEF_MARK), "{c}");
    }
}
