# Shared hosts: heavy jobs

This applies only on a host with a heavy-job lock. The owner or the wake prompt names its lock
dir. A heavy job loads the CPU for minutes: a full build, a stress Gate, a load test. On a shared
host it can make the CI runs of other PRs fail. `crewboard lock --help` has the full text.

## Before a heavy job

1. Check the load. If it is higher than the number of cores, wait and check again.
2. Run the job under the lock. Do not change the form of the command:
   `crewboard lock run <lock-dir> --holder <your agent> --reason "<job>" --wait 3600 -- nice -n 19 <test command>`
3. Give the job its variables after `--`: `-- env NAME=value <command>`.
4. Use a fixed job count and your own build dir. Two agents with one build dir block each other.
5. Say in the `done` note that you ran a heavy job, and when.
6. A Gate of N runs: `crewboard repeat --times N --lock <lock-dir> --holder <your agent> -- <command>`.

## A job longer than the wake

The wake ends at its time budget and would kill the job. Add
`--detach --result $CREWBOARD_RESULTS_DIR/<n>.result --ticket <n>` to the `run` command, and end
the wake. The next wake gets the event
`detached job finished: ticket <n> rc=<rc> log=<log> result=<file>`.

- `rc=0`: run `done` once, with a short `--local-run` line (at most 400 bytes).
- `rc` not 0: fix the cause.
- Do not start the job again while the lock names it.

## The lock

- Use only `crewboard lock`. Never make, write or remove a lock dir by hand.
- A lock looks stale: `crewboard lock status <lock-dir>`, then `crewboard lock break <lock-dir> --if-dead`.
- The lock dir is outside your sandbox: `crewboard block <n> --why "no host lock reachable from my sandbox"`. Never guess a path.
