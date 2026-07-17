---
title: How to animate a shot with the Kinema identity
description: Use the pluggable Kinema identity to take a shot brief end-to-end to a rendered animated PNG frame sequence — with a storyboard, a rig, and a Synfig vector source — using the `simard kinema` CLI.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../howto/design-with-atelier.md
  - ../reference/simard-cli.md
---

# How to animate a shot with the Kinema identity

**Kinema** is a pluggable Simard identity for 2D/3D animation &amp; motion
graphics. It takes a structured *shot brief* and produces a **storyboard**, a
**rig** (one armature per object), a **Synfig vector source**, and a **rendered
animated PNG frame sequence** — so a brief can go end-to-end from an idea to a
playable sequence of frames.

Kinema is repo-grounded and runs in engineer mode
(`inspect → act → verify → persist`): it reads the brief, extracts a storyboard,
derives rigs, renders every frame, verifies the result against the brief, and
writes a `manifest.json` recording exactly what was built.

## Prerequisites

- Simard binary built (`cargo build --quiet --bin simard`).
- **No external tool is required.** Kinema's rendered sequence is produced by a
  guaranteed, dependency-free pure-Rust rasterizer, so a shot always renders end
  to end — including in CI.
- Optional, for extra artifacts (Kinema degrades gracefully without them):
  - [Blender](https://www.blender.org/) (`blender`) for a Grease Pencil 2D /
    3D render.
  - [Synfig](https://www.synfig.org/) (`synfig`) to render the emitted `.sif`
    vector source.
  - [Natron](https://natrongithub.github.io/) (`natron` / `NatronRenderer`) for
    a composite pass over the rendered frames.

Check what is available:

```bash
simard kinema inspect --out /tmp/does-not-exist   # prints a tool report
```

## Select the Kinema identity

Kinema ships as a built-in identity (`simard-kinema`) and as a pluggable
identity card under
`prompt_assets/simard/identities/kinema/identity.toml`. Select it for a session
with the identity environment variable:

```bash
export SIMARD_IDENTITY=simard-kinema
```

See [Configure Pluggable Identity](configure-pluggable-identity.md) for how
identity cards are discovered and loaded.

## Write a shot brief

A brief is a small JSON document describing the animated shot. Save it as
`shot.json`:

```json
{
  "name": "hero-crossing",
  "style": "2d",
  "fps": 12,
  "duration_s": 2.0,
  "resolution": { "width": 320, "height": 240 },
  "background": { "r": 18, "g": 22, "b": 33 },
  "objects": [
    {
      "name": "hero",
      "kind": "character",
      "color": { "r": 240, "g": 200, "b": 60 },
      "size": 0.16,
      "keyframes": [
        { "t": 0.0, "x": 0.1, "y": 0.62 },
        { "t": 2.0, "x": 0.9, "y": 0.62 }
      ]
    },
    {
      "name": "beacon",
      "kind": "circle",
      "color": { "r": 90, "g": 170, "b": 255 },
      "size": 0.08,
      "keyframes": [
        { "t": 0.0, "x": 0.5, "y": 0.28, "scale": 0.6, "opacity": 0.5 },
        { "t": 1.0, "x": 0.5, "y": 0.28, "scale": 1.4, "opacity": 1.0 },
        { "t": 2.0, "x": 0.5, "y": 0.28, "scale": 0.6, "opacity": 0.5 }
      ]
    }
  ]
}
```

- `style` selects the preferred external engine: `2d` / `grease-pencil` →
  Blender Grease Pencil, `3d` → Blender, `vector` → Synfig, anything else →
  Natron motion graphics. The pure-Rust rasterizer renders every style.
- `objects[].kind` is `circle`, `rect`, or `character`. A `character` gets a
  full head/torso/arms/legs skeleton and a walk-cycle limb swing; shapes get a
  single transform bone.
- `keyframes` positions are normalised to `[0, 1]` with the origin at the
  top-left. `t` is seconds. `scale` and `opacity` default to `1.0`. Values are
  interpolated with smoothstep easing and clamped before the first / after the
  last keyframe.

## Render the sequence

```bash
simard kinema build --brief shot.json --out ./pkg
```

This writes to `./pkg`:

| File                  | What it is                                                    |
| --------------------- | ------------------------------------------------------------ |
| `storyboard.json`     | Structured storyboard beats sampled from the keyframes.      |
| `storyboard.md`       | Human-readable storyboard.                                   |
| `rig.json`            | One armature per object (skeleton for characters).           |
| `shot.sif`            | Synfig vector source describing the shot.                    |
| `frames/frame_*.png`  | The rendered animated sequence — one PNG per frame.          |
| `sequence.json`       | Descriptor listing every rendered frame in order.            |
| `manifest.json`       | Build record + verification result.                          |
| `blender/`            | Blender Grease Pencil render — only when Blender is installed.|
| `synfig/`             | Synfig render of `shot.sif` — only when Synfig is installed. |
| `composite/`          | Natron composite — only when Natron is installed.            |

Example output:

```text
kinema: hero-crossing (grease-pencil-2d) — 2.00s @ 12 fps, 320x240, 24/24 frames, 2 objects, 8 bones
  [     ok] storyboard.json (…)
  [     ok] rig.json (…)
  [     ok] shot.sif (…)
  [     ok] sequence.json (…)
  [     ok] frames/ (…) — 24 PNG frames
  [skipped] blender/ (0 bytes) — blender not installed
  [skipped] synfig/ (0 bytes) — synfig not installed
  [skipped] composite/ (0 bytes) — natron not installed
  verification: PASS (external render: no)
  manifest: ./pkg/manifest.json
```

`--no-grease-pencil` / `--no-composite` disable the optional Blender / Natron
passes. `--strict` makes the command exit non-zero if the produced sequence
fails verification.

## Verify an existing package

`inspect` re-reads a package directory and re-verifies it against what is
actually on disk, without rebuilding:

```bash
simard kinema inspect --out ./pkg
```

Verification always requires the core deliverables — a storyboard, a rig with at
least one bone per object, every expected frame rendered and non-empty, a
consistent sequence descriptor, and the vector source. If a rendered frame has
gone missing or empty since build time, `inspect` flips the `frames-rendered`
check (and the overall result) to `FAIL` and exits non-zero. The external-render
check is advisory and never fails the package.

## How degradation works

Kinema treats the pure-Rust rasterizer as the guaranteed engine and every
external tool as best-effort:

- **No Blender** → the built-in frame render stands in for the Grease Pencil
  render (advisory).
- **No Synfig** → `shot.sif` is still emitted as a portable, re-renderable
  source; only the Synfig render pass is skipped.
- **No Natron** → the composite pass is skipped; the frame sequence is complete
  on its own.

Every skip is recorded in `manifest.json` with a reason, so the package is
always self-describing — and the animated sequence always renders.
