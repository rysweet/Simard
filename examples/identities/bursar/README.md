# bursar — example identity

`bursar` is an investment-portfolio **research & advisory** identity
(research/advisory only — it **never** executes trades, places orders, or moves
money). It takes a portfolio and a mandate through a five-stage loop — asset
allocation → backtesting → risk analysis → rebalancing **plan** → report —
driving domain tooling (`pandas`, `backtrader`, `QuantLib`) from its recipe, with
zero `src/` changes. A rebalancing "plan" is a document of proposed trades for a
human to review, not an instruction the identity carries out.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
