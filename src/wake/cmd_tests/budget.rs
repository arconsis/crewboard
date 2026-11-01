//! The time budget of a wake.

use super::*;

#[test]
fn tb1_a_wake_over_its_budget_is_stopped_the_coordinator_hears_of_it_and_the_run_exits_0() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let wds = wd.to_str().unwrap();
    let mut st = Scripted::new(&["0||", "0||"]).with_mine(&[(0, MINE12)]);
    let t0 = std::time::Instant::now();
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--agent",
            "dev@host-a",
            "--work-dir",
            wds,
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "the sleep was stopped"
    );
    let say = st
        .calls
        .iter()
        .find(|c| c.starts_with("say "))
        .unwrap()
        .clone();
    assert!(
        say.starts_with("say demo/app#12 anyway=true stopped at the time budget (1s) of the wake wrapper. Ticket demo/app#12, "),
        "{say}"
    );
    assert!(
        say.ends_with(". The next wake continues from here."),
        "{say}"
    );
    let log = rig.log();
    assert!(
        log.contains("wake stopped at the time budget of 1s"),
        "{log}"
    );
    assert!(
        log.contains("said on demo/app#12: stopped at the time budget (1s)"),
        "{log}"
    );
    assert!(
        !log.contains("wake finished OK") && !log.contains("wake command failed"),
        "{log}"
    );
    assert!(
        rig.state_text("last-wake").is_some(),
        "a stopped wake starts the back-off too"
    );
    assert!(!rig.lock().exists());
    // twin (TB2): a wake inside its budget ends by itself: no say
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]).with_mine(&[(0, MINE12)]);
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", "exit 0", "--max-wake", "20s"]),
        0
    );
    assert_eq!(st.count("say"), 0);
    assert!(rig.log().contains("wake finished OK"));
}

#[test]
fn tb7_with_an_events_file_the_unread_events_are_read_before_say_and_kept() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&[
        "0||",
        "0|EVENT 1|",
        "0|MESSAGE from coordinator on #12: please rebase|",
        "0||",
    ])
    .with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    let order: Vec<&str> = st
        .calls
        .iter()
        .map(String::as_str)
        .filter(|c| c.starts_with("poll") || c.starts_with("say"))
        .collect();
    assert_eq!(order.len(), 4, "{order:?}");
    assert_eq!(order[..3], ["poll --quiet", "poll", "poll"], "{order:?}");
    assert!(
        order[3].starts_with("say demo/app#12 anyway=false "),
        "no --override-unread with an events file: {order:?}"
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("== Events read at the time budget end (20"),
        "{kept}"
    );
    assert!(kept.contains("please rebase"), "{kept}");
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
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

/// A checkout with one commit; with `dirty` a changed tracked file and a new file.
fn checkout(dir: &Path, dirty: bool) {
    std::fs::create_dir_all(dir).unwrap();
    git_in(dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git_in(dir, &["add", "a.txt"]);
    git_in(dir, &["commit", "-q", "-m", "one"]);
    if dirty {
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
    }
}

/// One wake that hits its budget with an events file, then the next run.
fn budget_twice(rig: &Rig, wd: &Path) -> (String, String, Scripted) {
    let evf = ev(rig);
    let wds = wd.to_str().unwrap();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12|", "0||", "0||"])
        .with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-dir",
            wds,
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    let kept = read(&rig.p("ev.txt.unhandled"));
    let say = st
        .calls
        .iter()
        .find(|c| c.starts_with("say "))
        .cloned()
        .unwrap_or_default();
    // the next run: a quiet board, but the kept events wake the agent again
    let cmd = copy_events(rig, "seen");
    let mut next = Scripted::new(&["1||", "1||"]);
    assert_eq!(
        rig.run(
            &mut next,
            &[
                "--wake-cmd",
                &cmd,
                "--events-file",
                &evf,
                "--work-backoff",
                "off",
                "--max-wake",
                "off"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1, "the next tick wakes the agent again");
    (kept, say, next)
}

#[test]
fn a_wake_at_its_budget_keeps_its_events_and_names_uncommitted_work() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, true);
    let head = git_in(&wd, &["rev-parse", "HEAD"]);
    let porcelain = git_in(&wd, &["status", "--porcelain"]);
    let (kept, say, _) = budget_twice(&rig, &wd);
    let header = format!(
        "The last wake hit the time budget. Uncommitted work in {}: commit or continue it first.",
        wd.display()
    );
    assert_eq!(kept.lines().next(), Some(header.as_str()), "{kept}");
    assert!(
        kept.contains("EVENT 1: assign #12"),
        "the events file of the wake is kept: {kept}"
    );
    assert!(
        say.contains(&format!(
            " Uncommitted work in {}. The next wake continues from here.",
            wd.display()
        )),
        "{say}"
    );
    let seen = read(&rig.p("seen"));
    assert!(
        seen.contains(&header) && seen.contains("EVENT 1: assign #12"),
        "{seen}"
    );
    // the wrapper reads the tree and changes nothing: no commit, no staging
    assert_eq!(git_in(&wd, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_in(&wd, &["status", "--porcelain"]), porcelain);
    assert_eq!(std::fs::read_to_string(wd.join("a.txt")).unwrap(), "two\n");
}

#[test]
fn a_wake_at_its_budget_with_a_clean_tree_keeps_its_events_without_the_header() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, false);
    let (kept, say, _) = budget_twice(&rig, &wd);
    assert!(!kept.contains("Uncommitted work"), "{kept}");
    assert!(kept.contains("EVENT 1: assign #12"), "{kept}");
    assert!(!say.contains("Uncommitted"), "{say}");
    assert!(say.contains("Ticket demo/app#12, "), "{say}");
    // a work dir that is no checkout has no header either
    let rig = Rig::new();
    let plain = rig.p("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let (kept, _, _) = budget_twice(&rig, &plain);
    assert!(!kept.contains("Uncommitted work"), "{kept}");
}

#[test]
fn only_a_checkout_of_its_own_counts_not_a_dir_inside_one() {
    let d = tempfile::tempdir().unwrap();
    checkout(d.path(), true);
    std::fs::create_dir_all(d.path().join("sub")).unwrap();
    let dirty = describe_head::dirty_checkouts(d.path(), &mut |_| {});
    assert_eq!(dirty, [d.path().to_path_buf()]);
    // a clone under a plain work dir
    let parent = tempfile::tempdir().unwrap();
    checkout(&parent.path().join("app"), true);
    checkout(&parent.path().join("lib"), false);
    assert_eq!(
        describe_head::dirty_checkouts(parent.path(), &mut |_| {}),
        [parent.path().join("app")]
    );
    assert!(describe_head::dirty_checkouts(&parent.path().join("nope"), &mut |_| {}).is_empty());
}

/// One run of a wake that ends at its 1 s budget on ticket 12, with an events file.
fn budget_stop(rig: &Rig, plan: &[&str], backoff: &str) -> Scripted {
    let evf = ev(rig);
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let wds = wd.to_string_lossy().to_string();
    let mut st = Scripted::new(plan).with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-dir",
            &wds,
            "--work-backoff",
            backoff,
        ],
    );
    assert_eq!(rc, 0);
    st
}

fn said(st: &Scripted) -> String {
    st.calls
        .iter()
        .find(|c| c.starts_with("say "))
        .cloned()
        .unwrap_or_default()
}

const TWICE: &str = "hit the time budget twice in a row: split the ticket or raise --max-wake";

#[test]
fn the_second_budget_stop_in_a_row_waits_for_the_backoff_and_tells_the_coordinator() {
    let rig = Rig::new();
    // first stop: events kept, no streak line
    let st = budget_stop(
        &rig,
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        "600",
    );
    assert!(!said(&st).contains(TWICE), "{}", said(&st));
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("1 demo/app#12\n")
    );
    // the board is quiet: the kept events wake the agent at once (the back-off is not over)
    let st = budget_stop(&rig, &["1||", "1||", "1||", "0||"], "600");
    assert_eq!(st.count("mine --quiet"), 0, "woken by the kept events");
    let body = said(&st);
    assert!(body.contains(TWICE), "{body}");
    assert!(body.contains("stopped at the time budget (1s)"), "{body}");
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("2 demo/app#12\n")
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("EVENT 1: assign #12"),
        "events are kept: {kept}"
    );
    assert_eq!(kept.matches("The last wake hit the time budget").count(), 0);
    // third run, inside the back-off: a quiet board and no wake, the events stay
    let evf = ev(&rig);
    let mut quiet = Scripted::new(&["1||"]);
    let wake = rig.wake_cmd();
    assert_eq!(
        rig.run(
            &mut quiet,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 0, "no wake inside the back-off");
    assert!(
        rig.p("ev.txt.unhandled").is_file(),
        "the events are still kept"
    );
    // after the back-off the agent wakes, and a wake that ends by itself ends the streak
    rig.clock.advance(601);
    let mut due = Scripted::new(&["1||", "1||"]).with_mine(&[(0, MINE12), (0, MINE12)]);
    assert_eq!(
        rig.run(
            &mut due,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1);
    assert_eq!(rig.state_text("budget-streak"), None);
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the events went to that wake"
    );
}

#[test]
fn a_budget_stop_on_another_ticket_starts_the_count_again() {
    let rig = Rig::new();
    rig.write_state("budget-streak", "1 demo/app#99\n");
    let st = budget_stop(
        &rig,
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        "off",
    );
    assert!(!said(&st).contains(TWICE), "{}", said(&st));
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("1 demo/app#12\n")
    );
}

#[test]
fn new_events_wake_the_agent_even_while_the_streak_holds_the_kept_events() {
    let rig = Rig::new();
    rig.write_state("budget-streak", "2 demo/app#12\n");
    std::fs::write(rig.p("ev.txt.unhandled"), "old kept\n").unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 2: message|"]).with_mine(&[(0, MINE12)]);
    let wake = copy_events(&rig, "seen");
    assert_eq!(
        rig.run(
            &mut st,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1);
    let seen = read(&rig.p("seen"));
    assert!(
        seen.contains("old kept") && seen.contains("EVENT 2: message"),
        "{seen}"
    );
}

#[test]
fn the_header_of_an_earlier_budget_stop_is_dropped_so_the_new_one_is_first_and_alone() {
    let old =
        "The last wake hit the time budget. Uncommitted work in /a: commit or continue it first.";
    let new = "The last wake hit the time budget. Uncommitted work in /a, /b: commit or continue it first.";
    let text = format!(
        "== Events kept from an earlier wake:\n{old}\nEVENT 1\n\n== Events (crewboard poll, t):\nEVENT 2\n"
    );
    let out = String::from_utf8(with_budget_header(text.as_bytes(), Some(new))).unwrap();
    assert!(out.starts_with(&format!("{new}\n== Events kept")), "{out}");
    assert_eq!(
        out.matches("The last wake hit the time budget").count(),
        1,
        "{out}"
    );
    assert!(out.contains("EVENT 1") && out.contains("EVENT 2"));
    // the same header again stays once
    let again = String::from_utf8(with_budget_header(out.as_bytes(), Some(new))).unwrap();
    assert_eq!(again, out);
    // a clean tree drops the stale header and adds none
    let clean = String::from_utf8(with_budget_header(out.as_bytes(), None)).unwrap();
    assert!(!clean.contains("hit the time budget"), "{clean}");
    // bytes that are not UTF-8 pass through
    assert_eq!(with_budget_header(b"a\xffb\n", None), b"a\xffb\n");
}

#[test]
fn the_events_file_of_a_second_stop_keeps_the_header_once_and_first() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, true);
    let evf = ev(&rig);
    let wds = wd.to_str().unwrap();
    for plan in [
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        &["1||", "1||", "1||", "0||"],
    ] {
        let mut st = Scripted::new(plan).with_mine(&[(0, MINE12), (0, MINE12)]);
        let rc = rig.run(
            &mut st,
            &[
                "--wake-cmd",
                "sleep 30",
                "--max-wake",
                "1s",
                "--events-file",
                &evf,
                "--agent",
                "dev@host-a",
                "--work-dir",
                wds,
                "--work-backoff",
                "600",
            ],
        );
        assert_eq!(rc, 0);
    }
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert_eq!(
        kept.matches("The last wake hit the time budget").count(),
        1,
        "{kept}"
    );
    assert!(
        kept.starts_with("The last wake hit the time budget."),
        "{kept}"
    );
    assert!(kept.contains("EVENT 1: assign #12"), "{kept}");
}

#[test]
fn a_checkout_where_git_status_fails_is_a_log_line_not_a_clean_tree() {
    let d = tempfile::tempdir().unwrap();
    checkout(d.path(), false);
    std::fs::write(d.path().join(".git/index"), "not an index").unwrap();
    let mut lines = Vec::new();
    let dirty = describe_head::dirty_checkouts(d.path(), &mut |l| lines.push(l.to_string()));
    assert!(dirty.is_empty());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("git status failed"), "{lines:?}");
    // twin: a sound checkout logs nothing
    let ok = tempfile::tempdir().unwrap();
    checkout(ok.path(), true);
    let mut none = Vec::new();
    assert_eq!(
        describe_head::dirty_checkouts(ok.path(), &mut |l| none.push(l.to_string())).len(),
        1
    );
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn a_hidden_dir_under_the_work_dir_is_no_checkout() {
    let parent = tempfile::tempdir().unwrap();
    checkout(&parent.path().join(".cache"), true);
    checkout(&parent.path().join("app"), true);
    assert_eq!(
        describe_head::dirty_checkouts(parent.path(), &mut |_| {}),
        [parent.path().join("app")]
    );
}
