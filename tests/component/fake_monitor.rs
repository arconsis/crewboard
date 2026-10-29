//! `crewboard monitor` end to end against a fake GitHub: silent while there are no events, the
//! events once, and one line for an error. The loop rules are unit tests in `src/ops/monitor.rs`.

use crate::support::cli::{crew, run_ok};

/// `--timeout` is shorter than `--interval`: one `watch` makes one poll and ends at once.
const ONE_POLL: &[&str] = &["--timeout", "5s", "--interval", "10s", "--pause", "0"];

fn monitor(
    fake: &crate::support::board::FakeBoard,
    dir: &std::path::Path,
    extra: &[&str],
) -> crate::support::board::Out {
    let mut args = vec!["monitor"];
    args.extend_from_slice(ONE_POLL);
    args.extend_from_slice(extra);
    fake.run(dir, &args)
}

#[test]
fn monitor_is_silent_without_events_and_prints_the_events_once() {
    let (fake, c, b) = crew();
    // no event: three silent cycles, nothing on stdout, exit 0
    let _ = fake.poll_json(&b);
    let o = monitor(&fake, &b.dir, &["--max-cycles", "3"]);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
    // an assignment is an event: the monitor prints it once and ends
    let t = fake.ticket_issue(
        "IT monitor",
        "Goal: test.\n\n**Gate:** the PR is open.\nCI: none\n",
        &["crew:work"],
    );
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    let o = monitor(&fake, &b.dir, &["--max-cycles", "3"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains(&format!("#{t}")), "{}", o.all());
    assert!(
        o.stdout.ends_with('\n') && !o.stdout.contains("no events"),
        "{}",
        o.all()
    );
    // the cursor moved: the next run is silent again
    let o = monitor(&fake, &b.dir, &["--max-cycles", "1"]);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
}

#[test]
fn monitor_prints_one_line_for_an_error_and_exits_with_its_code() {
    let (fake, _c, b) = crew();
    let nowhere = b.dir.join("no-such-dir");
    let o = monitor(&fake, &nowhere, &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert_eq!(o.stdout.lines().count(), 1, "{}", o.all());
    assert!(
        o.stdout.starts_with("crewboard-watch: exit 4: "),
        "{}",
        o.all()
    );
    // the JSON form is one object
    let o = monitor(&fake, &nowhere, &["--json"]);
    assert_eq!(o.code, 4, "{}", o.all());
    let v: serde_json::Value =
        serde_json::from_str(o.stdout.trim()).unwrap_or_else(|_| panic!("{}", o.all()));
    assert_eq!(
        (v["monitor"].as_str(), v["code"].as_u64()),
        (Some("exit"), Some(4))
    );
    // a value that does not parse is exit 4 before any request
    let o = fake.run(&b.dir, &["monitor", "--timeout", "soon"]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stdout.starts_with("crewboard-watch: exit 4: --timeout"),
        "{}",
        o.all()
    );
}

#[test]
fn arguments_after_the_dashes_reach_watch() {
    let (fake, _c, b) = crew();
    // `--json` after `--` makes `watch` print JSON; a bad one is a configuration error
    let o = monitor(&fake, &b.dir, &["--max-cycles", "1", "--", "--bogus"]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stdout.contains("watch arguments"), "{}", o.all());
}

#[test]
fn a_bad_command_line_prints_the_one_line_on_stdout_and_exits_4() {
    let (fake, _c, b) = crew();
    for args in [
        &["monitor", "--max-cycles", "abc"][..],
        &["monitor", "--no-such-flag"],
    ] {
        let o = fake.run(&b.dir, args);
        assert_eq!(o.code, 4, "{}", o.all());
        assert_eq!(o.stdout.lines().count(), 1, "{}", o.all());
        assert!(
            o.stdout.starts_with("crewboard-watch: exit 4: error: "),
            "{}",
            o.all()
        );
        // the JSON form, when --json is among the arguments
        let mut j = args.to_vec();
        j.push("--json");
        let o = fake.run(&b.dir, &j);
        assert_eq!(o.code, 4, "{}", o.all());
        let v: serde_json::Value =
            serde_json::from_str(o.stdout.trim()).unwrap_or_else(|_| panic!("{}", o.all()));
        assert_eq!(
            (v["monitor"].as_str(), v["code"].as_u64()),
            (Some("exit"), Some(4))
        );
    }
    // the twin: --help is still help, with exit 0
    let o = fake.run(&b.dir, &["monitor", "--help"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("--max-cycles"), "{}", o.all());
}

#[test]
fn the_retry_notice_of_watch_goes_to_the_log_and_not_to_stderr() {
    let (fake, _c, b) = crew();
    let _ = fake.poll_json(&b);
    // three failures of one read: the client gives up (exit 3 inside watch), watch retries once
    for _ in 0..3 {
        fake.fail_once("GET", "comments", "");
    }
    let log = b.dir.join("monitor.log");
    let log_arg = log.to_string_lossy().into_owned();
    let o = monitor(&fake, &b.dir, &["--max-cycles", "1", "--log", &log_arg]);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
    assert!(
        !o.stderr.contains("crewboard: transient error ("),
        "{}",
        o.all()
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("crewboard: transient error ("), "{text}");
    // the plain `watch` keeps the notice on stderr
    for _ in 0..3 {
        fake.fail_once("GET", "comments", "");
    }
    let o = fake.run(&b.dir, &["watch", "--timeout", "5s", "--interval", "10s"]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(
        o.stderr.contains("crewboard: transient error ("),
        "{}",
        o.all()
    );
}
