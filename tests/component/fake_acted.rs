//! The event receipts of `poll-and-wake.sh`: the hidden command `acted` reads an
//! events file and names the events that need an action and that the agent did not act on,
//! against a fake GitHub. It reads one comment list per ticket named in the events, and nothing
//! when no event needs an action.

use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::run_ok;
use crewboard::clock::{format_ts, now_epoch};
use crewboard::ops::acted::REDELIVERED_MARKER;
use crewboard::rules::Role;

fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    fake.ticket_issue(
        title,
        "Goal: integration test ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    )
}

/// One block of the `poll` output.
fn block(n: usize, label: &str, kind: &str, to: &str, issue: u64, ts: i64, extra: &str) -> String {
    format!(
        "== [{n}] {label} ==\nfrom: sender  to: {to}  kind: {kind}  re: none\nwhere: {REPO}#{issue} comment 1\nts: {}\nurl: u\n\n{kind} text\n{extra}\n",
        format_ts(ts)
    )
}

fn count_reads(fake: &FakeBoard, from: usize, issue: u64) -> usize {
    fake.requests()[from..]
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&format!("/issues/{issue}/comments")))
        .count()
}

struct Scene {
    fake: FakeBoard,
    b: Agent,
    dir: tempfile::TempDir,
    t_taken: u64,
    t_asked: u64,
    t_info: u64,
    before: i64,
}

fn scene() -> Scene {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let t_taken = ticket(&fake, "IT acted taken");
    let t_asked = ticket(&fake, "IT acted asked");
    let t_info = ticket(&fake, "IT acted info");
    let before = now_epoch() - 120;
    for t in [t_taken, t_asked, t_info] {
        run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    }
    // the agent acts on the first ticket only
    run_ok(&fake, &b, &["take", &t_taken.to_string()]);
    Scene {
        fake,
        b,
        dir: tempfile::tempdir().unwrap(),
        t_taken,
        t_asked,
        t_info,
        before,
    }
}

fn acted(s: &Scene, file: &std::path::Path, extra: &[&str]) -> Out {
    let mut a = vec!["acted", "--events-file", file.to_str().unwrap()];
    a.extend_from_slice(extra);
    s.fake.run(&s.b.dir, &a)
}

#[test]
fn an_assignment_with_a_later_take_is_acted_and_an_ask_without_a_comment_is_not() {
    let s = scene();
    let text = format!(
        "== Events (crewboard poll, x):\n{}{}{}== Your tickets (crewboard mine):\n",
        block(
            1,
            "TICKET",
            "assignment",
            &s.b.name,
            s.t_taken,
            s.before,
            ""
        ),
        block(2, "MESSAGE", "question", &s.b.name, s.t_asked, s.before, ""),
        block(3, "MESSAGE", "info", &s.b.name, s.t_info, s.before, ""),
    );
    let f = s.dir.path().join("events.txt");
    std::fs::write(&f, &text).unwrap();
    let redo = s.dir.path().join("events.txt.unhandled");
    let from = s.fake.requests().len();
    let o = acted(
        &s,
        &f,
        &[
            "--since",
            &format_ts(s.before),
            "--redeliver-to",
            redo.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        o.stdout,
        format!("{REPO}#{} question again\n", s.t_asked),
        "{}",
        o.all()
    );
    // the read cost: ONE comment list per ticket that needs an action; none for the info
    assert_eq!(count_reads(&s.fake, from, s.t_taken), 1);
    assert_eq!(count_reads(&s.fake, from, s.t_asked), 1);
    assert_eq!(count_reads(&s.fake, from, s.t_info), 0);
    let total = s.fake.requests().len() - from;
    // 6 reads of the cycle, and the read of AUTHORITY.md where the context loads
    assert!(
        total == 7,
        "{total} requests: {:?}",
        s.fake.requests()[from..]
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect::<Vec<_>>()
    );
    // the text for the next wake: the marker and the block of the ask only
    let again = std::fs::read_to_string(&redo).unwrap();
    assert!(again.starts_with(REDELIVERED_MARKER), "{again}");
    assert!(again.contains("kind: question"), "{again}");
    assert!(!again.contains("assignment"), "{again}");
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn an_event_that_was_delivered_again_is_named_twice_and_not_written_again() {
    let s = scene();
    let b = block(1, "MESSAGE", "request", &s.b.name, s.t_asked, s.before, "");
    let f = s.dir.path().join("events.txt");
    std::fs::write(&f, format!("{REDELIVERED_MARKER}\n{b}")).unwrap();
    let redo = s.dir.path().join("events.txt.unhandled");
    let o = acted(&s, &f, &["--redeliver-to", redo.to_str().unwrap()]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(o.stdout, format!("{REPO}#{} request twice\n", s.t_asked));
    assert!(!redo.exists(), "nothing goes to the next wake");
}

#[test]
fn a_message_before_the_wake_started_is_no_act() {
    let s = scene();
    // the take of the first ticket is older than the start of this wake
    let b = block(
        1,
        "TICKET",
        "assignment",
        &s.b.name,
        s.t_taken,
        s.before,
        "",
    );
    let f = s.dir.path().join("events.txt");
    std::fs::write(&f, b).unwrap();
    let later = format_ts(now_epoch() + 3600);
    let o = acted(&s, &f, &["--since", &later]);
    assert_eq!(o.stdout, format!("{REPO}#{} assignment again\n", s.t_taken));
}

#[test]
fn no_event_that_needs_an_action_reads_no_ticket() {
    let s = scene();
    let text = format!(
        "{}{}",
        block(1, "MESSAGE", "info", &s.b.name, s.t_info, s.before, ""),
        block(
            2,
            "TICKET",
            "status-changed",
            &s.b.name,
            s.t_info,
            s.before,
            ""
        )
    );
    let f = s.dir.path().join("events.txt");
    std::fs::write(&f, text).unwrap();
    let from = s.fake.requests().len();
    // `AgentCtx::load` runs first; the check itself reads no ticket and no registry
    let o = acted(&s, &f, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.is_empty());
    assert_eq!(count_reads(&s.fake, from, s.t_info), 0);
}

#[test]
fn list_prints_the_receipts_and_reads_nothing() {
    let s = scene();
    let text = format!(
        "{}{}detached job finished: ticket 5 rc=0 log=/l result=/r\n",
        block(
            1,
            "TICKET",
            "assignment",
            &s.b.name,
            s.t_taken,
            s.before,
            ""
        ),
        block(2, "MESSAGE", "info", &s.b.name, s.t_info, s.before, "")
    );
    let f = s.dir.path().join("events.txt");
    std::fs::write(&f, text).unwrap();
    let from = s.fake.requests().len();
    let o = acted(&s, &f, &["--list"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        o.stdout,
        format!(
            "{REPO}#{} assignment\n{REPO}#{} info\n#5 detached-job-finished\n",
            s.t_taken, s.t_info
        )
    );
    assert_eq!(s.fake.requests().len(), from, "no request");
}
