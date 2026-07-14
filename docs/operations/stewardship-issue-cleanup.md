---
title: Stewardship issue cleanup
description: Preview and close only confirmed workstream-gap and recurring-reblock noise without touching pull requests or unrelated issues.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: runbook
---

# Stewardship issue cleanup

This runbook prepares operator-reviewed cleanup of confirmed issue-flood noise.
It does not run automatically and is not part of daemon startup. Keep
`simard-ooda` stopped for the entire cleanup.

The candidate predicate is the conjunction of:

1. The GitHub item is an issue, not a pull request.
2. Its body contains the exact line `filed-by: simard-stewardship`.
3. Its `failure-kind` is `workstream_gap:*` or exactly
   `recurring_goal_reblock`.

Do not weaken this predicate. Author, title prefix, label, creation time, or a
signature alone is insufficient.

## Preconditions

```bash
systemctl --user is-active simard-ooda
gh auth status
```

The first command must report `inactive` or `failed`. If it reports `active`,
stop and follow the normal daemon shutdown procedure before continuing.

Set the repository and a deliberately small operator batch:

```bash
repo=rysweet/Simard
batch_limit=20
```

The batch is an operator safety bound, independent of the autonomous
per-cycle mutation limit.

## Export and select candidates

Export every REST issue object for offline review. The endpoint also returns
pull requests, so selection explicitly rejects any object with `pull_request`
before checking body fields.

```bash
export_file="$HOME/simard-stewardship-cleanup-$(date -u +%Y%m%dT%H%M%SZ).json"

gh api --paginate "repos/${repo}/issues?state=all&per_page=100" --slurp \
  >"$export_file"

jq '
  flatten
  | map(select(
    (has("pull_request") | not)
    and ((.body // "") | contains("filed-by: simard-stewardship"))
    and ((.body // "") | test(
      "(^|\\n)failure-kind: (workstream_gap:[^\\n]+|recurring_goal_reblock)(\\n|$)"
    ))
  ))
  | sort_by(.number)
' "$export_file"
```

Review the full title, body, state, URL, and issue number. Save the approved
issue numbers outside the repository:

```bash
approved_file="$HOME/simard-stewardship-cleanup-approved.txt"
${EDITOR:-vi} "$approved_file"
```

Use one numeric issue number per line. Do not generate this file from the
selection without human review.

## Revalidate immediately before mutation

For each approved number, retrieve the REST object and re-check all predicates:

```bash
count=0
while IFS= read -r number; do
  case "$number" in
    ''|*[!0-9]*) echo "invalid issue number: $number" >&2; exit 1 ;;
  esac

  if [ "$count" -ge "$batch_limit" ]; then
    echo "batch limit $batch_limit reached" >&2
    exit 1
  fi

  item="$(gh api "repos/${repo}/issues/${number}")" || exit 1

  if jq -e 'has("pull_request")' >/dev/null <<<"$item"; then
    echo "refusing pull request #$number" >&2
    exit 1
  fi

  body="$(jq -r '.body // ""' <<<"$item")"
  state="$(jq -r '.state' <<<"$item")"
  marker_count="$(grep -Fxc 'filed-by: simard-stewardship' <<<"$body")"
  failure_kind_count="$(grep -c '^failure-kind: ' <<<"$body")"
  failure_kind="$(sed -n 's/^failure-kind: //p' <<<"$body")"

  if [ "$state" != open ]; then
    echo "issue #$number is not open" >&2
    exit 1
  fi
  if [ "$marker_count" -ne 1 ]; then
    echo "marker changed for #$number" >&2
    exit 1
  fi
  if [ "$failure_kind_count" -ne 1 ]; then
    echo "failure kind is missing or duplicated for #$number" >&2
    exit 1
  fi

  case "$failure_kind" in
    workstream_gap:*|recurring_goal_reblock) ;;
    *) echo "failure kind changed for #$number" >&2; exit 1 ;;
  esac

  printf 'READY issue #%s: %s\n' "$number" "$(jq -r '.html_url' <<<"$item")"
  count=$((count + 1))
done <"$approved_file"
```

The preview loop performs no mutation. Preserve its output with the export and
approved list as the audit record.

## Close one reviewed batch

Run only after the preview count and URLs match the reviewed batch. Repeat the
same REST revalidation immediately before each close.

```bash
audit_file="$HOME/simard-stewardship-cleanup-audit-$(date -u +%Y%m%dT%H%M%SZ).jsonl"
printf 'Type CLOSE to close at most %s reviewed issues: ' "$batch_limit"
read -r confirmation
[ "$confirmation" = CLOSE ] || { echo "cleanup cancelled" >&2; exit 1; }

count=0
while IFS= read -r number; do
  case "$number" in
    ''|*[!0-9]*) echo "invalid issue number: $number" >&2; exit 1 ;;
  esac
  [ "$count" -lt "$batch_limit" ] \
    || { echo "batch limit $batch_limit reached" >&2; exit 1; }
  service_state="$(systemctl --user is-active simard-ooda 2>&1)" || {
    service_rc=$?
    case "$service_rc:$service_state" in
      3:inactive|3:failed) ;;
      *) echo "cannot prove simard-ooda is inactive: $service_state" >&2; exit 1 ;;
    esac
  }
  case "$service_state" in
    inactive|failed) ;;
    *) echo "simard-ooda is not safely stopped: $service_state" >&2; exit 1 ;;
  esac

  item="$(gh api "repos/${repo}/issues/${number}")" || exit 1
  jq -e 'has("pull_request") | not' >/dev/null <<<"$item" \
    || { echo "refusing pull request #$number" >&2; exit 1; }

  body="$(jq -r '.body // ""' <<<"$item")"
  state="$(jq -r '.state' <<<"$item")"
  marker_count="$(grep -Fxc 'filed-by: simard-stewardship' <<<"$body")"
  failure_kind_count="$(grep -c '^failure-kind: ' <<<"$body")"
  failure_kind="$(sed -n 's/^failure-kind: //p' <<<"$body")"
  [ "$state" = open ] \
    || { echo "issue #$number is not open" >&2; exit 1; }
  [ "$marker_count" -eq 1 ] \
    || { echo "marker changed for #$number" >&2; exit 1; }
  [ "$failure_kind_count" -eq 1 ] \
    || { echo "failure kind is missing or duplicated for #$number" >&2; exit 1; }
  case "$failure_kind" in
    workstream_gap:*|recurring_goal_reblock) ;;
    *) echo "failure kind changed for #$number" >&2; exit 1 ;;
  esac

  url="$(jq -r '.html_url' <<<"$item")"
  previous_state="$(jq -r '.state' <<<"$item")"
  jq -nc \
    --arg number "$number" \
    --arg url "$url" \
    --arg previous_state "$previous_state" \
    --arg failure_kind "$failure_kind" \
    --arg operator "${USER:-unknown}" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{status: "intent", number: $number, url: $url,
      previous_state: $previous_state, failure_kind: $failure_kind,
      operator: $operator, recorded_at: $recorded_at}' \
    >>"$audit_file" || { echo "cannot write audit intent" >&2; exit 1; }

  gh issue close "$number" --repo "$repo" --reason completed || exit 1
  closed_state="$(gh api "repos/${repo}/issues/${number}" --jq '.state')" || exit 1
  [ "$closed_state" = closed ] \
    || { echo "issue #$number did not reach closed state" >&2; exit 1; }

  jq -nc \
    --arg number "$number" \
    --arg url "$url" \
    --arg closed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{status: "closed", number: $number, url: $url, closed_at: $closed_at}' \
    >>"$audit_file" || { echo "cannot write audit outcome" >&2; exit 1; }
  count=$((count + 1))
done <"$approved_file"
```

Never replace this with `gh pr close`, GraphQL bulk mutation, title matching,
or a repository cleanup script.

The external JSONL audit records an intent before each single close mutation and
a verified outcome afterward. Any audit write failure aborts the batch.

## Stop conditions

Stop the batch immediately when:

- `simard-ooda` is active;
- any candidate has a `pull_request` field;
- the exact marker is missing or duplicated;
- the failure kind is outside the two allowed forms;
- an issue changed since review;
- authentication or API calls fail;
- the batch bound would be exceeded; or
- any issue is ambiguous.

Do not restart or deploy `simard-ooda` as part of cleanup. Daemon lifecycle and
deployment require their separate operational procedure.
