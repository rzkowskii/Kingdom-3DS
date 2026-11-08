# Squid movement and retreat fidelity

This pass continues the P3 checkpoint using the shipped PC 1.2.8 assembly,
serialized prefabs, animation controller, physics settings and compiled shader.
It replaces height easing with physical flight and follows the PC's retreat
coroutines. It does not establish a measured Unity trajectory or 3DS hardware
performance result.

## Flight and obstacles

The extracted squid definition supplies mass 1, gravity 15, gravity scale 0.3,
living drag 0.5, speed 1.3, lift 8, duration 0.6, cooldown 0.4 and fixed step 0.02.
The body uses the authored radius 0.58 and offset (0, 0.875). The flap predicate
uses world Y plus half the vertical velocity. Lift is a force multiplied by mass,
queued after `WaitForFixedUpdate`; it is not an immediate velocity impulse.
The float loop produces 31 force applications for the shipped settings. An
already running flap continues after death; dead drag is zero and radius is 0.22.

Authored altitudes are absolute world coordinates. Actors in this port store
ground-relative height, so base altitude 2.2 becomes 1.325, portal altitude 1
becomes 0.125, and the no-portal altitude 10 becomes 9.125. Saved actor positions
already use ground-relative coordinates and are not shifted during migration.

Physical walls are extracted from their layer-20 polygons, including child
transforms and collider origins. Placed mirroring and the ground-coordinate
conversion are applied once. Scaffolding disables the unfinished wall's collider.
The obstacle scanner uses its independent cache and the authored forward/backward
and vertical range. Damageable targets do not automatically become obstacles.
Retreat changes the body to the ground-only collision layer.

Wave cohesion uses the PC's random target offset (-5 to 10), speed changes
(-0.12 ahead, +0.108 behind), and clamp from moveSpeed-0.4 to moveSpeed. Citizens
and nearby obstacles determine whether the squid dives or keeps its base height.
Scanner selection retains the PC's selection-before-eligibility behavior.

## Retreat and presentation

Transition checks during a running coroutine require strictly more than one
second; a completed chase coroutine checks immediately. Retreat unregisters the
attacker, selects the nearest same-side portal, and refreshes after one second.
Within three units of the portal it starts the despawn coroutine, lowers the
desired altitude, ramps the magenta overlay over two seconds, then removes the
squid after one second. This is a color overlay, not a transparency fade.

The shipped no-portal branch has an observable quirk: after setting altitude 10
and waiting ten seconds, it calls `Despawn()` but discards the returned iterator
without starting it. This port preserves that behavior: the squid keeps flying
with its prior horizontal command and is no longer an active attacker. It does
not invent an offscreen removal timer. The prefab also does not retreat at dawn.

The Move/Dive In/Dive Out controller follows vertical-speed thresholds and exit
times, including Move's 0.75 normalized exit and 0.25-second transition. Attack
presentation runs to its authored endpoint independently of the grab's completion
and settle clocks. Death keeps the body opaque for two seconds, fades over the
next second, and removes it after four seconds.

Back-to-back captures queue their next Attack instead of completing the grab
from a timer started at capture. Completion follows the actual animation marker,
then a separate persisted 0.1-second settle. Mover also resumes on the update
that expires its pause, as the PC does.

The actual `Custom/PowerSprite2` DX9 shader programs were recovered from the
original serialized Shader object. Both normal and recolor variants add the
overlay **after** lighting:

`RGB = lit_recolored_RGB * (1 - overlay_alpha) + overlay_RGB * overlay_alpha`

Coverage alpha is unchanged. Desktop GLSL and 3DS texture-environment stage 1
follow this order. The 3DS batching key and stereo replay retain the overlay;
ordinary draws reset the extra stage.

## Persistence and regression protection

Save version 10 appends squid physics, cognition, retreat and animation clocks
outside the unchanged legacy game-state prefix. Exact version 9 and version 8
tails still decode through explicit migration. Missing, extra, nonfinite and
invalid runtime data is rejected. Scanner contents remain transient references;
restoring them does not consume the saved gameplay RNG stream.

Asset version 4 includes the squid definition and obstacle polygons. Re-extract
and repack world and sheet together. World fingerprints remain enforced: older
saves tied to different converted world bytes remain protected, rather than
being silently rewritten against incompatible assets.

Tests cover lift timing and mass cancellation, damping, ground/wall contacts,
retreat collision filtering, cached obstacle decisions, wave speed adjustment,
transition boundaries, portal/no-portal outcomes, death, motion/attack animation,
save migration and corruption, mirrored collider snapshots and draw state.

An independent upstream Box2D 2.3.1 harness compares 600 fixed steps. Maximum
component error is approximately 3.2e-5 for free/dead motion (the reference sleeps
at rest), and 9.81e-7 for the wall/filter sequence. The local reproducible harness
and five detected solver mutations are in
`review/2026-11-08/squid-flight-oracle.{cpp,json}`.

## Evidence and remaining limits

REA method evidence is bound to `Assembly-CSharp.dll` SHA-256
`4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de`, MVID
`404eb695-3c80-4444-8f66-e84481194f96`. It includes Squid Update/FlyUp/Chase/
Retreat/Despawn/HandleOnDeath, StateMachine transition scheduling, scanner and
Mover behavior, and SpriteFX/Damageable fade routines. The final local REA bundle
is `review/2026-11-08/p4-squid-evidence.json` (58 retained records); separate extraction, numerical and
shader receipts preserve the source paths, hashes, method tokens and assumptions.

Shader path ID 102 comes from `sharedassets0.assets`, SHA-256
`8b37a926ac7b30ae9f2279a5881f283ea824767dc93ca5ab56cd8e10cb42d662`.
Raw programs, decoded instruction offsets and constant-register bindings are
retained locally. A float32 interpreter of those programs passes 240 numerical
cases; applying lighting to the overlay fails all 180 nonzero-overlay controls.
This proves overlay order; the port's overall lighting remains an approximation.

The specialized solver does not reproduce Unity's exact concave decomposition,
manifold ordering, sleeping or dead-body rotation. Capped/tied scanner results
can differ from Unity broadphase order. Sprite transitions retain their clocks
but do not blend two sprite images. Captured citizen identities remain separate
fidelity work. Boss boulder
throwback, red-moon presentation and real-device GPU/DSP/performance checks also
remain open.

Validation receipts and native 400×240 captures are stored under
`target/p4-squid-validation/`. The fixture injects initial positions, retreat and
death, then runs production 60 Hz ticks sampled at 30 Hz. It is silent and uses no
save storage; it is not a PC capture, playthrough or FPS benchmark.

Final checks on November 8, 2026:

- All-target workspace build and **227 workspace tests** passed. All **three**
  private reference tests against the actual PC asset export passed separately.
- Clippy with warnings denied, public-file guard, ShellCheck and whitespace checks
  passed. All **13** isolated solver/FSM mutations were detected; temporary
  mutation workspaces were removed.
- Fresh extraction and real `pack3ds` passed. Seven packed texture pages use
  14.0 MiB decoded; 296 audio clips remain prepared separately for SD storage.
- **392 native 400×240 motion frames**, four GPU overlay probes, and a 200-frame
  normal desktop smoke run passed. Sampled images and motion/phase traces were
  inspected. The probes verify unlit overlay, transparent silhouette coverage
  and half-alpha blending on the actual desktop GPU backend.
- Final populated 3DS release compile/link and `.3dsx` packaging passed with
  nightly `2026-11-07`, cargo-3ds 0.1.5 and the cached devkitPro image. Exact
  version-4 world and sheet bytes were found inside the packaged binary.
- The output is `target/p4-squid-validation/kingdom.3dsx` (3,018,588 bytes,
  SHA-256 `9b4a420bc0ffacd3b5722c3ff4ab03d1466fd94af30d59145ed7a06c18cf8ddc`).
  Build/test/capture processes finished and the dedicated build VM stopped.
  No simulator was started. Real 3DS hardware validation remains outstanding.
