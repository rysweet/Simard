# kinema — example identity

`kinema` is an **animation & motion-graphics** identity that turns a story brief
and a shot list into a rendered, playable animation sequence with a written
motion brief. Its four-stage recipe (storyboard → rig → render → motion brief)
drives real domain tooling — Blender (Grease Pencil for 2D, armature rigging +
Cycles/EEVEE for 3D), Synfig (vector 2D tweening), and Natron (node-based
compositing) — entirely from its recipe and the agent sessions it spawns, with
zero `src/` changes.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
