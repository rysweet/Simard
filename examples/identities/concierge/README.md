# concierge — example identity

`concierge` is a hospitality-domain example: it turns a hotel brief into a durable
operations package — a property program and layout, a guest-experience and brand
design, and runnable reservations / PMS / housekeeping / channel-management
workflows whose reservation lifecycle (book → check-in → check-out → housekeeping
→ restored availability) is exercised with enforced no-double-booking and
availability-conservation invariants. Like every example here it carries **no**
`BuiltinIdentityLoader` arm — it is defined entirely by the data files in its
package and loaded by `load_example_identity`. Its assets are validated
end-to-end by `tests/concierge_example_assets_valid.rs` and the
`tests/qa-scenarios/concierge-example-end-to-end.yaml` scenario.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
