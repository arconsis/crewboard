# No-model coordinator

A no-model coordinator (`start` prints a `scope:` line) runs without a model:
`crewboard wake --tick` runs `crewboard tick`.

`tick` merges only a ticket whose signed Gate is `ci` (the body line `Gate: ci`). It leaves every
other Gate for the interactive coordinator.

If a model runs in this config dir anyway, do nothing on the board. Tell the owner to use that
command.
