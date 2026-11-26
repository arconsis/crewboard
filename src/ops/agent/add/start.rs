//! Step 7 of `agent add`: `start` as the agent, and the wait for the approval.

use super::capture::{Captured, STEP_LIMIT, exit_of, run_captured};
use super::next::waiting_lines;
use super::output::{Fail, PREFIX, Run};
use super::plan::{Plan, seam_bin};
use crate::clock::SystemClock;
use crate::ops::agent::exec_env_plan;
use crate::ops::agent::shim::sq;
use crate::ops::agent::wait::{Wait, note_interrupt, poll_until};
use std::path::PathBuf;
use std::process::Command;

/// The binary that runs `start` (and `join` with the seam): the seam, else this one.
fn self_bin(p: &Plan) -> Result<PathBuf, Fail> {
    match seam_bin(p) {
        Some(b) => Ok(PathBuf::from(b)),
        None => std::env::current_exe().map_err(|e| Fail {
            step: 7,
            name: "start",
            what: format!("cannot find the crewboard binary: {e}"),
            fix: "run crewboard by its full path".into(),
            code: 4,
        }),
    }
}

pub(super) enum Start {
    Done,
    NotApproved,
    Failed(Fail),
}

/// Is this the refusal of `start` for an agent that the owner has not approved yet? It also
/// covers the moment inside `approve`: the signed approval is posted, and the item State still
/// says `requested` until `approve` writes `approved` a second later.
pub(super) fn is_not_approved(output: &str) -> bool {
    output.contains("rule=agent-approval-pending")
        || (output.contains("rule=sender-not-approved")
            && (output.contains("has not approved it yet")
                || output.contains("item State is 'requested'")))
}

/// Run `start` once as the agent, in the work dir, with the environment that the runtime gets.
pub(super) fn start_once(p: &Plan) -> Result<Captured, Fail> {
    let bin = self_bin(p)?;
    let abs = &p.agent_dir;
    let mut c = Command::new(&bin);
    // a shell sets PWD to the path as typed; the child shows the same path as the caller
    c.arg("start")
        .current_dir(&p.work_dir)
        .env("PWD", &p.work_dir);
    for (k, v) in exec_env_plan(
        abs,
        abs.join("bin").is_dir(),
        abs.join("gh-empty-config").is_dir(),
        std::env::var_os("PATH").as_deref(),
    ) {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    Ok(run_captured(c, STEP_LIMIT))
}

/// What the output of `start` means: done, not approved yet, or a failed step 7.
pub(super) fn judge_start(p: &Plan, cap: &Captured) -> Start {
    if cap.code == 0 && !cap.timed_out {
        return Start::Done;
    }
    if is_not_approved(&cap.text) {
        return Start::NotApproved;
    }
    Start::Failed(Fail {
        step: 7,
        name: "start",
        what: format!("crewboard start exit {}", cap.code),
        fix: format!(
            "see which setup check fails: cd {} && crewboard agent exec --agent-dir {} -- crewboard doctor --setup",
            sq(&p.work_dir.display().to_string()),
            sq(&p.agent_dir.display().to_string())
        ),
        code: exit_of(cap.code),
    })
}

/// Step 7: `start` as the agent.
pub(super) fn step_start(p: &Plan, run: &mut Run) -> Start {
    match start_once(p) {
        Ok(cap) => {
            run.record("start", &cap.text);
            judge_start(p, &cap)
        }
        Err(f) => Start::Failed(f),
    }
}

/// Print what the owner must do, then poll `start` until the approval is there. `NotApproved`
/// means the time ran out.
pub(super) fn wait_for_approval(p: &Plan, run: &mut Run, initiative: &str, w: &Wait) -> Start {
    for l in waiting_lines(p, initiative, w) {
        println!("{l}");
    }
    note_interrupt();
    let end = poll_until(
        w,
        &SystemClock,
        |n| {
            let cap = match start_once(p) {
                Ok(c) => c,
                Err(f) => return Some(Start::Failed(f)),
            };
            let verdict = judge_start(p, &cap);
            // a refusal that repeats is kept once in the log; the answer that ends the wait is kept
            if n == 0 || !matches!(verdict, Start::NotApproved) {
                run.record("start", &cap.text);
            }
            (!matches!(verdict, Start::NotApproved)).then_some(verdict)
        },
        |minutes| {
            println!(
                "{PREFIX} still waiting for the approval of {} ({minutes} min)",
                p.name
            );
        },
    );
    end.unwrap_or_else(|| {
        println!(
            "{PREFIX} gave up after {} min: the approval did not come",
            w.max.as_secs() / 60
        );
        Start::NotApproved
    })
}
