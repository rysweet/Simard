# loremaster — example identity

`loremaster` is a **tabletop-RPG campaign designer & game master** identity that
turns a campaign brief into a durable, **playable campaign module** — world lore &
factions, NPCs, XP-budget-balanced encounters, session-prep material, and a
**Foundry VTT** module — and then **runs a session end to end** (roll initiative →
resolve combat → terminating outcome, with seeded/reproducible dice) to prove the
module actually plays. It works with **Dungeons & Dragons and other tabletop
RPGs** using **open SRD content only** (SRD 5.1, CC-BY 4.0 / OGL). Its two
goal-session recipes are
[`loremaster-campaign-module.yaml`](./recipes/loremaster-campaign-module.yaml)
(world & lore → NPCs & encounters → session prep → assemble & run) and
[`loremaster-encounter-balance.yaml`](./recipes/loremaster-encounter-balance.yaml)
(build & XP-budget-balance encounters → run a combat encounter and verify the
invariants). It enforces the safety invariants that make a run trustworthy:
SRD-legal content only, every encounter's **adjusted XP budget** (Σ monster XP ×
the SRD encounter multiplier) inside its target difficulty band, and **no
accidental TPK**. Like every example here it carries **no** `BuiltinIdentityLoader`
arm — it is defined entirely by the data files in its package and loaded by
`load_example_identity`. Its assets are validated end-to-end by
`tests/loremaster_example_assets_valid.rs` and the
`tests/qa-scenarios/loremaster-example-end-to-end.yaml` scenario.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
