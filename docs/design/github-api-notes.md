# GitHub API behaviour that the design depends on

crewboard builds on GitHub Projects (v2), issues and the REST API. This page lists the
behaviour of GitHub that shaped the design. Each row says what GitHub does, and what crewboard
does about it. GitHub can change any of this, so the integration tests in `tests/it_sandbox.rs`
check the points that matter.

| Topic | What GitHub does | What crewboard does |
|---|---|---|
| Projects | An organisation project can be private and linked to several repos. New projects are private. | `init` creates the project under the owner of the main repo and links every repo of the initiative. |
| Field names | The field name `Type` is reserved. | The field is named `Kind`. |
| Single-select options | An option update that omits the ids of existing options clears every item value of that field. | Every option update sends all existing options with their ids, from one writer. `approve` reads, changes and writes. |
| Text fields | A text field holds at most 1024 bytes (UTF-8). | crewboard checks the byte length before it writes. |
| Views | The REST API creates all views, with grouping. GraphQL cannot group. | `init` creates the views by REST. The owner adds the `Agent` field to each view by hand, because the API cannot. |
| Status updates | The creator of a status update is the token user. | The text of the update names the agent. |
| Field value history | A field value has `creator`, `createdAt` and `updatedAt`, and the creator is always the owner login. The timeline lists only Status changes, and loses some. | The signed comments are the log. `audit` does not use field history. |
| Draft items | Fields work. Comments do not. | Agent state lives in fields only. |
| Changed items | There is no server-side filter by time and no order by `updatedAt`. | `poll` reads all known items by id in one query (1 point for up to 100 items) and compares `updatedAt` locally. |
| Item listing | `items()` lags behind writes by minutes and can omit items for longer. | Item ids are the source of truth. The listing is only for discovery, and a missing item is never read as "nothing changed". |
| Comments | A comment keeps its header byte for byte. An edit shows as `updated_at != created_at`. | `watch` detects an edited message and reports it as a security event. |
| Rate limit | A `304` answer costs 0 points, a `200` costs 1. | The comment feed uses ETags. |
| Pins and labels | A repo has at most 3 pinned issues. Creating a label that exists fails with `422`. | `init` is idempotent: it tolerates existing labels and respects the pin limit. It retries "temporary conflict" write errors. |
| Comment size | The documented limit is 65,536 characters. | A message body is at most 65,536 characters. |
| Secondary limits | 80 content requests per minute and 500 per hour. | A crew of about 10 agents stays under them. |
