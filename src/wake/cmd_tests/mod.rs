//! The tests of `crewboard wake`, one file per topic. `fakes` and `rig` hold what the topics
//! share.

use super::*;
use crate::clock::FakeClock;
use crate::lock::procs::Procs;
use crate::wake::describe_head;
use crate::wake::flow::{Flow, Reason};
use crate::wake::lockdir::set_mtime;
use crate::wake::policy::{NetBackoff, Quiet, WorkBackoff};
use crate::wake::preflight::config_dir_of_command;
use crate::wake::procs::{Os, Proc, descendant_procs, procs_of, same_processes, stop_group_with};
use crate::wake::state;
use crate::wake::steps::{OpenPrRef, PrHead, StepRc};
use crate::wake::wake_events::with_budget_header;
use clap::Parser;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const NOW: i64 = 1_700_000_000;

const TICKOUT: &str = "0|tick: verified and merged demo/app#5\\nleft for the interactive coordinator: demo/app#6, the Gate is not CI only|";

const MINE12: &str = "demo/app#12                        In Progress      P1  order 1\n";

#[derive(Parser)]
struct P {
    #[command(flatten)]
    w: WakeArgs,
}

fn parse(argv: &[&str]) -> WakeArgs {
    let mut v = vec!["wake"];
    v.extend_from_slice(argv);
    P::try_parse_from(v).unwrap().w
}

mod budget;
mod env_file;
mod events_file;
mod fakes;
mod heartbeats;
mod host_locks;
mod legacy;
mod more_cases;
mod options;
mod precheck;
mod receipts;
mod results;
mod retry;
mod rig;
mod stop;
mod tick;
mod values;
mod wake_lock;

use fakes::*;
use rig::*;
