//! CI first and `--gate` as the real binary. `HOST_LOCK_PS_CMD` plays the
//! process list: a `Runner.Worker` line is a CI job on this host. Ids are those of the shell suite.
#![cfg(unix)]

use crate::support::lock::*;

fn take(r: &Rig, owner: &str, extra: &[&str]) -> Out {
    let l = r.s("l");
    let mut a = vec!["take", l.as_str()];
    let live = r.live().to_string();
    a.extend(["--holder", owner, "--no-board", "--pid", &live]);
    a.extend_from_slice(extra);
    r.run(&a)
}

#[test]
fn h13_a_running_runner_worker_keeps_an_agent_from_a_free_lock_76_without_ci_it_takes() {
    let r = Rig::new();
    r.set_runner(true);
    let o = take(&r, "a@x", &[]);
    assert_eq!(o.code, 76, "{}", o.all());
    assert!(!r.p("l").exists());
    assert!(o.stderr.contains("CI first"), "{}", o.stderr);
    // no CI, the same take works
    r.set_runner(false);
    let o = take(&r, "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(r.p("l/owner").exists());
}

#[test]
fn h13c_the_agent_waits_while_runner_worker_runs_and_goes_on_when_it_is_gone() {
    let r = Rig::new();
    r.set_runner(true);
    let (l, live, err) = (r.s("l"), r.live().to_string(), r.p("take.stderr"));
    let mut child = r
        .cmd(&[
            "take",
            &l,
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            &live,
            "--wait",
            "20",
            "--poll",
            "0.3",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    // the take has seen the CI job and waits: only then the job goes away
    let said = || std::fs::read_to_string(&err).is_ok_and(|t| t.contains("CI first"));
    assert!(wait_for(20, said), "the take did not wait for the CI job");
    r.set_runner(false);
    let code = child.wait().unwrap().code();
    let text = std::fs::read_to_string(&err).unwrap_or_default();
    assert_eq!(code, Some(0), "{text}");
    assert!(r.p("l/owner").exists());
}

#[test]
fn h14_a_waiting_list_entry_blocks_an_agent_76_and_ci_status_says_so() {
    let r = Rig::new();
    let l = r.s("l");
    let o = r.run(&["ci-wait", &l, "--holder", "ci-job-1"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(r.p("l.waiting/ci-job-1").is_file());
    assert!(
        o.stderr.contains("registered CI entry ci-job-1"),
        "{}",
        o.stderr
    );
    let o = take(&r, "a@x", &[]);
    assert_eq!(o.code, 76, "{}", o.all());
    assert!(!r.p("l").exists());
    let s = r.run(&["ci-status", &l]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(
        s.stdout.contains("waiting list entry ci-job-1"),
        "{}",
        s.stdout
    );
}

#[test]
fn h14b_ci_done_frees_it_ci_status_is_1_then_the_agent_takes_and_the_list_is_gone() {
    let r = Rig::new();
    let l = r.s("l");
    r.run(&["ci-wait", &l, "--holder", "ci-job-1"]);
    assert_eq!(r.run(&["ci-status", &l]).code, 0);
    assert_eq!(r.run(&["ci-done", &l, "--holder", "ci-job-1"]).code, 0);
    assert_eq!(r.run(&["ci-status", &l]).code, 1);
    let o = take(&r, "a@x", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!r.p("l.waiting").exists());
}

fn gt_run(r: &Rig, name: &str, extra: &[&str], cmd: &[&str]) -> Out {
    let l = r.s(name);
    let mut a = vec!["run", l.as_str(), "--holder", "a@hostA", "--no-board"];
    a.extend_from_slice(extra);
    a.push("--");
    a.extend_from_slice(cmd);
    r.run(&a)
}

#[test]
fn gt1_a_ci_job_runs_plain_run_is_76_run_gate_runs_the_job() {
    let r = Rig::new();
    r.set_runner(true);
    let (plain, gate) = (r.s("ran.plain"), r.s("ran.gate"));
    let o = gt_run(&r, "l7", &[], &["touch", &plain]);
    assert_eq!(o.code, 76, "{}", o.all());
    assert!(!r.p("ran.plain").exists());
    let o = gt_run(&r, "l7", &["--gate"], &["touch", &gate]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(r.p("ran.gate").exists() && !r.p("l7").exists());
}

#[test]
fn gt2_gate_still_waits_for_a_held_lock_75() {
    let r = Rig::new();
    r.set_runner(true);
    r.plant("l8", "b", HOST, r.live(), now());
    let o = gt_run(&r, "l8", &["--gate"], &["true"]);
    assert_eq!(o.code, 75, "{}", o.all());
}

fn wait_result(path: &std::path::Path) -> bool {
    wait_for(15, || path.exists())
}

#[test]
fn gt3_gate_with_detach_skips_the_ci_wait() {
    let r = Rig::new();
    r.set_runner(true);
    let res = r.s("r9");
    let o = gt_run(
        &r,
        "l9",
        &["--gate", "--detach", "--result", &res],
        &["sh", "-c", "exit 0"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(wait_result(&r.p("r9")));
    assert!(
        std::fs::read_to_string(r.p("r9"))
            .unwrap()
            .starts_with("rc=0 ")
    );
}

#[test]
fn gt4_detach_without_gate_waits_for_ci_first_76_nothing_started() {
    let r = Rig::new();
    r.set_runner(true);
    let res = r.s("r10");
    let o = gt_run(&r, "l10", &["--detach", "--result", &res], &["true"]);
    assert_eq!(o.code, 76, "{}", o.all());
    assert!(!r.p("l10").exists() && !r.p("r10").exists());
}
