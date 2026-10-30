//! Profiles, capability matching and the toolchain check of `start` against a fake GitHub
//!.
//!
//! These tests run in plain `cargo test`: no token, no sandbox, no real GitHub. They replace
//! four tests of `it_sandbox.rs` (see `CONTRIBUTING.md`). The facts (`arch:`, `os:`,
//! `tool:`) come from the host that runs the test, as on the sandbox.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, Out, REGISTRY};
use crate::support::cli::GATE;
use crewboard::config::{ConfigDir, RequiredTool};
use crewboard::rules::Role;
use serde_json::{Value, json};

fn run_ok(fake: &FakeBoard, dir: &std::path::Path, args: &[&str]) -> Out {
    let o = fake.run(dir, args);
    assert_eq!(o.code, 0, "{args:?}: {}", o.all());
    o
}

/// The command is refused: exit 2 and the rule named in the text.
fn run_refused(fake: &FakeBoard, dir: &std::path::Path, args: &[&str], rule: &str) -> Out {
    let o = fake.run(dir, args);
    assert_eq!(o.code, 2, "{args:?}: {}", o.all());
    assert!(o.stderr.contains(rule), "{rule} missing: {}", o.all());
    o
}

fn json_of(o: &Out) -> Value {
    serde_json::from_str(o.stdout.lines().last().unwrap_or(""))
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", o.all()))
}

fn caps(fake: &FakeBoard, a: &Agent) -> String {
    fake.item_field(&a.item_id, "Capabilities")
        .unwrap_or_default()
}

/// The comments in the registry that record a profile change: a signed `Profile change.`
/// message of the agent or a `/crew profile` comment of the owner.
fn profile_records(fake: &FakeBoard) -> Vec<String> {
    fake.comments_of(REGISTRY)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.starts_with("/crew profile") || b.contains("\nProfile change.\n"))
        .collect()
}

fn arch_of(fake: &FakeBoard, a: &Agent) -> String {
    caps(fake, a)
        .split_whitespace()
        .find_map(|t| t.strip_prefix("arch:"))
        .map(str::to_string)
        .expect("arch fact")
}

/// A coordinator and a dev agent B; B has run `start`, so its facts are on the board.
fn crew() -> (FakeBoard, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    run_ok(&fake, &b.dir, &["start"]);
    (fake, c, b)
}

fn set_profile(fake: &FakeBoard, a: &Agent, args: &[&str]) -> Out {
    let mut v = vec!["agent", "profile", "set"];
    v.extend_from_slice(args);
    fake.run(&a.dir, &v)
}

#[test]
#[allow(clippy::too_many_lines)]
fn profile_facts_declared_tags_owner_changes_and_signed_history() {
    let (fake, _c, b) = crew();
    let owner = fake.owner_dir();
    let dir = tempfile::tempdir().unwrap();

    // 1. `start` detected the facts and wrote them to the field
    let shown = json_of(&run_ok(
        &fake,
        &b.dir,
        &["agent", "profile", "show", "--json"],
    ));
    let facts: Vec<&str> = shown["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(facts.iter().any(|f| f.starts_with("arch:")), "{facts:?}");
    assert!(facts.iter().any(|f| f.starts_with("os:")), "{facts:?}");
    assert!(facts.contains(&"tool:git"), "git is installed: {facts:?}");
    let original = caps(&fake, &b);
    assert!(
        original.contains("arch:") && original.len() <= 1024,
        "{original}"
    );

    // 2. a fact that someone types into the field does not survive the next start
    fake.set_item(
        &b.item_id,
        "Capabilities",
        &format!("{original} tool:fake-it-tool arch:riscv64"),
    );
    run_ok(&fake, &b.dir, &["start"]);
    assert_eq!(caps(&fake, &b), original, "the real facts are back");

    // 3. refusals write nothing and post nothing
    for bad in ["Bad Tag", "arch:x86_64", "tool:cargo", "UPPER", "-x"] {
        let o = set_profile(&fake, &b, &[&format!("--tag={bad}")]);
        assert_eq!(o.code, 2, "{bad}: {}", o.all());
        assert!(o.stderr.contains("profile-tag-invalid"), "{}", o.all());
    }
    let leak = dir.path().join("leak.md");
    let token: String = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ"
        .chars()
        .cycle()
        .take(36)
        .collect();
    std::fs::write(&leak, format!("my token ghp_{token}\n")).unwrap();
    let o = set_profile(&fake, &b, &["--about-file", leak.to_str().unwrap()]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("secret-in-body"), "{}", o.all());
    assert!(!o.all().contains(&token), "the secret is not echoed");
    // nothing to change is a usage error, not a silent success
    assert_eq!(set_profile(&fake, &b, &[]).code, 64);
    assert_eq!(caps(&fake, &b), original, "a refusal writes nothing");
    assert!(profile_records(&fake).is_empty(), "a refusal posts nothing");

    // 4. the agent changes its own profile: field, body, one signed record
    let about = dir.path().join("about.md");
    std::fs::write(&about, "IT agent. Builds on this machine.\nSecond line.\n").unwrap();
    let o = set_profile(
        &fake,
        &b,
        &[
            "--tag",
            "it-e2e",
            "--tag",
            "it-builds",
            "--tier",
            "mid",
            "--about-file",
            about.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("recorded (signed"), "{}", o.stdout);
    let f = caps(&fake, &b);
    assert!(
        f.starts_with("it-builds it-e2e tier:mid arch:"),
        "declared tags, tier, then the facts: {f}"
    );
    let body = fake.draft_body(&b.item_id);
    assert!(body.contains("IT agent. Builds on this machine."), "{body}");
    assert!(body.contains("Tags: it-builds, it-e2e"), "{body}");
    assert!(
        body.contains("- tools:"),
        "the full facts are in the body: {body}"
    );
    let recs = profile_records(&fake);
    assert_eq!(recs.len(), 1, "{recs:?}");
    assert!(
        recs[0].starts_with("<!-- crew v=1 from="),
        "signed: {}",
        recs[0]
    );
    // the same request again changes nothing and posts nothing
    let o = set_profile(&fake, &b, &["--tag", "it-e2e", "--tier", "mid"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("no change"), "{}", o.stdout);
    assert_eq!(profile_records(&fake).len(), 1, "no record for no change");

    // 5. a new start keeps the declared profile and the background text
    run_ok(&fake, &b.dir, &["start"]);
    assert!(caps(&fake, &b).starts_with("it-builds it-e2e tier:mid arch:"));
    assert!(
        fake.draft_body(&b.item_id)
            .contains("IT agent. Builds on this machine.")
    );

    // 6. the owner changes the profile; the owner comment is the record
    let o = run_ok(
        &fake,
        owner.path(),
        &[
            "agent",
            "profile",
            "set",
            "--agent",
            &b.name,
            "--tag",
            "it-gpu",
            "--untag",
            "it-e2e",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert!(o.stdout.contains("owner profile change"), "{}", o.stdout);
    let f = caps(&fake, &b);
    assert!(f.starts_with("it-builds it-gpu tier:mid arch:"), "{f}");
    let recs = profile_records(&fake);
    assert_eq!(recs.len(), 2, "{recs:?}");
    assert_eq!(
        recs[1],
        format!("/crew profile {} +it-gpu -it-e2e", b.name),
        "the owner comment has the exact command"
    );
    // the owner cannot write a fact either: a tag with a colon is refused
    let o = fake.run(
        owner.path(),
        &[
            "agent",
            "profile",
            "set",
            "--agent",
            &b.name,
            "--tag",
            "arch:arm64",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert_eq!(profile_records(&fake).len(), 2);

    // 7. the history lists both changes, in order, with who made them
    let h = json_of(&run_ok(
        &fake,
        &b.dir,
        &["agent", "profile", "show", "--json"],
    ));
    let hist = h["history"].as_array().unwrap();
    assert_eq!(hist.len(), 2, "{h}");
    assert_eq!(hist[0]["by"], "agent");
    assert!(
        hist[0]["change"]
            .as_str()
            .unwrap()
            .contains("+it-e2e +it-builds tier=mid"),
        "{h}"
    );
    assert_eq!(hist[1]["by"], "owner");
    assert_eq!(hist[1]["change"], "+it-gpu -it-e2e");
    assert_eq!(h["tier"], "mid");

    // 8. status shows the tags
    let st = json_of(&run_ok(
        &fake,
        owner.path(),
        &["status", "--json", "--initiative", INITIATIVE],
    ));
    let row = st["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == b.name.as_str())
        .unwrap_or_else(|| panic!("{st}"));
    assert_eq!(row["tags"], json!(["it-builds", "it-gpu"]));
    assert_eq!(row["tier"], "mid");

    // 9. back to facts only
    let empty = dir.path().join("empty.md");
    std::fs::write(&empty, "").unwrap();
    let o = set_profile(
        &fake,
        &b,
        &[
            "--untag",
            "it-builds",
            "--untag",
            "it-gpu",
            "--clear-tier",
            "--about-file",
            empty.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(caps(&fake, &b).starts_with("arch:"), "{}", caps(&fake, &b));
    assert!(fake.draft_body(&b.item_id).contains("Tags: (none)"));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// A ticket with a body of its own (the `Needs:` line is part of it) and labels.
fn ticket_with(fake: &FakeBoard, title: &str, needs_line: &str, labels: &[&str]) -> u64 {
    let body = format!("Goal: capability test.\n\n{GATE}{needs_line}\n");
    let mut all = vec!["crew:work"];
    all.extend_from_slice(labels);
    fake.ticket_issue(title, &body, &all)
}

fn assignment_body(fake: &FakeBoard, issue: u64) -> String {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("Assigned to you"))
        .unwrap_or_default()
}

/// A refused `assign` leaves the ticket as it was: no comment, no board item.
fn untouched(fake: &FakeBoard, t: u64) {
    assert!(fake.comments_of(t).is_empty(), "no comment on #{t}");
    assert!(fake.item_of_issue(t).is_none(), "no board item for #{t}");
}

#[test]
fn assign_refuses_a_capability_mismatch_and_records_an_override_with_its_reason() {
    let (fake, c, b) = crew();
    let o = set_profile(&fake, &b, &["--tag", "it-cap-ok", "--tag", "it-cap-two"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let arch = arch_of(&fake, &b);
    let assign = |t: u64, extra: &[&str]| {
        let ts = t.to_string();
        let mut v = vec!["assign", ts.as_str(), "--to", b.name.as_str()];
        v.extend_from_slice(extra);
        fake.run(&c.dir, &v)
    };

    // pass: the real architecture of B (bare form) and a declared tag
    let t1 = ticket_with(
        &fake,
        "IT caps met",
        &format!("Needs: {arch}, it-cap-ok"),
        &[],
    );
    let o = assign(t1, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("meets arch:"), "{}", o.stdout);
    assert!(!assignment_body(&fake, t1).contains("capability-override"));
    // pass: needs as labels
    let t1b = ticket_with(
        &fake,
        "IT caps met by labels",
        "",
        &["needs:it-cap-two", "needs:it-cap-ok"],
    );
    let o = assign(t1b, &[]);
    assert_eq!(o.code, 0, "{}", o.all());

    // refusal: a mismatch in the body line names the missing tags, not the met one
    let t2 = ticket_with(
        &fake,
        "IT caps missing",
        "Needs: it-cap-ok, it-cap-missing, it-gpu-nope",
        &[],
    );
    let o = assign(t2, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("capability-mismatch"), "{}", o.all());
    assert!(
        o.stderr.contains("lacks: it-cap-missing, it-gpu-nope"),
        "{}",
        o.all()
    );
    untouched(&fake, t2);
    // refusal: a missing label need
    let t3 = ticket_with(
        &fake,
        "IT caps missing label",
        "",
        &["needs:it-cap-missing"],
    );
    let o = assign(t3, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("lacks: it-cap-missing"), "{}", o.all());
    untouched(&fake, t3);
    // refusal: another architecture
    let other = if arch == "arm64" { "x86_64" } else { "arm64" };
    let t4 = ticket_with(&fake, "IT caps other arch", &format!("Needs: {other}"), &[]);
    let o = assign(t4, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(&format!("arch:{other}")), "{}", o.all());
    // refusal: a requirement that does not parse is refused, not dropped
    let t5 = ticket_with(&fake, "IT caps bad needs", "Needs: Fast CPU!", &[]);
    let o = assign(t5, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("needs-invalid"), "{}", o.all());
    untouched(&fake, t5);

    // refusal: an override without a reason, also with a blank reason
    let o = assign(t2, &["--override-capabilities"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("capability-override-needs-reason"),
        "{}",
        o.all()
    );
    let o = assign(t2, &["--override-capabilities", "--why", "  "]);
    assert_eq!(o.code, 2, "{}", o.all());
    // a reason without the flag is a usage error (exit 64)
    let o = assign(t2, &["--why", "because"]);
    assert_eq!(o.code, 64, "{}", o.all());
    untouched(&fake, t2);

    // pass: an override with a reason is recorded in the signed assignment message
    let o = assign(
        t2,
        &[
            "--override-capabilities",
            "--why",
            "owner agreed: CPU only is enough for this test",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("OVERRIDDEN"), "{}", o.stdout);
    let item = fake.item_of_issue(t2).expect("item");
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    let body = assignment_body(&fake, t2);
    assert!(
        body.contains("capability-override: it-cap-missing, it-gpu-nope"),
        "{body}"
    );
    assert!(
        body.contains("override-reason: owner agreed: CPU only is enough for this test"),
        "{body}"
    );
    assert!(body.starts_with("<!-- crew v=1 from="), "signed: {body}");
    // B receives it with the override in the body
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["events"].as_array().unwrap().iter().any(|e| e["body"]
            .as_str()
            .is_some_and(|x| x.contains("capability-override"))),
        "{v}"
    );

    // reassign --to is checked the same way: refused, then passed with the override
    let t6 = ticket_with(&fake, "IT caps reassign", "Needs: it-cap-missing", &[]);
    let o = assign(t6, &["--override-capabilities", "--why", "first assign"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t6s = t6.to_string();
    let re = [
        "reassign",
        t6s.as_str(),
        "--reason",
        "late",
        "--why",
        "retry",
        "--to",
        b.name.as_str(),
    ];
    run_refused(&fake, &c.dir, &re, "capability-mismatch");
    let mut with = re.to_vec();
    with.push("--override-capabilities");
    let o = run_ok(&fake, &c.dir, &with);
    assert!(o.stdout.contains("OVERRIDDEN"), "{}", o.stdout);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn agents_lists_the_agents_that_meet_the_needs_fewest_tickets_first() {
    let (fake, c, b) = crew();
    let a = fake.agent("claude@fake-a", Role::Dev);
    run_ok(&fake, &a.dir, &["start"]);
    let o = set_profile(&fake, &b, &["--tag", "it-agent-only-b", "--tier", "low"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let names_in = |v: &Value| -> Vec<String> {
        v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect()
    };
    let agents = |need: &str| {
        json_of(&run_ok(
            &fake,
            &c.dir,
            &["status", "--agents", "--can", need, "--json"],
        ))
    };

    // only B has the tag
    let v = agents("it-agent-only-b");
    assert_eq!(names_in(&v), std::slice::from_ref(&b.name), "{v}");
    assert_eq!(v["agents"][0]["tier"], "low");
    // an impossible need lists nobody and says why for each agent
    let v = agents("it-agent-nobody-has");
    assert!(names_in(&v).is_empty(), "{v}");
    let why = v["not_listed"].as_array().unwrap();
    for n in [&a.name, &b.name] {
        assert!(
            why.iter().any(|w| w["name"] == n.as_str()
                && w["why"]
                    .as_str()
                    .unwrap()
                    .contains("lacks it-agent-nobody-has")),
            "{n}: {v}"
        );
    }
    // a bad requirement is refused before the board is read
    let before = fake.requests().len();
    let o = fake.run(&c.dir, &["status", "--agents", "--can", "Bad!Tag"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert_eq!(fake.requests().len(), before, "no request for a bad need");

    // both dev agents have the architecture; both have load 0
    let arch = arch_of(&fake, &b);
    let load_of = |v: &Value, n: &str| -> u64 {
        v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == n)
            .unwrap_or_else(|| panic!("{n} not listed: {v}"))["load"]
            .as_u64()
            .unwrap()
    };
    let v = agents(&arch);
    assert_eq!((load_of(&v, &a.name), load_of(&v, &b.name)), (0, 0), "{v}");
    // A gets a ticket: B (load 0) comes before A (load 1)
    let t = ticket_with(&fake, "IT agents load", "", &[]);
    run_ok(&fake, &c.dir, &["assign", &t.to_string(), "--to", &a.name]);
    let v = agents(&arch);
    assert_eq!(load_of(&v, &a.name), 1, "{v}");
    assert_eq!(load_of(&v, &b.name), 0, "{v}");
    assert_eq!(names_in(&v), [b.name.clone(), a.name.clone()], "{v}");
    // B gets two: A (load 1) comes before B (load 2)
    for title in ["IT agents load b1", "IT agents load b2"] {
        let t = ticket_with(&fake, title, "", &[]);
        run_ok(&fake, &c.dir, &["assign", &t.to_string(), "--to", &b.name]);
    }
    let v = agents(&arch);
    assert_eq!(load_of(&v, &b.name), 2, "{v}");
    assert_eq!(names_in(&v), [a.name.clone(), b.name.clone()], "{v}");
    // the text form names the columns
    let o = run_ok(&fake, &c.dir, &["status", "--agents", "--can", &arch]);
    assert!(
        o.stdout.contains("load 2") && o.stdout.contains("tier low"),
        "{}",
        o.stdout
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// Set `required_tools` and `smoke_cmd` in the board config of the agent. The fake repo has no
/// `.crewboard.toml`, so `start` keeps this copy.
fn toolchain(a: &Agent, tools: &[&str], smoke: Option<&str>) {
    let cd = ConfigDir(a.dir.clone());
    let mut cfg = cd.load_agent(INITIATIVE).unwrap();
    cfg.board.required_tools = tools.iter().map(|s| RequiredTool::all(*s)).collect();
    cfg.board.smoke_cmd = smoke.map(str::to_string);
    cd.save_agent(&cfg).unwrap();
}

fn toolchain_flag(fake: &FakeBoard, owner: &std::path::Path, name: &str) -> Value {
    let v = json_of(&run_ok(
        fake,
        owner,
        &["status", "--json", "--initiative", INITIATIVE],
    ));
    v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .unwrap_or_else(|| panic!("{v}"))["toolchain_unreachable"]
        .clone()
}

#[test]
fn start_refuses_an_agent_that_cannot_run_the_repo_tools_and_status_shows_the_note() {
    let (fake, _c, b) = crew();
    let owner = fake.owner_dir();
    assert_ne!(toolchain_flag(&fake, owner.path(), &b.name), true);

    // refusal: a tool that does not exist
    toolchain(&b, &["crewboard-no-such-tool-xyz --version"], None);
    let o = run_refused(&fake, &b.dir, &["start"], "toolchain-unreachable");
    assert!(
        o.stderr.contains("crewboard-no-such-tool-xyz"),
        "{}",
        o.all()
    );
    assert!(
        caps(&fake, &b).contains("toolchain:unreachable"),
        "{}",
        caps(&fake, &b)
    );
    assert_eq!(toolchain_flag(&fake, owner.path(), &b.name), true);
    // refusal: the tool exists but the smoke command fails
    toolchain(&b, &["exit 0"], Some("exit 1"));
    run_refused(&fake, &b.dir, &["start"], "toolchain-unreachable");
    // pass: tools and smoke command run; the note is removed
    toolchain(&b, &["exit 0"], Some("exit 0"));
    let o = run_ok(&fake, &b.dir, &["start"]);
    assert!(o.stdout.contains("toolchain: 2 command(s)"), "{}", o.stdout);
    assert!(
        !caps(&fake, &b).contains("toolchain:unreachable"),
        "{}",
        caps(&fake, &b)
    );
    assert_ne!(toolchain_flag(&fake, owner.path(), &b.name), true);
    // no keys: no check
    toolchain(&b, &[], None);
    let o = run_ok(&fake, &b.dir, &["start"]);
    assert!(!o.stdout.contains("toolchain:"), "{}", o.stdout);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// Set `required_tools` to these entries in the board config copy of the agent.
fn toolchain_entries(a: &Agent, tools: Vec<RequiredTool>) {
    let cd = ConfigDir(a.dir.clone());
    let mut cfg = cd.load_agent(INITIATIVE).unwrap();
    cfg.board.required_tools = tools;
    cfg.board.smoke_cmd = None;
    cd.save_agent(&cfg).unwrap();
}

#[test]
fn start_checks_a_tagged_tool_only_for_an_agent_with_the_tag() {
    let (fake, _c, b) = crew();
    let list = vec![
        RequiredTool::all("exit 0"),
        RequiredTool::tagged("crewboard-no-such-tool-xyz --version", &["rust"]),
    ];
    // no tag: the tagged tool is skipped, the start passes, and the output names both
    toolchain_entries(&b, list.clone());
    let o = run_ok(&fake, &b.dir, &["start"]);
    assert!(
        o.stdout.contains("toolchain: 1 command(s)")
            && o.stdout.contains("checked: `exit 0`")
            && o.stdout.contains(
                "skipped by tag: `crewboard-no-such-tool-xyz --version` (needs tag rust)"
            ),
        "{}",
        o.stdout
    );
    // another tag does not unlock it either
    run_ok(&fake, &b.dir, &["agent", "profile", "set", "--tag", "docs"]);
    run_ok(&fake, &b.dir, &["start"]);
    // control: with the tag the same tool is checked and the start is refused
    run_ok(&fake, &b.dir, &["agent", "profile", "set", "--tag", "rust"]);
    let o = run_refused(&fake, &b.dir, &["start"], "toolchain-unreachable");
    assert!(
        o.stderr.contains("crewboard-no-such-tool-xyz"),
        "{}",
        o.all()
    );
    // with the tag and a tool that works: checked, nothing skipped
    toolchain_entries(
        &b,
        vec![
            RequiredTool::all("exit 0"),
            RequiredTool::tagged("exit 0", &["rust"]),
        ],
    );
    let o = run_ok(&fake, &b.dir, &["start"]);
    assert!(
        o.stdout.contains("toolchain: 2 command(s)") && !o.stdout.contains("skipped by tag: `"),
        "{}",
        o.stdout
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
