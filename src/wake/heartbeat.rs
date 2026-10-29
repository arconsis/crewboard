//! The heartbeat of a long wake. The scheduler does not tick while a wake runs, so the wake sends
//! `crewboard heartbeat` every `--heartbeat-every` seconds, from its own thread with its own
//! board calls, and stops with the wake: [`Control::stop`] returns after the last call. A failed
//! heartbeat is a line for the log, not an end of the wake.

use super::steps::Steps;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// What the loop waited for.
pub enum Wait {
    /// The interval passed.
    Elapsed,
    /// The wake ended.
    Stop,
}

/// The loop: wait, send, until the wait says stop. `report` gets one line for each failed call.
pub fn run(wait: &mut dyn FnMut() -> Wait, steps: &mut dyn Steps, report: &mut dyn FnMut(String)) {
    while let Wait::Elapsed = wait() {
        let rc = steps.heartbeat();
        if rc.code != 0 {
            report(format!("heartbeat failed: exit {}: {}", rc.code, rc.msg));
        }
    }
}

/// The wait of a thread: the stop channel, with the interval as its timeout.
pub fn wait_on(stop: &Receiver<()>, every: Duration) -> Wait {
    match stop.recv_timeout(every) {
        Err(RecvTimeoutError::Timeout) => Wait::Elapsed,
        Ok(()) | Err(RecvTimeoutError::Disconnected) => Wait::Stop,
    }
}

/// The two ends that the wake keeps: stop the thread, and read what it reports.
pub struct Control {
    stop: Sender<()>,
    pub reports: Receiver<String>,
}

impl Control {
    /// The channels: the ends for the thread and the ends for the wake.
    pub fn new() -> (Control, Receiver<()>, Sender<String>) {
        let (stop, stop_rx) = mpsc::channel();
        let (report_tx, reports) = mpsc::channel();
        (Control { stop, reports }, stop_rx, report_tx)
    }

    /// Tell the thread to stop. It may be in a call; the caller joins it.
    pub fn stop(&self) {
        // the thread may be gone already; then there is nobody to tell
        self.stop.send(()).ok(); // the receiver may be gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::steps::StepRc;
    use std::path::Path;

    /// Counts the heartbeats; answers with the scripted codes.
    struct Counting {
        beats: usize,
        codes: Vec<u8>,
    }

    impl Steps for Counting {
        fn heartbeat(&mut self) -> StepRc {
            let c = self.codes.get(self.beats).copied().unwrap_or(0);
            self.beats += 1;
            StepRc {
                code: c,
                msg: if c == 0 {
                    String::new()
                } else {
                    "refused".into()
                },
            }
        }
        fn poll(&mut self, _: bool, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn mine_quiet(&mut self) -> StepRc {
            unreachable!()
        }
        fn mine_text(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn say(&mut self, _: &str, _: &str, _: bool) -> StepRc {
            unreachable!()
        }
        fn tick(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_list(&mut self, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn pr_head(&mut self, _: &str, _: &str) -> Result<Option<(String, String)>, StepRc> {
            unreachable!()
        }
        fn open_prs(&mut self, _: &str) -> Result<Vec<(String, String, String)>, StepRc> {
            unreachable!()
        }
    }

    #[test]
    fn p12_one_heartbeat_for_each_elapsed_interval_and_none_after_the_stop() {
        let mut steps = Counting {
            beats: 0,
            codes: vec![],
        };
        let mut waits = vec![Wait::Elapsed, Wait::Elapsed, Wait::Elapsed, Wait::Stop];
        waits.reverse();
        let mut reports = Vec::new();
        run(
            &mut || waits.pop().unwrap_or(Wait::Stop),
            &mut steps,
            &mut |m| reports.push(m),
        );
        assert_eq!(steps.beats, 3);
        assert!(reports.is_empty());
        // twin: a wake that ends before the first interval sends none
        let mut steps = Counting {
            beats: 0,
            codes: vec![],
        };
        run(&mut || Wait::Stop, &mut steps, &mut |_| {});
        assert_eq!(steps.beats, 0);
    }

    #[test]
    fn a_failed_heartbeat_is_reported_and_the_loop_goes_on() {
        let mut steps = Counting {
            beats: 0,
            codes: vec![3, 0],
        };
        let mut waits = vec![Wait::Elapsed, Wait::Elapsed, Wait::Stop];
        waits.reverse();
        let mut reports = Vec::new();
        run(&mut || waits.pop().unwrap(), &mut steps, &mut |m| {
            reports.push(m)
        });
        assert_eq!(steps.beats, 2);
        assert_eq!(reports, ["heartbeat failed: exit 3: refused"]);
    }

    #[test]
    fn the_thread_waits_on_the_channel_and_stops_at_once_without_a_last_beat() {
        let (ctl, stop_rx, tx) = Control::new();
        let h = std::thread::spawn(move || {
            let mut steps = Counting {
                beats: 0,
                codes: vec![],
            };
            run(
                &mut || wait_on(&stop_rx, Duration::from_secs(60)),
                &mut steps,
                &mut |m| {
                    let _ = tx.send(m);
                },
            );
            steps.beats
        });
        ctl.stop();
        assert_eq!(
            h.join().unwrap(),
            0,
            "a stop in the interval gives no heartbeat"
        );
        assert!(ctl.reports.try_recv().is_err());
        // an interval that passes gives a heartbeat before the stop
        let (ctl, stop_rx, tx) = Control::new();
        let (waited_tx, waited) = mpsc::channel();
        let h = std::thread::spawn(move || {
            let mut steps = Counting {
                beats: 0,
                codes: vec![],
            };
            run(
                &mut || {
                    let w = wait_on(&stop_rx, Duration::from_millis(20));
                    waited_tx.send(()).ok(); // the test may have stopped listening
                    w
                },
                &mut steps,
                &mut |m| {
                    let _ = tx.send(m);
                },
            );
            steps.beats
        });
        // the second wait of the thread has ended: the first interval has had its heartbeat
        for _ in 0..2 {
            waited.recv_timeout(Duration::from_secs(20)).unwrap();
        }
        ctl.stop();
        assert!(h.join().unwrap() >= 1);
    }
}
