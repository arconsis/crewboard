//! The repeated-run gate (`crewboard repeat`): run a command N times in a row and keep honest
//! evidence. This module is pure text and judgement: the verdict of one run from its exit code,
//! its own log text and the two patterns, and the evidence block. `ops::gate_cmd` runs the
//! processes.

pub mod evidence;
pub mod verdict;
