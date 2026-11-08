# PC fidelity: catapult physics and squid cognition

This is the P3 checkpoint. The subsequent [squid movement and retreat pass](p4-squid-flight-retreat.md)
replaces its remaining squid movement/retreat gaps and advances save/assets to versions 10/4.

This supersedes the constrained recoil model documented in
[the preceding P2 pass](p2-remaining-fidelity.md). It also corrects boulder release
timing and the remaining squid scan/chase cadence that could be established from
the original PC assembly.

## Catapult physics

The catapult now has three dynamic bodies: its chassis and two independent
wheels, connected by free hinges. A bounded, allocation-free Rust solver uses
Box2D 2.3.1 sequential-impulse equations for these joints and circle/ground
contacts. It includes warm starting, friction, damping, position correction,
velocity limits, sleep and wake. The original Box2D zlib notice and an alteration
notice are retained in `crates/game/src/recoil.rs`; no external physics runtime is
added to the 3DS build.

REA and the actual exported prefab establish these rules:

- The chassis has mass 5, linear drag 1 and angular drag 0. Each wheel has mass 1,
  radius 0.171875 and zero drag. Wheel anchors are at ±0.421875, 0.171875.
- The chassis polygon is a trigger inside `Flippable`. It contributes mass
  geometry but does not create a solid ground contact. Physical wheel bodies
  are outside `Flippable`; turning does not swap or mirror them.
- Direction mirrors `Flippable` and moves the chassis center of mass to the
  corresponding physical wheel. Inertia is derived separately for each direction
  from the mirrored polygon and selected center. Both evaluate to approximately
  0.8364514 for the shipped prefab.
- The impact callback applies angular impulse `-0.5 * direction * mass` to the
  chassis. Hinges and contacts transmit it to the wheels; no leading wheel is
  pinned and no restoring spring is invented.
- Moving writes chassis velocity `(direction * 0.15 * worker_count, 0)`; entering
  Reloading clears only that body's linear velocity. Wheels and angular motion
  remain dynamic. Physical displacement updates the authoritative level object,
  so workers, targeting, drawing and saves use the same root.
- Gravity is 15, fixed timestep 0.02 seconds, and solver iterations are 8/3.
  Ground is at world Y=0.8675: the scene edge offset is 0.43 and
  `Level.GenerateInternal` supplies points at `0.875 * 0.5`. The catapult spawns
  at Y=1 and settles. Null physics materials use Unity's documented defaults of
  friction 0.4 and restitution 0.

Extraction validates the supported topology, dynamic-body settings, trigger
collider, referenced flippable transform, hinge connections, materials and
collision layers. A different configuration fails conversion. Required physics
settings are read explicitly; fixture defaults do not substitute for missing
production data.

Drawing uses the chassis pose and each wheel's own position and spin. Only the
authored flippable subtree mirrors. The Spoon launch point shares the chassis
pose. Local solver coordinates are rebased when root displacement is committed;
the physical ground reference moves with that rebase.

Two integration corrections accompany the solver:

- A border change can cancel a Ready-to-Firing transition in the same update,
  matching the PC's independent in-position check. No later animation payload is
  emitted for that cancelled shot.
- New boulders advance only for time after their release marker. For example,
  crossing 0.14→0.16 seconds with release at 0.15 gives the new boulder 0.01 seconds
  of flight, while existing boulders receive the full 0.02 seconds. This also
  preserves the new boulder's full initial collision-grace duration.

## Numerical reference and limits

An independent C++ harness executes upstream Box2D tag `v2.3.1`, commit
`7e633c4fb86a68bf072fb8ae67ea2c060114750e`, with the extracted parameters.
Each direction runs 400 fixed steps covering initial fall, contact, sleep,
impulse, one second of pushing, chassis stop and coasting. All three bodies'
positions, angles and linear/angular velocities are compared, along with sleep.
Across 800 steps the maximum absolute component error is `2.02e-6`; sleep agrees
throughout. Sixteen checkpoints are embedded in the regression suite with a
`1e-5` tolerance. Removing hinge impulses, friction, chassis damping or joint
warm starting fails the independent-reference regression.

This establishes agreement with that Box2D specialization, **not a measured
Unity-native trajectory**. Chassis inertia remains derived rather than read from
a running `Rigidbody2D`. Unity's precise contact generation, manifold ordering
and internal solver variant remain unverified. The implementation supports two
circle contacts against the game's flat static ground and two free hinges; it is
not a general replacement for Unity Physics2D. Contact offset maps to edge skin,
and the reference run uses discrete collision to match the prefab setting.
3DS hardware performance and visual parity still require device validation.

The reproducible C++ harness, pinned revision, complete comparison metrics and
limitations are retained locally in
`review/2026-11-08/catapult-three-body-oracle.{cpp,json}`. The asset receipt is
`catapult-physics-extraction.json` in the same ignored directory.

## Squid scan and chase behavior

Squids now retain the PC scanner's seeded startup phase, 0.5-second minimum
refresh interval and maximum 12 cached citizens. Cached identities resolve live
positions, and inactive identities drop out. The chase coroutine reevaluates
after 0.5 seconds when following a citizen, or 5 seconds when heading toward the
campfire. A retreating squid can still capture citizens until its capacity is
reached, without losing its retreat state during grab completion or reload.

These cognition clocks are transient, as in the PC's saved `SquidData`. Loading
reinitializes them lazily while preserving the port's saved gameplay RNG stream.
Fresh spawns consume their seeded startup draw normally. Selection still occurs
before the eligibility check; an ineligible selected citizen is not silently
replaced with a farther eligible one.

Known gaps remain: Unity broadphase ordering for tied/capped results, captured
citizen identities, obstacle scanner behavior, flap physics, wave-cohesion speed,
retreat transition cadence and its fade/despawn delay. Broader outstanding items
include boss boulder throwback and red-moon presentation. General Animator
blending and the previously documented GPU/DSP hardware checks also remain.

## Persistence and assets

**Save v9** stores all three bodies, velocities, fixed-step remainder, hinge and
contact impulses, sleep state and ground reference in the versioned tail. The
legacy game-state prefix is unchanged. Exact v8 decoding validates the old recoil
layout before migrating its pose, tangential velocity and clock into the new
model. Versions 5–7 retain their prior endpoint migration. Corrupt, missing,
trailing, nonfinite and overflow-prone states are rejected, including a ground
reference inconsistent with the saved object. Death removes the physics state
before a same-tick save.

**Asset version 3** carries the expanded physics definition; world and sheet must
be regenerated and repacked together. The clip envelope remains version 1.
Existing world fingerprints remain enforced. An older save tied to different
`world.bin` bytes stays protected as a world mismatch; format migration does not
bypass this check. Keep the earlier build/assets for an existing run until a
separate world migration is available.

## Evidence and validation

REA inspected the original PC assembly without executing it. Identity remains
SHA-256 `4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de`,
MVID `404eb695-3c80-4444-8f66-e84481194f96`.

| Behavior | Original assembly evidence |
| --- | --- |
| Direction, center of mass and flippable scale | `Catapult.set_direction`, `0x06000b3f` |
| Chassis velocity reset and impact impulse | `Catapult.SetState` / `ArmImpactFromAnimation`, `0x06000b48` / `0x06000b49` |
| Spoon release and completion | `Catapult.ReleaseProjectileFromAnimation` / `FiringDoneFromAnimation`, `0x06000b4a` / `0x06000b4b` |
| Builder drive and independent border check | `Catapult.Update`, `0x06000b4e`; REA view `ev_80aebc6ca8abe61982532ca50212617d6c951878df9577d227296ab28cd3dda1` |
| Ground edge point height | `Level.GenerateInternal`, `0x06000fe5` |
| Scanner cache, refresh and selection | REA view `ev_d3255df72c0126986f4092efd3871a5dac5447e817fc6660d1dc9282d41003e1` |
| Chase coroutine waits | REA view `ev_70920306438c3f67bd15aae11e0f918ddaee334e24d04decd8bdfbb69cd66294` |
| Cognition omitted from PC persistence | REA view `ev_dad921f621576325775e2831c51a4bbc3824828c34bf23e21685088e0714475c` |

The combined local REA bundle is
`review/2026-11-08/p3-physics-evidence.json` (48 retained records). Default-material
behavior is corroborated by the [Unity 2017.1 API documentation](https://docs.unity3d.com/2017.1/Documentation/ScriptReference/Rigidbody2D-sharedMaterial.html).

Final checks on November 8, 2026:

- Workspace all-target build: passed. **204 workspace tests** passed; both opt-in
  tests against the actual PC asset export passed separately.
- Clippy with warnings denied, publication file guard, ShellCheck and diff
  whitespace checks: passed.
- Sixteen isolated negative controls detected: four solver, seven integration
  and five squid mutations. Temporary mutation workspaces were removed.
- Full PC extraction and real `pack3ds`: passed. Seven texture pages remain
  14.0 MiB decoded, with 296 audio clips prepared for SD storage.
- 360 native 400×240 frames at 30 Hz: captured with actual assets and production
  GameState ticks; sampled images and phase/body traces inspected. Catapults
  settle for 120 ticks before the injected fire state. The fixed-camera recording
  is silent, includes mirrored cases, and is not a PC comparison or FPS result.
  A separate 200-frame normal desktop smoke capture completed with saving/audio
  disabled.
- Populated 3DS release compile/link and `.3dsx` packaging: passed using nightly
  `2026-11-07`, `cargo-3ds 0.1.5` and the cached devkitPro image from the P1 notes.

Receipts, screenshots, the silent motion video and build logs are ignored under
`target/p3-physics-validation/`. All test/capture processes finished and the
dedicated build VM stopped. No simulator was started.
