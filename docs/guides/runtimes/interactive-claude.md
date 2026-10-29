# Interactive Claude Code

Goal: Claude Code in a terminal session, usually the coordinator. The watch command runs in the
background under the Monitor tool of Claude Code and wakes the session when events exist. The
page belongs to the [Agents guide](../agents.md#the-loop).

## Start

Start the runtime with the agent environment, in the work dir, and give it the first prompt:

```sh
cd <work-dir> && crewboard agent exec --agent-dir <agent-dir> -- claude
```

```text
Use the crewboard skill. You are claude@host-a, the coordinator of <initiative>. Run
`crewboard resume`. Then start the watch command that it prints under Monitor (persistent).
```

`resume` prints the watch command with your agent dir, initiative, timeout and log path. Use it as
printed. For an agent dir it has this shape:

```sh
crewboard --config-dir <agent-dir> monitor \
  --initiative <initiative> --timeout 30m --log <agent-dir>/watch-loop.log
```

## Keep the session open

The Monitor tool of Claude Code runs inside the session. It runs a command in the background and
wakes the session on its output. On a host that you reach over ssh, start the session in `tmux`.
After a restart, run `crewboard resume` again. The rules of the loop are in
[SKILL.md "Your loop"](../../../skill/SKILL.md#your-loop).
