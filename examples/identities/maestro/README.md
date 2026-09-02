# maestro — example identity

`maestro` is a **music composition & production** identity that turns a musical
brief into an **engraved score** (readable notation) plus a **rendered audio
track** (playable audio). Its five-stage recipe (compose → arrange → engrave →
produce → score & production brief) drives real domain tooling — LilyPond and
MuseScore for engraving, MIDI + open-source synths (FluidSynth, TiMidity++) for
the DAW / render pass, and ffmpeg for encoding / mastering — entirely from its
recipe and the agent sessions it spawns, with zero `src/` changes. Its assets are
validated end-to-end by `tests/maestro_assets_valid.rs` and the
`tests/qa-scenarios/maestro-score-to-audio.yaml` scenario.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
