# terra — example identity

`terra` is a **virtual-worlds & game-level** identity that turns a world brief
into a **launchable, navigable 3D scene** — end to end. Its four-stage recipe
(world design & blockout → terrain & asset authoring → scene assembly → world
brief) plans the spaces, navigation graph, and interaction beats; authors the
terrain and assets in Blender and exports glTF/.glb; wires them into a runnable
scene with a player controller, collision, a baked navmesh, and interaction
triggers; and **verifies the scene launches and is navigable**. It drives real
domain tooling — Godot (game levels, GDScript, `NavigationRegion3D` navmesh,
headless `godot --headless --export-release` build), Blender (terrain + asset
authoring via `bpy`, glTF/.glb export), and A-Frame / WebXR (in-browser
explorable 3D worlds) — entirely from its recipe and the agent sessions it
spawns, with zero `src/` changes. Its assets are validated end-to-end by
`tests/terra_assets_valid.rs` and the
`tests/qa-scenarios/terra-world-build-end-to-end.yaml` scenario.

See [`../README.md`](../README.md) for the data-only example-identity boundary
and the `identity.toml` schema.
