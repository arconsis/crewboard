# CI response fixtures

Not every file here is recorded from a live run: the sandbox repo has no CI run history, so some
are written by hand.

- `runs-empty.json`, `checks-empty.json`: recorded live from a sandbox repo, shown as `example-org/app` (a commit
  of `main`, 2026-10-15). `gh api repos/.../branches/main/protection/required_status_checks` answered
  HTTP 403 "Upgrade to GitHub Pro" there; `fetch_ci` treats 403 and 404 as "no required list".
- all other files: written by hand in the documented response shape of
  `GET /repos/{r}/actions/runs?head_sha=`, `GET /repos/{r}/commits/{sha}/check-runs` and
  `GET /repos/{r}/branches/{b}/protection/required_status_checks` (field names, `status`, `conclusion`,
  `head_sha`, `html_url`). They are NOT recorded from GitHub.

## Commit statuses (M3)

- `status-empty.json`, `statuses-empty.json`: recorded live from a sandbox repo, shown as `example-org/app` (a commit of
  `main`, 2026-10-16). The combined answer is trimmed to `state`, `statuses`, `sha`, `total_count`. Note that
  GitHub answers `"state": "pending"` for a commit with NO status (`total_count` 0): "pending" alone is not
  "CI is running". `evaluate_ci` looks at `total_count`.
- `status-{success,failure,pending,error}.json` and `statuses-*.json`: written by hand in the documented shape of
  `GET /repos/{r}/commits/{sha}/status` (combined) and `GET /repos/{r}/commits/{sha}/statuses` (list, newest
  first). NOT recorded from GitHub.
