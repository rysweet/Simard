---
title: File and consolidate stewardship issues safely
description: Use typed semantic decisions and the durable issue guard without recursive inputs.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
---

# File and consolidate stewardship issues safely

Routine `workstream_gap` observations are never issue proposals. Keep them on
the existing observation, counter, and operator-notification path.

## 1. Admit typed sources

Require current `ArtifactProvenance` with `Operator`, `System`, or `External`
origin. Reject `Stewardship`, `LegacyUnknown`, missing lineage, and unknown
versions before recipe invocation. Do not infer provenance from issue bodies,
titles, labels, authors, or goal descriptions.

## 2. Ask the recipe for semantics

Invoke `prompt_assets/simard/recipes/issue-consolidation.yaml` with bounded,
already-admitted source IDs and evidence. Its typed result contains:

```json
{
  "schema_version": 1,
  "decisions": [{
    "condition_id": "stable-condition-id",
    "classification": "actionable_failure",
    "source_ids": ["typed-source-id"],
    "proposed_action": "create_issue",
    "confidence": 0.9,
    "evidence_refs": ["typed-evidence-ref"]
  }]
}
```

The agent decides semantic equivalence and stable naming. The condition ID must
not depend on issue numbers, run IDs, or generated goal slugs. The recipe does
not emit provenance, authorization, mutation identities, cycle identities,
limits, or retry decisions.

## 3. Construct the typed request

Trusted Rust code validates the recipe envelope and constructs
`IssueMutationIdentity`, `ArtifactProvenance`, and `IssueMutationRequest`.
For issue creation, the transport appends:

```text
simard-mutation-id: <stable identity>
simard-provenance: stewardship
```

Stewardship issue bodies also retain `filed-by`, `stewardship-signature`,
`stewardship-condition-id`, and `failure-kind` fields for operations and audit.
For new guarded filings, the signature is the typed condition identity supplied
by the admitted producer; Rust does not normalize error prose to invent one.
These markers assist reconciliation and cleanup; they are not the idempotency
database.

## 4. Execute through the owning adapter

The stewardship observer, engineer-log-analysis thread, supply-chain steward,
creative-ideas review route, and OODA safeguard/escalation paths all route
creates through `MutationGuard`. Do not add a raw `gh issue` write or call
`IssueMutationTransport` from another autonomous component.

Propagate guard errors. In particular, do not continue a cycle after mutation
limit exhaustion, ambiguous reservation, persistence failure, or provenance
rejection.

## 5. Keep output out of recursion

Stewardship issue outcomes never enter `GoalBoard`. Typed provenance still
protects legacy or reconstructed stewardship artifacts at promotion, in-flight
discovery, blocked-goal discovery, gap detection, and signal conversion.

Legacy snapshots without provenance deserialize as `LegacyUnknown` and remain
excluded until a trusted migration or operator action classifies them.

## Anti-patterns

- Filing an issue or backlog item for a routine workstream gap.
- Deriving semantic identity from prose in Rust.
- Using GitHub search as authoritative idempotency state.
- Retrying an unfinished reservation automatically.
- Treating missing provenance as external.
- Logging a fatal guard error and continuing the cycle.
