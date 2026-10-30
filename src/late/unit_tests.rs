use super::*;
use std::cell::Cell;

const T0: i64 = 1_800_000_000;
const H: i64 = 3600;
const SHA1: &str = "1111111111111111111111111111111111111111";
const SHA2: &str = "2222222222222222222222222222222222222222";

fn row(n: u64, status: &str, agent: &str, entered_ago: i64) -> TicketRow {
    TicketRow {
        repo: "o/r".into(),
        issue: n,
        item_id: format!("PVTI_{n}"),
        status: Some(status.into()),
        agent: Some(agent.into()),
        order: None,
        updated: Some(T0 - entered_ago),
    }
}

/// Injected GitHub data. `calls` counts the reads that are not free.
struct Fake {
    branch: Option<bool>,
    prs: Vec<OpenPr>,
    green: Option<Option<i64>>,
    /// Overrides `green` for `ci_state` when set.
    state: Option<Option<CiState>>,
    done: Option<bool>,
    verify: Option<VerifyState>,
    question: Option<i64>,
    /// The issues whose open wait is a question to the owner.
    to_owner: Vec<u64>,
    calls: Cell<u32>,
}

impl Fake {
    fn new() -> Self {
        Fake {
            branch: Some(false),
            prs: vec![],
            green: Some(None),
            state: None,
            done: Some(false),
            verify: None,
            question: None,
            to_owner: Vec::new(),
            calls: Cell::new(0),
        }
    }
    fn pr(mut self, n: u64, sha: &str) -> Self {
        self.prs.push(OpenPr {
            number: 100 + n,
            head_ref: format!("crew/dev@host/{n}-work"),
            head_sha: sha.into(),
            base_ref: "main".into(),
        });
        self
    }
}

impl Probe for Fake {
    fn branch_prefix(&self) -> String {
        "crew/".into()
    }
    fn has_branch(&self, _: &str, _: &str, _: &str, _: u64) -> Option<bool> {
        self.calls.set(self.calls.get() + 1);
        self.branch
    }
    fn open_prs(&self, _: &str) -> Option<Vec<OpenPr>> {
        Some(self.prs.clone())
    }
    fn ci_state(&self, _: &str, _: &OpenPr) -> Option<CiState> {
        self.calls.set(self.calls.get() + 1);
        if let Some(s) = &self.state {
            return s.clone();
        }
        match self.green {
            Some(Some(t)) => Some(CiState::Green(Some(t))),
            Some(None) => Some(CiState::Pending),
            None => None,
        }
    }
    fn has_done(&self, _: &str, _: u64, _: &str, _: &str) -> Option<bool> {
        self.done
    }
    fn verify_state(&self, _: &str, _: u64, _: Option<&str>) -> Option<VerifyState> {
        self.calls.set(self.calls.get() + 1);
        self.verify.clone()
    }
    fn question_open(&self, _: &str, issue: u64, _: Option<&str>) -> Option<OpenWait> {
        self.calls.set(self.calls.get() + 1);
        self.question.map(|created| OpenWait {
            created,
            to_owner: self.to_owner.contains(&issue),
        })
    }
}

/// The late waits of `inp.rows`, sorted by ticket and wait.
fn late(inp: &Inputs<'_>, probe: &dyn Probe) -> Vec<Late> {
    scan(inp, probe).late
}

fn defaults() -> Expect {
    Expect::defaults()
}

fn inputs<'a>(
    rows: &'a [TicketRow],
    e: &'a Expect,
    seen: &'a BTreeMap<String, String>,
    scope: Scope<'a>,
    github: bool,
) -> Inputs<'a> {
    Inputs {
        rows,
        expect: e,
        now: T0,
        scope,
        github,
        seen,
        agents: &[],
        interval: 600,
    }
}

fn words(l: &[Late]) -> Vec<&'static str> {
    l.iter().map(|l| l.wait.case()).collect()
}

fn all(rows: &[TicketRow], f: &Fake) -> Vec<Late> {
    all_with(rows, &defaults(), f)
}

fn all_with(rows: &[TicketRow], e: &Expect, f: &Fake) -> Vec<Late> {
    let seen = BTreeMap::new();
    late(&inputs(rows, e, &seen, Scope::All, true), f)
}

fn cfg_with(table: &str) -> BoardConfig {
    let base = "initiative = \"x\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n";
    BoardConfig::parse(&format!("{base}{table}")).unwrap()
}

#[test]
fn push_is_late_over_the_expected_time_and_not_under_it() {
    let f = Fake::new();
    // over 2 h, no branch: late, with its age and since
    let over = all(&[row(1, "In Progress", "dev@host", 2 * H + 60)], &f);
    assert_eq!(words(&over), ["no-push"]);
    assert_eq!(over[0].age, 2 * H + 60);
    assert_eq!(over[0].since, T0 - 2 * H - 60);
    assert_eq!(over[0].expected, 2 * H);
    // exactly at the expected time and under it: not late
    for ago in [2 * H, H] {
        assert!(all(&[row(1, "In Progress", "dev@host", ago)], &f).is_empty());
    }
    // over the time, but the branch is pushed: not late
    let pushed = Fake {
        branch: Some(true),
        ..Fake::new()
    };
    assert!(all(&[row(1, "In Progress", "dev@host", 5 * H)], &pushed).is_empty());
    // the branch read failed: nothing is guessed
    let unknown = Fake {
        branch: None,
        ..Fake::new()
    };
    assert!(all(&[row(1, "In Progress", "dev@host", 5 * H)], &unknown).is_empty());
    // a ticket that is not In Progress is no push case
    assert!(all(&[row(1, "Done", "dev@host", 9 * H)], &f).is_empty());
}

#[test]
fn done_is_late_over_the_expected_time_and_not_under_it() {
    let rows = [row(7, "In Progress", "dev@host", 600)];
    let green_for = |secs: i64| Fake {
        green: Some(Some(T0 - secs)),
        ..Fake::new().pr(7, SHA1)
    };
    // green for 31 min: late, with the SHA and the PR; since is the time CI turned green
    let m = all(&rows, &green_for(1860));
    assert_eq!(words(&m), ["green-not-done"]);
    assert_eq!(m[0].age, 1860);
    assert_eq!(m[0].since, T0 - 1860);
    assert_eq!(m[0].sha.as_deref(), Some(SHA1));
    assert_eq!(m[0].pr.as_deref(), Some("o/r#107"));
    // green for 10 min, or exactly 30: not late
    for secs in [600, 1800] {
        assert!(all(&rows, &green_for(secs)).is_empty());
    }
    // green long enough, but a done names the head: not late
    let done = Fake {
        done: Some(true),
        ..green_for(7200)
    };
    assert!(all(&rows, &done).is_empty());
    // not green, or unreadable: not late
    assert!(all(&rows, &Fake::new().pr(7, SHA1)).is_empty());
    let unknown = Fake {
        green: None,
        ..Fake::new().pr(7, SHA1)
    };
    assert!(all(&rows, &unknown).is_empty());
    // a PR of another ticket is not this ticket's PR
    let other = Fake {
        green: Some(Some(T0 - 7200)),
        ..Fake::new().pr(8, SHA1)
    };
    assert!(all(&rows, &other).is_empty());
}

#[test]
fn verify_is_late_over_the_expected_time_and_not_under_it() {
    let f = Fake::new();
    let seen = BTreeMap::new();
    let e = defaults();
    let reading = |rows: &[TicketRow]| late(&inputs(rows, &e, &seen, Scope::All, true), &f);
    let m = reading(&[row(3, "Review", "dev@host", 1801)]);
    assert_eq!(words(&m), ["review-not-verified"]);
    assert_eq!(m[0].age, 1801);
    for ago in [1800, 60] {
        assert!(reading(&[row(3, "Review", "dev@host", ago)]).is_empty());
    }
    // other statuses are not a verify case
    assert!(reading(&[row(3, "Done", "dev@host", 9 * H)]).is_empty());
}

#[test]
fn the_waits_that_are_off_do_not_fire_and_fire_when_the_table_gives_a_time() {
    let d = defaults();
    assert_eq!(
        (d.take, d.answer, d.owner),
        (None, Some(4 * H), Some(24 * H))
    );
    let off = Expect {
        answer: None,
        owner: None,
        ..d
    };
    let rows = [
        row(1, "Ready", "dev@host", 3 * H),
        row(2, "Blocked", "dev@host", 3 * H),
        row(3, "Blocked", "dev@host", 3 * H),
        row(4, "Review", "dev@host", 10),
    ];
    let f = Fake {
        question: Some(T0 - 2 * H),
        to_owner: vec![3],
        verify: Some(VerifyState::OwnerMerge),
        ..Fake::new()
    };
    // control for the filter: with the times on, every wait fires
    let on = Expect {
        take: Some(H),
        answer: Some(H),
        owner: Some(5),
        ..off
    };
    let got = all_with(&rows, &on, &f);
    assert_eq!(words(&got), ["take", "answer", "owner", "owner-merge"]);
    assert_eq!(got[0].expected, H);
    assert_eq!(got[1].since, T0 - 2 * H, "answer counts from the question");
    assert_eq!(got[2].since, T0 - 2 * H);
    // off: nothing fires and nothing is read
    let g = Fake {
        question: Some(T0 - 2 * H),
        to_owner: vec![3],
        verify: Some(VerifyState::OwnerMerge),
        ..Fake::new()
    };
    assert!(all_with(&rows, &off, &g).is_empty());
    assert_eq!(g.calls.get(), 0);
    // owner-merge only when the done says so
    let plain = Fake {
        verify: Some(VerifyState::Verified {
            head: SHA1.into(),
            since: T0 - H,
        }),
        ..Fake::new()
    };
    assert!(
        all_with(&rows[3..], &on, &plain)
            .iter()
            .all(|l| l.wait != Wait::OwnerMerge)
    );
    // a question younger than the time, or none, is not late
    let young = Fake {
        question: Some(T0 - 60),
        ..Fake::new()
    };
    assert!(all_with(&rows[1..2], &on, &young).is_empty());
    assert!(all_with(&rows[1..3], &on, &Fake::new()).is_empty());
}

#[test]
fn expect_reads_late_else_stale_else_the_defaults() {
    let d = defaults();
    assert_eq!(expect(&cfg_with("")), d);
    assert_eq!((d.push, d.done, d.verify), (7200, 1800, 1800));
    assert_eq!((d.escalate, d.repeat), (1800, 7200));
    // [stale]: no_push is push, green is done, review is verify
    let s = expect(&cfg_with(
        "[stale]\nno_push = \"1h\"\nreview = \"10m\"\nescalate = \"45m\"\nrepeat = \"3h\"\n",
    ));
    assert_eq!(
        (s.push, s.done, s.verify, s.escalate, s.repeat),
        (3600, 1800, 600, 2700, 3 * 3600)
    );
    assert_eq!(
        (s.take, s.answer, s.owner),
        (None, Some(4 * H), Some(24 * H))
    );
    // [late]: the new keys, and the three waits for a person
    let l = expect(&cfg_with(
        "[late]\npush = \"1h\"\ndone = \"20m\"\nverify = \"10m\"\nescalate = \"1h\"\nrepeat = \"4h\"\ntake = \"15m\"\nanswer = \"1h\"\nowner = \"24h\"\n",
    ));
    assert_eq!(
        (l.push, l.done, l.verify, l.escalate, l.repeat),
        (3600, 1200, 600, 3600, 4 * 3600)
    );
    assert_eq!(
        (l.take, l.answer, l.owner),
        (Some(900), Some(3600), Some(86400))
    );
}

#[test]
fn the_waits_for_a_person_have_defaults_and_each_key_can_turn_one_off() {
    let d = defaults();
    // a [late] table without the keys keeps the defaults
    let none = expect(&cfg_with("[late]\npush = \"1h\"\n"));
    assert_eq!(
        (none.take, none.answer, none.owner),
        (None, d.answer, d.owner)
    );
    // `off` (any case) turns one wait off and leaves the others
    let off = expect(&cfg_with("[late]\nanswer = \"off\"\nowner = \"OFF\"\n"));
    assert_eq!((off.take, off.answer, off.owner), (None, None, None));
    let one = expect(&cfg_with("[late]\nowner = \"off\"\ntake = \"1h\"\n"));
    assert_eq!(
        (one.take, one.answer, one.owner),
        (Some(3600), d.answer, None)
    );
    // a table survives a write and a read with `off`
    let c = cfg_with("[late]\nanswer = \"off\"\n");
    assert_eq!(BoardConfig::parse(&c.to_toml()).unwrap(), c);
    // `off` is no value for the other keys
    let base = "initiative = \"x\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n";
    assert!(BoardConfig::parse(&format!("{base}[late]\npush = \"off\"\n")).is_err());
}

#[test]
fn a_blocked_question_and_an_owner_merge_are_late_by_default() {
    let rows = [
        row(2, "Blocked", "dev@host", 5 * H),
        row(3, "Blocked", "dev@host", 25 * H),
        row(4, "Review", "dev@host", 25 * H),
    ];
    let f = Fake {
        question: Some(T0 - 5 * H),
        to_owner: vec![3],
        verify: Some(VerifyState::OwnerMerge),
        ..Fake::new()
    };
    let got = all_with(&rows, &defaults(), &f);
    // the owner question is 5 h old here, so it is not late yet; the answer wait and the merge wait fire
    assert_eq!(words(&got), ["answer", "owner-merge"]);
}

#[test]
fn a_bad_time_or_both_tables_in_the_config_file_are_refused() {
    let base = "initiative = \"x\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n";
    for bad in [
        "[stale]\nno_push = \"soon\"",
        "[stale]\nreview = \"0\"",
        "[stale]\ngreeen = \"1h\"",
        "[stale]\nescalate = \"later\"",
        "[stale]\nrepeat = \"0\"",
        "[late]\npush = \"soon\"",
        "[late]\ntake = \"0\"",
        "[late]\nowner = \"later\"",
        "[late]\nno_push = \"1h\"",
        "[late]\npush = \"1h\"\n[stale]\nno_push = \"1h\"",
    ] {
        assert!(
            BoardConfig::parse(&format!("{base}{bad}\n")).is_err(),
            "{bad}"
        );
    }
    let both = BoardConfig::parse(&format!("{base}[late]\n[stale]\n")).unwrap_err();
    assert!(
        both.to_string().contains("late-and-stale-both-set"),
        "{both}"
    );
    // a table survives a write and a read
    for t in [
        "[stale]\nno_push = \"3h\"\n",
        "[late]\npush = \"3h\"\ntake = \"5m\"\n",
    ] {
        let ok = cfg_with(t);
        assert_eq!(BoardConfig::parse(&ok.to_toml()).unwrap(), ok);
    }
}

#[test]
fn a_changed_time_moves_the_line() {
    // 90 min is over a 1 h push time, under the default
    let rows = [row(1, "In Progress", "dev@host", 90 * 60)];
    assert!(all(&rows, &Fake::new()).is_empty());
    let e = expect(&cfg_with("[late]\npush = \"1h\"\n"));
    assert_eq!(words(&all_with(&rows, &e, &Fake::new())), ["no-push"]);
}

#[test]
fn a_wait_comes_once_per_sha_and_a_new_head_comes_again() {
    let rows = [row(7, "In Progress", "dev@host", 600)];
    let e = defaults();
    let green = |sha: &str| Fake {
        green: Some(Some(T0 - 3600)),
        ..Fake::new().pr(7, sha)
    };
    let dev = Scope::Dev("dev@host");
    let mut seen = BTreeMap::new();
    // first poll: one wait; the caller records it
    let first = late(&inputs(&rows, &e, &seen, dev, true), &green(SHA1));
    assert_eq!(first.len(), 1);
    seen.insert(first[0].key().unwrap(), first[0].value());
    // second poll, same SHA: nothing, and the CI read is skipped
    let f = green(SHA1);
    assert!(late(&inputs(&rows, &e, &seen, dev, true), &f).is_empty());
    assert_eq!(f.calls.get(), 0, "a seen wait must not read CI again");
    // a new head SHA: a new event
    let third = late(&inputs(&rows, &e, &seen, dev, true), &green(SHA2));
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].sha.as_deref(), Some(SHA2));
    seen.insert(third[0].key().unwrap(), third[0].value());
    assert!(late(&inputs(&rows, &e, &seen, dev, true), &green(SHA2)).is_empty());
}

#[test]
fn push_and_verify_come_again_after_the_ticket_left_the_status() {
    let mut seen = BTreeMap::new();
    let np = [row(1, "In Progress", "dev@host", 3 * H)];
    let rv = [row(2, "Review", "dev@host", 2 * H)];
    let both: Vec<TicketRow> = np.iter().chain(rv.iter()).cloned().collect();
    let f = Fake::new();
    let e = defaults();
    let first = late(&inputs(&both, &e, &seen, Scope::All, true), &f);
    assert_eq!(words(&first), ["no-push", "review-not-verified"]);
    for l in &first {
        seen.insert(l.key().unwrap(), l.value());
    }
    // the Review ticket is verified (Done): its entry goes; the other stays
    seen.insert("PVTI_1|no-push|escalated".into(), "-|1".into());
    seen.insert("PVTI_5|take".into(), "-".into());
    let after = [np[0].clone(), row(2, "Done", "dev@host", 0)];
    prune_seen(&mut seen, &after);
    assert_eq!(
        seen.keys().collect::<Vec<_>>(),
        ["PVTI_1|no-push", "PVTI_1|no-push|escalated"]
    );
}

#[test]
fn the_legacy_option_needs_human_reads_as_blocked() {
    let on = Expect {
        answer: Some(H),
        owner: Some(H),
        ..defaults()
    };
    let rows = [row(3, "Needs Human", "dev@host", 3 * H)];
    // a question to the owner: the owner wait; a block: the answer wait
    let f = Fake {
        question: Some(T0 - 2 * H),
        to_owner: vec![3],
        ..Fake::new()
    };
    assert_eq!(words(&all_with(&rows, &on, &f)), ["owner"]);
    let f = Fake {
        question: Some(T0 - 2 * H),
        ..Fake::new()
    };
    assert_eq!(words(&all_with(&rows, &on, &f)), ["answer"]);
    // the seen entries of such a ticket stay while it is there, and go when it leaves
    let mut seen = BTreeMap::new();
    seen.insert("PVTI_3|owner".to_string(), "-".to_string());
    prune_seen(&mut seen, &rows);
    assert_eq!(seen.len(), 1);
    prune_seen(&mut seen, &[row(3, "Done", "dev@host", 0)]);
    assert!(seen.is_empty());
}

#[test]
fn the_scope_decides_whose_waits_are_looked_at() {
    let seen = BTreeMap::new();
    let e = defaults();
    let f = Fake {
        green: Some(Some(T0 - 3 * H)),
        ..Fake::new().pr(1, SHA1)
    };
    let rows = [
        row(1, "In Progress", "dev@host", 3 * H),
        row(2, "In Progress", "other@host", 3 * H),
        row(3, "Review", "other@host", 3 * H),
    ];
    // a dev agent: its own In Progress ticket only, no Review
    let dev = late(&inputs(&rows, &e, &seen, Scope::Dev("dev@host"), true), &f);
    assert_eq!(words(&dev), ["no-push", "green-not-done"]);
    assert!(dev.iter().all(|l| l.subject_name() == "o/r#1"));
    // the coordinator: Review of any agent, and a dev wait that is over its time by escalate
    let coord = late(&inputs(&rows, &e, &seen, Scope::Coordinator, true), &f);
    assert_eq!(
        coord
            .iter()
            .map(|l| (l.subject_name(), l.wait.case()))
            .collect::<Vec<_>>(),
        [
            ("o/r#1".to_string(), "no-push"),
            ("o/r#1".to_string(), "green-not-done"),
            ("o/r#2".to_string(), "no-push"),
            ("o/r#3".to_string(), "review-not-verified"),
        ],
        "only ticket 1 has the PR"
    );
    // status: everything
    let every = late(&inputs(&rows, &e, &seen, Scope::All, true), &f);
    assert_eq!(every.len(), 4, "{every:?}");
}

#[test]
fn the_coordinator_reads_a_dev_wait_only_after_expected_plus_escalate() {
    let f = Fake::new();
    // `done` is far away, so only the push time decides whether the coordinator reads
    let e = Expect {
        done: 10 * H,
        ..defaults()
    };
    let seen = BTreeMap::new();
    let coord =
        |rows: &[TicketRow], f: &Fake| scan(&inputs(rows, &e, &seen, Scope::Coordinator, true), f);
    // push 2 h + escalate 30 min: at 2 h 30 m exactly and under it, nothing and no read
    for ago in [2 * H, 2 * H + 1800] {
        let s = coord(&[row(1, "In Progress", "dev@host", ago)], &f);
        assert!(s.late.is_empty() && !s.read, "{ago}");
    }
    assert_eq!(f.calls.get(), 0, "a young ticket cost a read");
    let s = coord(&[row(1, "In Progress", "dev@host", 2 * H + 1801)], &f);
    assert_eq!(words(&s.late), ["no-push"]);
    assert!(s.read);
    assert_eq!(f.calls.get(), 1);
    // the branch is there: no wait, and the entry of the coordinator is cleared
    let pushed = Fake {
        branch: Some(true),
        ..Fake::new()
    };
    let s = coord(&[row(1, "In Progress", "dev@host", 3 * H)], &pushed);
    assert!(s.late.is_empty());
    assert!(s.clear.contains(&("PVTI_1".to_string(), Wait::Push)));
    // a failed read neither fires nor clears
    let unknown = Fake {
        branch: None,
        ..Fake::new()
    };
    let s = coord(&[row(1, "In Progress", "dev@host", 3 * H)], &unknown);
    assert!(s.late.is_empty());
    assert!(!s.clear.contains(&("PVTI_1".to_string(), Wait::Push)));
    // a ticket that is too young again clears
    let s = coord(&[row(1, "In Progress", "dev@host", 60)], &f);
    assert!(s.clear.contains(&("PVTI_1".to_string(), Wait::Push)));
    assert!(s.clear.contains(&("PVTI_1".to_string(), Wait::Done)));
}

#[test]
fn done_for_the_coordinator_needs_green_for_done_plus_escalate() {
    let e = defaults();
    let seen = BTreeMap::new();
    let rows = [row(7, "In Progress", "dev@host", 3 * H)];
    let coord = |green_for: i64, done: Option<bool>| {
        let f = Fake {
            green: Some(Some(T0 - green_for)),
            done,
            branch: Some(true),
            ..Fake::new().pr(7, SHA1)
        };
        scan(&inputs(&rows, &e, &seen, Scope::Coordinator, true), &f)
    };
    // green 30 m + 30 m: exactly there, not yet; one second more, late
    assert!(coord(3600, Some(false)).late.is_empty());
    assert_eq!(words(&coord(3601, Some(false)).late), ["green-not-done"]);
    // a done that names the head clears; an unreadable one neither fires nor clears
    let s = coord(7200, Some(true));
    assert!(s.late.is_empty());
    assert!(s.clear.contains(&("PVTI_7".to_string(), Wait::Done)));
    let s = coord(7200, None);
    assert!(s.late.is_empty());
    assert!(!s.clear.contains(&("PVTI_7".to_string(), Wait::Done)));
}

#[test]
fn tells_follows_the_table_row_by_row() {
    let e = defaults();
    let ticket = |wait: Wait, since: i64, expected: i64| Late {
        wait,
        subject: Subject::Ticket {
            key: "o/r#1".into(),
            repo: "o/r".into(),
            issue: 1,
            item_id: "PVTI_1".into(),
            agent: Some("dev@host".into()),
        },
        since,
        age: T0 - since,
        expected,
        sha: None,
        pr: None,
    };
    let dev = || Who::Dev("dev@host".into());
    // take, push, done: dev at the time, coordinator escalate later, the owner never
    for w in [Wait::Take, Wait::Push, Wait::Done] {
        let l = ticket(w, T0 - 1000, 1000);
        for late in [false, true] {
            assert!(tells(&l, &e, late, T0).is_empty(), "{w:?} at the time");
            assert_eq!(tells(&l, &e, late, T0 + 1), [dev()], "{w:?} over");
            assert_eq!(
                tells(&l, &e, late, T0 + 1800),
                [dev()],
                "{w:?} at +escalate"
            );
            assert_eq!(
                tells(&l, &e, late, T0 + 1801),
                [dev(), Who::Coordinator],
                "{w:?} over +escalate"
            );
        }
    }
    // verify: the coordinator at the time; the owner too, when the coordinator is late
    let l = ticket(Wait::Verify, T0 - 1000, 1000);
    assert!(tells(&l, &e, true, T0).is_empty());
    assert_eq!(tells(&l, &e, false, T0 + 1), [Who::Coordinator]);
    assert_eq!(tells(&l, &e, false, T0 + 9999), [Who::Coordinator]);
    assert_eq!(tells(&l, &e, true, T0 + 1), [Who::Coordinator, Who::Owner]);
    // answer: the coordinator at the time, the owner after `escalate` more
    let l = ticket(Wait::Answer, T0 - 1000, 1000);
    for late in [false, true] {
        assert!(tells(&l, &e, late, T0).is_empty());
        assert_eq!(tells(&l, &e, late, T0 + 1), [Who::Coordinator]);
        assert_eq!(tells(&l, &e, late, T0 + 1800), [Who::Coordinator]);
        assert_eq!(
            tells(&l, &e, late, T0 + 1801),
            [Who::Coordinator, Who::Owner]
        );
    }
    // owner-merge and owner: the owner at the time, nobody else
    for w in [Wait::OwnerMerge, Wait::Owner] {
        let l = ticket(w, T0 - 1000, 1000);
        assert!(tells(&l, &e, true, T0).is_empty(), "{w:?} at the time");
        assert_eq!(tells(&l, &e, false, T0 + 1), [Who::Owner], "{w:?}");
        assert_eq!(tells(&l, &e, true, T0 + 9999), [Who::Owner], "{w:?}");
    }
    // the gate of the reads and the rule agree
    assert_eq!(coordinator_hears_at(Wait::Push, 100, 30), Some(130));
    assert_eq!(coordinator_hears_at(Wait::Verify, 100, 30), Some(100));
    assert_eq!(coordinator_hears_at(Wait::Owner, 100, 30), None);
    assert!(coordinator_hears_again(Wait::Take) && !coordinator_hears_again(Wait::Answer));
    // a ticket without an agent has no dev to tell
    let mut orphan = ticket(Wait::Push, T0 - 1000, 1000);
    if let Subject::Ticket { agent, .. } = &mut orphan.subject {
        *agent = None;
    }
    assert!(tells(&orphan, &e, false, T0 + 1).is_empty());
}

fn late_of(wait: Wait, sha: Option<&str>) -> Late {
    Late {
        wait,
        subject: Subject::Ticket {
            key: "o/r#7".into(),
            repo: "o/r".into(),
            issue: 7,
            item_id: "PVTI_7".into(),
            agent: Some("dev@host".into()),
        },
        since: T0 - 3 * H,
        age: 3 * H,
        expected: 1800,
        sha: sha.map(str::to_string),
        pr: sha.map(|_| "o/r#107".to_string()),
    }
}

#[test]
fn the_event_text_tells_each_role_what_to_do() {
    let e = defaults();
    let dev = Who::Dev("dev@host".into());
    // the full bodies, byte for byte: the prompts match them
    assert_eq!(
        late_of(Wait::Push, None).event_body(&dev, &e, "crew/"),
        "stale-work: no-push\nticket: o/r#7\nIt is In Progress for 3h (limit 30m) and no branch `crew/dev@host/7-...` is on GitHub.\nPush your work now, then run `done`. If you cannot finish, run `block` and say why.\n"
    );
    assert_eq!(
        late_of(Wait::Done, Some(SHA1)).event_body(&dev, &e, "crew/"),
        "stale-work: green-not-done\nticket: o/r#7\nPR o/r#107 has green CI on head 1111111 for 3h (limit 30m) and no `done` names that head.\nRun `done` now. If the work is not finished, push it or run `block` and say why.\n"
    );
    assert_eq!(
        late_of(Wait::Verify, None).event_body(&Who::Coordinator, &e, "crew/"),
        "stale-work: review-not-verified\nticket: o/r#7\nIt is in Review for 3h (limit 30m) and its latest `done` has no passing `verify` on the current PR head.\nRun `verify` now. If you cannot, tell the owner why. A ticket that passed `verify` on the current head is not reported: if you hold its merge, nothing is late.\n"
    );
    assert_eq!(
        late_of(Wait::Done, Some(SHA1)).event_body(&Who::Coordinator, &e, "crew/"),
        "stale-work: green-not-done\nticket: o/r#7\nescalated: true\nagent: dev@host\nage: 3h\nlimit: 30m\nescalated after: 30m past the limit\nhead: 1111111\npr: o/r#107\nThe dev agent got this marker and did not act. Ask it with `say`, or `reassign` the ticket, or `ticket split` it.\n"
    );
    assert_eq!(
        late_of(Wait::Push, None).event_body(&Who::Coordinator, &e, "crew/"),
        "stale-work: no-push\nticket: o/r#7\nescalated: true\nagent: dev@host\nage: 3h\nlimit: 30m\nescalated after: 30m past the limit\nThe dev agent got this marker and did not act. Ask it with `say`, or `reassign` the ticket, or `ticket split` it.\n"
    );
    // a new wait has `late: <wait>` first, then the same ticket line
    let t = late_of(Wait::Take, None).event_body(&dev, &e, "crew/");
    assert!(t.starts_with("late: take\nticket: o/r#7\n") && t.contains("`take`"));
    let tu = late_of(Wait::Take, None).event_body(&Who::Coordinator, &e, "crew/");
    assert!(tu.starts_with("late: take\nticket: o/r#7\nescalated: true\n"));
    let a = late_of(Wait::Answer, None).event_body(&Who::Coordinator, &e, "crew/");
    assert!(a.starts_with("late: answer\nticket: o/r#7\n") && !a.contains("escalated"));
}

#[test]
fn the_tick_line_text() {
    let l = late_of(Wait::Push, None).tick_line();
    assert_eq!(
        l,
        "left for the interactive coordinator: o/r#7, stale-work escalated: no-push for 3h, agent dev@host; the interactive coordinator runs `say`, `reassign` or `ticket split`\n"
    );
    assert!(
        late_of(Wait::Take, None)
            .tick_line()
            .contains("late: take for 3h")
    );
}

#[test]
fn the_status_line_and_json_name_wait_subject_since_age_expected_and_who() {
    let e = defaults();
    let l = late_of(Wait::Push, None);
    let line = l.status_line(&e, false, T0);
    for want in [
        "push",
        "o/r#7",
        &format!("since {}", crate::clock::format_ts(T0 - 3 * H)),
        "age 3h",
        "expected 30m",
        "told: dev dev@host, coordinator",
    ] {
        assert!(line.contains(want), "{want}: {line}");
    }
    let j = l.to_json(&e, false, T0);
    assert_eq!(j["wait"], "push");
    assert_eq!(j["subject"], "o/r#7");
    assert_eq!(j["age_s"], 3 * H);
    assert_eq!(j["expected_s"], 1800);
    assert_eq!(j["told"], json!(["dev dev@host", "coordinator"]));
    // a young wait is told to nobody yet
    let young = Late {
        since: T0 - 10,
        age: 10,
        ..l
    };
    assert!(young.status_line(&e, false, T0).contains("nobody yet"));
}

fn scan_ci(
    rows: &[TicketRow],
    seen: &BTreeMap<String, String>,
    scope: Scope<'_>,
    f: &Fake,
) -> CiScan {
    let e = defaults();
    ci_scan(&inputs(rows, &e, seen, scope, true), f)
}

fn with_state(state: Option<CiState>) -> Fake {
    Fake {
        state: Some(state),
        ..Fake::new().pr(1, SHA1)
    }
}

#[test]
fn ci_finished_says_green_red_and_nothing_while_pending() {
    let rows = [row(1, "In Progress", "dev@host", 600)];
    let seen = BTreeMap::new();
    let dev = Scope::Dev("dev@host");
    let green = scan_ci(&rows, &seen, dev, &with_state(Some(CiState::Green(None))));
    assert_eq!(green.events.len(), 1);
    assert_eq!(green.events[0].result, CiResult::Green);
    assert_eq!(green.events[0].sha, SHA1);
    assert_eq!(green.events[0].pr, "o/r#101");
    let red = scan_ci(
        &rows,
        &seen,
        dev,
        &with_state(Some(CiState::Red(vec![
            "run 5 (CI) concluded failure".into(),
        ]))),
    );
    assert_eq!(red.events[0].result, CiResult::Red);
    assert!(red.events[0].event_body().contains("concluded failure"));
    // not finished, or not readable: nothing
    for st in [Some(CiState::Pending), None] {
        let none = scan_ci(&rows, &seen, dev, &with_state(st));
        assert!(none.events.is_empty() && none.none_seen.is_empty());
    }
}

#[test]
fn ci_finished_comes_once_per_head_and_again_for_a_new_head() {
    let rows = [row(1, "In Progress", "dev@host", 600)];
    let dev = Scope::Dev("dev@host");
    let f = with_state(Some(CiState::Green(None)));
    let mut seen = BTreeMap::new();
    seen.insert(format!("PVTI_1|{CI_FINISHED}"), SHA1.to_string());
    let again = scan_ci(&rows, &seen, dev, &f);
    assert!(again.events.is_empty());
    assert_eq!(f.calls.get(), 0, "a head with an event is not read again");
    // a new head SHA
    let g = Fake {
        state: Some(Some(CiState::Green(None))),
        ..Fake::new().pr(1, SHA2)
    };
    assert_eq!(scan_ci(&rows, &seen, dev, &g).events[0].sha, SHA2);
}

#[test]
fn ci_finished_is_for_the_owner_of_an_in_progress_ticket_only() {
    let seen = BTreeMap::new();
    let f = with_state(Some(CiState::Green(None)));
    let rows = [
        row(1, "In Progress", "dev@host", 600),
        row(2, "In Progress", "other@host", 600),
        row(3, "Review", "dev@host", 600),
        row(4, "Ready", "dev@host", 600),
    ];
    let got = scan_ci(&rows, &seen, Scope::Dev("dev@host"), &f);
    assert_eq!(got.events.len(), 1);
    assert_eq!(got.events[0].ticket, "o/r#1");
    // the branch of the PR names this agent, but the ticket belongs to another one
    let foreign = [row(1, "In Progress", "other@host", 600)];
    assert!(
        scan_ci(&foreign, &seen, Scope::Dev("dev@host"), &f)
            .events
            .is_empty()
    );
    assert!(
        scan_ci(&rows, &seen, Scope::Coordinator, &f)
            .events
            .is_empty()
    );
    assert!(scan_ci(&rows, &seen, Scope::All, &f).events.is_empty());
    let e = defaults();
    let off = ci_scan(&inputs(&rows, &e, &seen, Scope::Dev("dev@host"), false), &f);
    assert!(off.events.is_empty());
}

#[test]
fn no_ci_needs_two_answers() {
    let rows = [row(1, "In Progress", "dev@host", 600)];
    let dev = Scope::Dev("dev@host");
    let f = with_state(Some(CiState::NoCi));
    let first = scan_ci(&rows, &BTreeMap::new(), dev, &f);
    assert!(first.events.is_empty());
    assert_eq!(first.none_seen, [("PVTI_1".to_string(), SHA1.to_string())]);
    let mut seen = BTreeMap::new();
    seen.insert(format!("PVTI_1|{CI_NONE_SEEN}"), SHA1.to_string());
    let second = scan_ci(&rows, &seen, dev, &f);
    assert_eq!(second.events[0].result, CiResult::None);
    // the first answer was for another head: it starts again
    seen.insert(format!("PVTI_1|{CI_NONE_SEEN}"), SHA2.to_string());
    assert!(scan_ci(&rows, &seen, dev, &f).events.is_empty());
}

#[test]
fn the_ci_finished_text_tells_what_to_do() {
    let e = |result, reasons: Vec<String>| CiFinished {
        ticket: "o/r#1".into(),
        repo: "o/r".into(),
        issue: 1,
        item_id: "PVTI_1".into(),
        sha: SHA1.into(),
        pr: "o/r#101".into(),
        result,
        reasons,
    };
    let g = e(CiResult::Green, vec![]).event_body();
    assert!(g.starts_with("ci-finished: green\n") && g.contains(SHA1));
    assert!(
        g.contains(&format!(
            "next: crewboard done 1 --pr https://github.com/o/r/pull/101 --sha {SHA1}\n"
        )),
        "{g}"
    );
    let r = e(CiResult::Red, vec!["x".into()]).event_body();
    assert!(r.starts_with("ci-finished: red\n") && r.contains("Fix the cause and push"));
    let n = e(CiResult::None, vec![]).event_body();
    assert!(n.starts_with("ci-finished: none\n") && n.contains("`CI: none`"));
}

#[test]
fn a_throttled_run_reads_nothing_from_github() {
    let seen = BTreeMap::new();
    let e = defaults();
    let f = Fake {
        green: Some(Some(T0 - 3 * H)),
        ..Fake::new().pr(1, SHA1)
    };
    let rows = [row(1, "In Progress", "dev@host", 3 * H)];
    let dev = Scope::Dev("dev@host");
    let s = scan(&inputs(&rows, &e, &seen, dev, false), &f);
    assert!(s.late.is_empty() && !s.read);
    assert_eq!(f.calls.get(), 0);
    // control: the same data with the read allowed fires
    let s = scan(&inputs(&rows, &e, &seen, dev, true), &f);
    assert!(s.read);
    assert_eq!(s.late.len(), 2);
}

#[test]
fn the_open_pr_list_is_parsed_and_a_wrong_shape_is_not_a_list() {
    let body = json!([
        {"number": 5, "head": {"ref": "crew/a@h/5-x", "sha": SHA1}, "base": {"ref": "main"}},
        {"number": 6, "head": {"ref": "feature"}, "base": {"ref": "main"}}
    ]);
    let v = parse_open_prs(&body).unwrap();
    assert_eq!(v.len(), 1, "an entry without a head SHA is dropped");
    assert_eq!(v[0].number, 5);
    assert_eq!(v[0].base_ref, "main");
    assert!(parse_open_prs(&json!({"message": "Not Found"})).is_none());
}

#[test]
fn green_since_is_the_latest_completion_of_the_counted_green_parts() {
    let runs = json!({"workflow_runs": [
        {"head_sha": SHA1, "event": "push", "conclusion": "success", "updated_at": "2026-10-17T10:00:00Z"},
        {"head_sha": SHA1, "event": "pull_request", "conclusion": "success", "updated_at": "2026-10-17T10:20:00Z"},
        // not counted: a schedule run, a run of another SHA, a failed run
        {"head_sha": SHA1, "event": "schedule", "conclusion": "success", "updated_at": "2026-10-17T12:00:00Z"},
        {"head_sha": SHA2, "event": "push", "conclusion": "success", "updated_at": "2026-10-17T13:00:00Z"},
        {"head_sha": SHA1, "event": "push", "conclusion": "failure", "updated_at": "2026-10-17T14:00:00Z"}
    ]});
    let checks = json!({"check_runs": [
        {"head_sha": SHA1, "conclusion": "success", "completed_at": "2026-10-17T10:25:00Z"},
        {"head_sha": SHA1, "conclusion": "skipped", "completed_at": "2026-10-17T15:00:00Z"}
    ]});
    let statuses = json!([
        {"state": "success", "updated_at": "2026-10-17T10:10:00Z"},
        {"state": "pending", "updated_at": "2026-10-17T16:00:00Z"}
    ]);
    let t = |s: &str| crate::clock::parse_ts(s).unwrap();
    assert_eq!(
        green_since_of(&[runs.clone(), checks.clone(), statuses.clone()], SHA1),
        Some(t("2026-10-17T10:25:00Z"))
    );
    // without the check runs, the run decides
    assert_eq!(
        green_since_of(&[runs, statuses], SHA1),
        Some(t("2026-10-17T10:20:00Z"))
    );
    // nothing with a time: no guess
    assert_eq!(green_since_of(&[json!({"workflow_runs": []})], SHA1), None);
    assert_eq!(green_since_of(&[checks], SHA2), None);
}

const I: i64 = 600;

fn agent(name: &str, coordinator: bool, since: Option<i64>, live: Liveness) -> AgentLate {
    AgentLate {
        name: name.into(),
        coordinator,
        since,
        live,
    }
}

fn lagging(age: i64) -> Liveness {
    Liveness::Late { age: Some(age) }
}

fn dead(age: i64) -> Liveness {
    Liveness::Dead { age }
}

#[test]
fn an_agent_is_late_when_stale_or_dead_and_not_before() {
    let alive = agent("c@h", true, Some(T0 - 100), Liveness::Alive { age: 100 });
    assert_eq!(agent_late(&alive, T0, I), None);
    let off = agent("c@h", true, Some(T0 - 100), Liveness::NotRunning);
    assert_eq!(agent_late(&off, T0, I), None);
    // stale: the heartbeat wait, expected after 3 intervals
    let a = agent("c@h", true, Some(T0 - 1900), lagging(1900));
    let l = agent_late(&a, T0, I).unwrap();
    assert_eq!(
        (l.wait, l.since, l.age, l.expected),
        (Wait::Heartbeat, T0 - 1900, 1900, 3 * I)
    );
    assert!(l.is_coordinator());
    // no baseline: nothing is judged
    assert_eq!(
        agent_late(&agent("c@h", true, None, lagging(0)), T0, I),
        None
    );
    // dead: the dead wait, expected after 5 intervals, the key comes from the heartbeat value
    let a = agent("a@h", false, Some(T0 - 4000), dead(4000));
    let l = agent_late(&a, T0, I).unwrap();
    assert_eq!((l.wait, l.expected), (Wait::Dead, 5 * I));
    assert_eq!(l.report_key(None), format!("dead:a@h:{}", T0 - 4000));
    // one episode, one key: the same heartbeat gives the same key at any later time
    let later = agent_late(&a, T0 + 600, I).unwrap();
    assert_eq!(later.report_key(None), l.report_key(None));
    // a revived agent that dies again was last seen with a newer heartbeat: a new key
    let again = agent("a@h", false, Some(T0 + 100), dead(4000));
    assert_ne!(
        agent_late(&again, T0 + 5000, I).unwrap().report_key(None),
        l.report_key(None)
    );
    // dead without a baseline is not judged (derive never gives it)
    assert_eq!(agent_late(&agent("a@h", false, None, dead(0)), T0, I), None);
}

#[test]
fn tells_names_the_owner_for_the_agent_rows() {
    let e = defaults();
    let coord = agent_late(&agent("c@h", true, Some(T0 - 1800), lagging(1800)), T0, I).unwrap();
    // the coordinator heartbeat: the owner when late, nobody else, whatever the flag says
    assert_eq!(tells(&coord, &e, false, T0), [Who::Owner]);
    let young = Late {
        since: T0 - 1799,
        age: 1799,
        ..coord.clone()
    };
    assert!(tells(&young, &e, false, T0).is_empty());
    // a dev agent heartbeat: nobody is told (the coordinator only shows it)
    let dev = agent_late(&agent("a@h", false, Some(T0 - 9000), lagging(9000)), T0, I).unwrap();
    assert!(tells(&dev, &e, true, T0).is_empty());
    // dead: the owner at once, for a dev agent too
    let d = agent_late(&agent("a@h", false, Some(T0 - 4000), dead(4000)), T0, I).unwrap();
    assert_eq!(tells(&d, &e, false, T0), [Who::Owner]);
    assert_eq!(Who::Owner.label(), "owner");
}

#[test]
fn a_late_coordinator_is_found_in_the_waits() {
    let c = agent_late(&agent("c@h", true, Some(T0 - 4000), lagging(4000)), T0, I).unwrap();
    let d = agent_late(&agent("a@h", false, Some(T0 - 4000), lagging(4000)), T0, I).unwrap();
    assert!(coordinator_is_late(&[d.clone(), c]));
    assert!(!coordinator_is_late(&[d]));
    assert!(!coordinator_is_late(&[]));
    // `scan` adds the agent waits to the ticket waits
    let agents = [agent("c@h", true, Some(T0 - 4000), lagging(4000))];
    let (rows, seen, e) = (Vec::new(), BTreeMap::new(), defaults());
    let mut inp = inputs(&rows, &e, &seen, Scope::Coordinator, false);
    inp.agents = &agents;
    let found = late(&inp, &Fake::new());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].wait, Wait::Heartbeat);
}

#[test]
fn the_owner_note_has_the_key_since_next_and_the_text() {
    let c = agent_late(&agent("c@h", true, Some(T0 - 1800), lagging(1800)), T0, I).unwrap();
    let note = c.owner_info("owner-login", "", None).unwrap();
    let lines: Vec<&str> = note.lines().collect();
    assert_eq!(lines[0], "late: heartbeat c@h");
    assert_eq!(lines[1], format!("late-key: heartbeat:c@h:{}", T0 - 1800));
    assert_eq!(
        lines[2],
        format!("since: {}", crate::clock::format_ts(T0 - 1800))
    );
    assert!(lines[3].starts_with("next: restart the coordinator session"));
    assert!(
        note.contains("@owner-login: the coordinator c@h shows no sign of life for 1800 s")
            && note.contains("late after 1800 s). It is not running."),
        "{note}"
    );
    // a ticket: the key has the ticket and the time since which it waits
    let v = late_of(Wait::Verify, None);
    let note = v
        .owner_info("o", "coordinator heartbeat: X (late)\n", Some(42))
        .unwrap();
    assert!(
        note.starts_with("late: verify o/r#7\nlate-key: verify:o/r#7:"),
        "{note}"
    );
    assert!(
        note.ends_with("coordinator heartbeat: X (late)\n"),
        "{note}"
    );
    assert!(note.contains("next: the interactive coordinator verifies, or the owner decides"));
    assert!(
        note.contains("late-key: verify:o/r#7:") && note.contains(":42\n"),
        "{note}"
    );
    // the key of a verify note names the coordinator baseline; other waits ignore it
    assert_eq!(v.report_key(None), format!("verify:o/r#7:{}", T0 - 3 * H));
    assert_eq!(
        v.report_key(Some(42)),
        format!("verify:o/r#7:{}:42", T0 - 3 * H)
    );
    let a = late_of(Wait::Answer, None);
    assert_eq!(a.report_key(Some(42)), a.report_key(None));
    // a dev wait is never told to the owner: no note
    assert_eq!(late_of(Wait::Push, None).owner_info("o", "", None), None);
}

fn verified(head: &str) -> Option<VerifyState> {
    Some(VerifyState::Verified {
        head: head.into(),
        since: T0 - 2 * H,
    })
}

fn scan_all(rows: &[TicketRow], f: &Fake) -> Scan {
    let (e, seen) = (defaults(), BTreeMap::new());
    scan(&inputs(rows, &e, &seen, Scope::All, true), f)
}

#[test]
fn a_passed_verify_on_the_current_head_ends_review_not_verified() {
    let rows = [row(3, "Review", "dev@host", 3 * H)];
    // control: the same ticket without a verify is late
    let none = Fake::new().pr(3, SHA1);
    assert_eq!(words(&all(&rows, &none)), ["review-not-verified"]);
    // verified on the head the PR has: no wait, and the ticket is listed as waiting for merge
    let f = Fake {
        verify: verified(SHA1),
        ..Fake::new().pr(3, SHA1)
    };
    let got = scan_all(&rows, &f);
    assert!(got.late.is_empty(), "{:?}", got.late);
    assert_eq!(got.verified, [("PVTI_3".to_string(), T0 - 2 * H)]);
    assert!(got.read);
    // an abbreviated head is the same head
    let short = Fake {
        verify: verified(&SHA1[..7]),
        ..Fake::new().pr(3, SHA1)
    };
    assert!(scan_all(&rows, &short).late.is_empty());
}

#[test]
fn a_passed_verify_of_an_older_head_does_not_end_review_not_verified() {
    let rows = [row(3, "Review", "dev@host", 3 * H)];
    // a push after the verify: the PR has a new head
    let moved = Fake {
        verify: verified(SHA1),
        ..Fake::new().pr(3, SHA2)
    };
    let got = scan_all(&rows, &moved);
    assert_eq!(words(&got.late), ["review-not-verified"]);
    assert!(got.verified.is_empty());
    // no open PR for the ticket, a verify that failed, or a read that failed: still late
    let no_pr = Fake {
        verify: verified(SHA1),
        ..Fake::new()
    };
    assert_eq!(
        words(&scan_all(&rows, &no_pr).late),
        ["review-not-verified"]
    );
    let failed = Fake {
        verify: Some(VerifyState::Unverified),
        ..Fake::new().pr(3, SHA1)
    };
    assert_eq!(
        words(&scan_all(&rows, &failed).late),
        ["review-not-verified"]
    );
    let unread = Fake::new().pr(3, SHA1);
    assert_eq!(
        words(&scan_all(&rows, &unread).late),
        ["review-not-verified"]
    );
    // a ticket without an agent cannot be verified: it stays late, with or without reads
    let (e, seen) = (defaults(), BTreeMap::new());
    let mut orphan = row(3, "Review", "dev@host", 3 * H);
    orphan.agent = None;
    let off = scan(
        &inputs(&[orphan], &e, &seen, Scope::All, false),
        &Fake::new(),
    );
    assert_eq!(words(&off.late), ["review-not-verified"]);
}

#[test]
fn an_owner_merge_ticket_has_only_the_owner_merge_wait() {
    let rows = [row(3, "Review", "dev@host", 25 * H)];
    let f = Fake {
        verify: Some(VerifyState::OwnerMerge),
        ..Fake::new().pr(3, SHA1)
    };
    let got = scan_all(&rows, &f);
    assert_eq!(words(&got.late), ["owner-merge"]);
    assert!(got.verified.is_empty());
}

#[test]
fn a_scan_without_reads_raises_no_verify_wait() {
    // A passing verify on the current head exists, but this scan cannot read: it must not
    // report `review-not-verified`; the next reading scan judges the ticket.
    let rows = [row(3, "Review", "dev@host", 3 * H)];
    let (e, seen) = (defaults(), BTreeMap::new());
    let f = Fake {
        verify: verified(SHA1),
        ..Fake::new().pr(3, SHA1)
    };
    let off = scan(&inputs(&rows, &e, &seen, Scope::All, false), &f);
    assert!(off.late.is_empty(), "{:?}", words(&off.late));
    assert!(!off.read);
    assert_eq!(f.calls.get(), 0);
    // and it clears nothing: a wait that was told stays told
    assert!(off.clear.is_empty());
    // control: the reading scan of the same ticket finds it verified
    let on = scan_all(&rows, &f);
    assert!(on.late.is_empty() && on.verified.len() == 1);
    // control: a reading scan of an unverified ticket still reports it
    let none = Fake::new().pr(3, SHA1);
    assert_eq!(words(&scan_all(&rows, &none).late), ["review-not-verified"]);
}

#[test]
fn an_owner_route_ticket_has_no_verify_wait() {
    // `verify` refuses an owner-route ticket by design: the verify wait could never clear.
    let rows = [row(3, "Review", "dev@host", 3 * H)];
    let f = Fake {
        verify: Some(VerifyState::OwnerMerge),
        ..Fake::new().pr(3, SHA1)
    };
    let got = scan_all(&rows, &f);
    assert!(got.late.is_empty(), "{:?}", words(&got.late));
    assert!(got.verified.is_empty());
    assert!(got.read);
}
