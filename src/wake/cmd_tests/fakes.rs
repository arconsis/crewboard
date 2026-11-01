//! The scripted calls to the board of a run.

use super::*;

/// The calls of a run, scripted: one result per call, in order. `mine` has a plan of its own.
/// An empty plan gives exit 1.
#[derive(Default)]
pub(super) struct Scripted {
    pub(super) plan: VecDeque<(u8, String, String)>,
    pub(super) mine: VecDeque<(u8, String)>,
    pub(super) calls: Vec<String>,
    /// Runs at the start of each call that pops the plan (a test sets a signal here).
    pub(super) hook: Option<Box<dyn Fn()>>,
    /// The answer of `acted --list`, and the receipts that `acted --since` gives (one per
    /// line); `acted_rc` is the exit code of both.
    pub(super) acted_list: String,
    pub(super) acted_check: Vec<String>,
    pub(super) acted_rc: u8,
    pub(super) acted_calls: Vec<String>,
    /// The heads of PRs by `<repo>#<n>`, and the open PRs of every repo.
    pub(super) heads: std::collections::HashMap<String, PrHead>,
    pub(super) open: Vec<OpenPrRef>,
}

impl Scripted {
    pub(super) fn new(plan: &[&str]) -> Scripted {
        let mut s = Scripted::default();
        for l in plan {
            s.push(l);
        }
        s
    }

    /// `<code>|<stdout>|<stderr>`
    pub(super) fn push(&mut self, l: &str) {
        let mut it = l.splitn(3, '|');
        let code = it.next().unwrap().parse().unwrap();
        let mut out = it.next().unwrap_or("").replace("\\n", "\n");
        if !out.is_empty() {
            out.push('\n');
        }
        let err = it.next().unwrap_or("").to_string();
        self.plan.push_back((code, out, err));
    }

    pub(super) fn with_mine(mut self, plan: &[(u8, &str)]) -> Scripted {
        self.mine = plan.iter().map(|(c, o)| (*c, o.to_string())).collect();
        self
    }

    pub(super) fn next(&mut self, out: &mut Vec<u8>) -> StepRc {
        if let Some(h) = &self.hook {
            h();
        }
        let (code, o, e) = self
            .plan
            .pop_front()
            .unwrap_or((1, String::new(), String::new()));
        out.extend_from_slice(o.as_bytes());
        StepRc::from_stderr(code, &e)
    }

    pub(super) fn count(&self, prefix: &str) -> usize {
        self.calls.iter().filter(|c| c.starts_with(prefix)).count()
    }
}

impl Steps for Scripted {
    fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc {
        self.calls.push(if quiet {
            "poll --quiet".into()
        } else {
            "poll".into()
        });
        self.next(out)
    }
    fn mine_quiet(&mut self) -> StepRc {
        self.calls.push("mine --quiet".into());
        let (c, _) = self.mine.pop_front().unwrap_or((1, String::new()));
        StepRc {
            code: c,
            msg: String::new(),
        }
    }
    fn mine_text(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.calls.push("mine".into());
        let (c, o) = self.mine.pop_front().unwrap_or((1, String::new()));
        out.extend_from_slice(o.as_bytes());
        StepRc {
            code: c,
            msg: String::new(),
        }
    }
    fn say(&mut self, ticket: &str, body: &str, anyway: bool) -> StepRc {
        self.calls
            .push(format!("say {ticket} anyway={anyway} {body}"));
        self.next(&mut Vec::new())
    }
    fn tick(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.calls.push("tick".into());
        self.next(out)
    }
    fn acted_list(&mut self, events: &Path, out: &mut Vec<u8>) -> StepRc {
        self.acted_calls
            .push(format!("--events-file {} --list", events.display()));
        if self.acted_rc != 0 {
            return StepRc::from_stderr(self.acted_rc, "error: unrecognized subcommand 'acted'");
        }
        out.extend_from_slice(self.acted_list.as_bytes());
        StepRc::ok()
    }
    fn acted_check(
        &mut self,
        events: &Path,
        since: &str,
        redeliver: &Path,
        out: &mut Vec<u8>,
    ) -> StepRc {
        self.acted_calls.push(format!(
            "--events-file {} --since {since} --redeliver-to {}",
            events.display(),
            redeliver.display()
        ));
        if self.acted_rc != 0 {
            return StepRc::from_stderr(self.acted_rc, "error: unrecognized subcommand 'acted'");
        }
        let text = std::fs::read_to_string(events).unwrap_or_default();
        for r in self.acted_check.clone() {
            if text.lines().any(|l| l == format!("REDELIVERED {r}")) {
                out.extend_from_slice(format!("{r} twice\n").as_bytes());
            } else {
                out.extend_from_slice(format!("{r} again\n").as_bytes());
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(redeliver)
                    .unwrap();
                write!(f, "REDELIVERED {r}\nblock of {r}\n\n").unwrap();
            }
        }
        StepRc::ok()
    }

    fn heartbeat(&mut self) -> StepRc {
        self.calls.push("heartbeat".into());
        StepRc::ok()
    }
    fn pr_head(&mut self, repo: &str, number: u64) -> Result<Option<PrHead>, StepRc> {
        self.calls.push(format!("pr_head {repo}#{number}"));
        Ok(self.heads.get(&format!("{repo}#{number}")).cloned())
    }
    fn open_prs(&mut self, repo: &str) -> Result<Vec<OpenPrRef>, StepRc> {
        self.calls.push(format!("open_prs {repo}"));
        Ok(self.open.clone())
    }
}
