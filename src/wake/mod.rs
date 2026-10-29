//! The parts of `crewboard wake`, the headless wake gate: the rules (`policy`), the files of the
//! state dir (`state`), the events file of a wake (`events`), the result files of detached jobs
//! (`results`), the event receipts (`receipts`), the wake lock (`lockdir`) and the calls to the
//! board (`steps`), the wake command of a dev agent for each runtime (`adapters`, `words`), and
//! the events hook of a Claude wake (`hook`). `settings` reads the options once into plain data;
//! `cmd` makes the calls and the log and runs the flow; `flow` is the run as a sequence of
//! phases (`preflight`, `gate`, the lock, `tick` or `wake_events` and `finish`, with `runtime`
//! and `procs` for the wake command); `hook_cmd` runs the hook; `job` builds the job of a
//! scheduler.
//!
//! `steps`, `cmd`, `finish` and `hook_cmd` reach into `ops` (the wake calls the commands of the agent);
//! `ops` does not import `wake`.

pub mod adapters;
pub mod cmd;
pub mod describe_head;
pub mod events;
pub mod finish;
pub mod flow;
pub mod gate;
pub mod heartbeat;
pub mod hook;
pub mod hook_cmd;
pub mod install;
pub mod job;
pub mod lockdir;
pub mod policy;
pub mod preflight;
pub mod procs;
pub mod receipts;
pub mod results;
pub mod runtime;
pub mod settings;
pub mod state;
pub mod steps;
pub mod tick;
pub mod wake_events;
pub mod words;
