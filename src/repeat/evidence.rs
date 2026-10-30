//! The evidence block of `repeat`: text that a person pastes into a PR. Each line of it is built
//! here, so that the wording stays in one place.

/// The block, built run by run.
#[derive(Debug, Default)]
pub struct Block(String);

impl Block {
    pub fn new() -> Block {
        Block::default()
    }

    fn add(&mut self, line: &str) {
        self.0.push_str(line);
        self.0.push('\n');
    }

    /// The command did not start: CI did not finish in time.
    pub fn ci_timeout(&mut self, n: usize, secs: u64, ci: &str) {
        self.add(&format!("run {n}: not run, paused {secs} s for CI: {ci}"));
    }

    /// The run waited for CI and went on.
    pub fn ci_pause(&mut self, n: usize, secs: u64) {
        self.add(&format!("run {n}: paused {secs} s for CI, then went on"));
    }

    /// The command did not start: HEAD is not the commit of the first run.
    pub fn head_changed(&mut self, n: usize, now: &str, first: &str) {
        self.add(&format!(
            "run {n}: not run, HEAD is {now}, the first run had {first}"
        ));
    }

    /// The first line of a run that ran: the commit after it, the exit code and the log.
    pub fn ran(&mut self, n: usize, sha: &str, rc: i32, log: &str) {
        self.add(&format!("run {n}: sha={sha} exit={rc} log={log}"));
    }

    /// The run was judged by the exit code alone.
    pub fn judged_by_exit_code(&mut self) {
        self.add("  (judged by the exit code; no --result-re)");
    }

    /// The result lines of the run, indented two spaces, or a note that there is none.
    pub fn results(&mut self, lines: &[String]) {
        if lines.is_empty() {
            self.add("  (no result line)");
        }
        for l in lines {
            self.add(&format!("  {l}"));
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The report on stdout: the evidence block between its header and the verdict, then the log dir.
pub fn render(cmd: &str, block: &Block, verdict: &str, log_dir: &str) -> String {
    format!(
        "gate-loop: evidence (cmd: {cmd})\n{}gate-loop: {verdict}\ngate-loop: logs in {log_dir}\n",
        block.as_str()
    )
}

/// The command as one line for the header: words with a blank or a shell character are quoted.
pub fn display_cmd(argv: &[String]) -> String {
    argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

fn quote(a: &str) -> String {
    let plain = !a.is_empty()
        && a.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:=@%+,-".contains(&b));
    if plain {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_green_run_with_result_lines() {
        let mut b = Block::new();
        b.ran(1, "abc", 0, "/l/run-1.log");
        b.results(&["test result: ok. 5 passed".to_string()]);
        assert_eq!(
            render("cargo test", &b, "PASS: 1 of 1 runs green on abc", "/l"),
            "gate-loop: evidence (cmd: cargo test)\n\
             run 1: sha=abc exit=0 log=/l/run-1.log\n\
             \x20 test result: ok. 5 passed\n\
             gate-loop: PASS: 1 of 1 runs green on abc\n\
             gate-loop: logs in /l\n"
        );
    }

    #[test]
    fn the_notes_of_a_run_without_result_lines() {
        let mut b = Block::new();
        b.judged_by_exit_code();
        b.results(&[]);
        assert_eq!(
            b.as_str(),
            "  (judged by the exit code; no --result-re)\n  (no result line)\n"
        );
    }

    #[test]
    fn the_lines_of_a_run_that_did_not_start() {
        let mut b = Block::new();
        b.ci_pause(1, 12);
        b.ci_timeout(2, 30, "a CI job runs on this host (Runner.Worker)");
        b.head_changed(3, "def", "abc");
        assert_eq!(
            b.as_str(),
            "run 1: paused 12 s for CI, then went on\n\
             run 2: not run, paused 30 s for CI: a CI job runs on this host (Runner.Worker)\n\
             run 3: not run, HEAD is def, the first run had abc\n"
        );
    }

    #[test]
    fn the_command_header_quotes_what_a_shell_would() {
        let a = |v: &[&str]| display_cmd(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["cargo", "test", "--locked"]), "cargo test --locked");
        assert_eq!(
            a(&["bash", "-c", "echo hi; exit 1"]),
            "bash -c 'echo hi; exit 1'"
        );
        assert_eq!(a(&["echo", "it's"]), r"echo 'it'\''s'");
        assert_eq!(a(&["x", ""]), "x ''");
    }
}
