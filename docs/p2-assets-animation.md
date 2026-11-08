# PC fidelity: asset transforms and animation

This records the first P2 checkpoint: transform and animation findings (F05/F06)
from the November 8 review, building on the P1 gameplay/save fixes. The
[follow-up phase](p2-remaining-fidelity.md) implements portal transitions,
catapult recoil and F07–F12, and supersedes the asset/save versions and unfinished
items recorded below.

## Implemented behavior

- Keep the full active prefab transform hierarchy, including ancestors without
  renderers. Compose position, quaternion rotation and nonuniform scale as an
  affine matrix; retain reflection, shear, sprite pivots and renderer X/Y flips.
  Desktop and 3DS use shared sprite-corner calculations.
- Preserve authored transform, scale, color and visibility curves; use Hermite
  tangents per component, including stepped values. Null sprite keys hide a
  renderer. Controller states, speeds, transitions and override mappings are
  retained. Duplicate embedded clip names have deterministic aliases so their
  distinct GUID references no longer collapse into the first clip.
- Sample animated ancestors for world objects. Catapult arm and rope geometry
  follow the PC hierarchy and attachment rule; projectile release uses that same
  animated Spoon transform. Boat float and sail controllers remain distinct.
- Dispatch gameplay markers from simulation-owned clocks. Bear attacks begin at
  the gallop's 0.2-second marker and repeat with its 0.7666667-second cycle.
  Worker hammer audio occurs at 0.41666666 seconds; the prefab's separate
  0.7-second work timer still controls construction completion.
- Catapult release, arm-impact and firing-done markers occur at 0.15,
  0.16666667 and 1.1666666 seconds respectively. Firing finishes before the
  2.1333334-second clip ends. Fire audio remains a state-entry event.
- Squid capture happens immediately. The next grab waits for the 0.5-second
  completion marker plus the original 0.1-second settle period. Catch-up cannot
  replay completion from another loop while settling.
- Pausing freezes these clocks. Save v7 preserves their phase; v5/v6 remain
  structurally readable and migrate the previous bear countdown and catapult
  timing. Death, removal and action interruption cancel pending work.

## PC evidence

REA inspected the original local PC `Assembly-CSharp.dll` without executing it.
Its SHA-256 is
`4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de`
and MVID is `404eb695-3c80-4444-8f66-e84481194f96`. Tokens below are specific
to that binary. The retained Unity 1.2.8 asset export supplies the actual keyframes,
controller speeds, transforms and event times; its reconstructed assembly is
not treated as the original binary.

| Rule | Original methods / tokens |
| --- | --- |
| Ability contact is an animation callback | `Steed.OnAnimTriggerAbility`, `0x060013ec` |
| Hammer callback plays sound; work uses its own timer | `Worker.OnAnimHammerHit`, `0x0600157e`; `<Work>c__Iterator1.MoveNext`, `0x06001c3d` |
| Capture immediately, finalize after callback and settle | `Squid.GrabCitizen`, `0x06001363`; `OnAnimGrabComplete`, `0x06001364`; `<FinalizeGrab>c__Iterator4.MoveNext`, `0x06001b91` |
| Launch at transformed Spoon, not a fixed offset | `Catapult.ReleaseProjectileFromAnimation`, `0x06000b4a` |
| Separate impact, completion and state-entry effects | `Catapult.ArmImpactFromAnimation`, `0x06000b49`; `FiringDoneFromAnimation`, `0x06000b4b`; `SetState`, `0x06000b48` |

Retained parent evidence:
`ev_e3cd60e93280cde9bfb2b56b620f26e0cc50163595eb6d736acdc12a037df8d4`.
Focused catapult marker view:
`ev_30e00b9444632755f92341dbb713ce12e122c87cb012c5a0a2cc0970c0f9146c`.
Focused squid capture/completion view:
`ev_190c336a2db1627438699435f21fcb41773e1255caca6f99400f62f3ef71ecb6`.
The complete local bundle and clip receipts remain ignored under
`review/2026-11-08/`; original game assets are not included in source control.

## Asset and save compatibility

Re-extract and repack assets together. The old flattened data cannot recover
discarded transforms or events. `world.bin` and `sheet.bin` now have distinct
magic values, version 1 and an exact payload length; `clips.json` has a versioned
envelope. Old, mixed, future, truncated and trailing payloads fail explicitly.
Hierarchy indices and finite composed poses are validated before world loading.

```sh
cargo run --release -p extract -- --ripped reference/ripped/ExportedProject --out assets
cargo run --release -p extract --bin pack3ds -- assets crates/n3ds/romfs
```

The normal setup workflow also reconverts assets. The save world fingerprint
still identifies exact `world.bin` bytes: rebuilding this changed asset schema
changes that fingerprint. Saves tied to an older world are protected as world
mismatches, not silently applied to a different world or overwritten. Save v5/v6
decoding support does not bypass that check. Keep the old build/assets if an
existing run must remain playable without a separate world migration.

## Validation and remaining limits

Regression coverage includes parent scale/rotation/shear, mirrored pivots,
non-rendered ancestors, actual draw calls, malformed hierarchies, numeric curves,
null sprite keys, controller overrides, duplicate clip names, exact event
boundaries, catch-up, interruption, pause and save/resume. Isolated mutations
restore the old failures and are required to fail these tests.

The real reference conversion preserves 244 clips, 2,547 sprite keys, 4,440
scalar keys, 52 typed event markers, 268 controller states and 349 transitions.
The conversion report lists 94 unsupported features, mostly Unity UI and custom
script properties plus title collider toggles. The boat emblem's active toggle
continues through its explicit existing handling. Required gameplay timelines
must validate; a partially extracted library cannot silently use fixture timers.

Local desktop captures use actual extracted sprites and the production renderer
at 400×240, with injected portal/catapult states and mirrored variants. They are
visual fixtures, not a recorded PC-versus-3DS comparison or a frame-rate result.
The separate desktop smoke run advances normal simulation with saving/audio off.

At this checkpoint, general transition blending and portal transition clocks
were unfinished; portals rendered authoritative endpoints and catapult bodies
remained kinematic. The follow-up implements portal clocks, constrained recoil,
camera, reflection, audio and targeting fixes. General Animator blending,
hardware audiovisual parity and 3DS performance still need further validation.

Validation receipts, extracted private assets and screenshots are ignored under
`target/p2-validation/`. Final command results are in `gates.json`.

Final checks on November 8, 2026:

- Workspace all-target build and Clippy with warnings denied: passed.
- Workspace tests: 154 passed, zero failed. The one opt-in private-reference
  test is ignored by default and passed separately against the local export.
- Publication guard, ShellCheck and `git diff --check`: passed.
- Full PC extraction and real `pack3ds` conversion: passed. Packed textures
  occupy 14.0 MiB over seven pages, with 296 audio clips copied for SD storage.
- Fourteen 400×240 native portal/catapult fixture captures and a 200-frame
  desktop smoke run: completed and inspected.
- 3DS release compile/link and `.3dsx` packaging with populated RomFS: passed,
  using nightly `2026-11-07`, `cargo-3ds 0.1.5` and the cached devkitPro image
  recorded in the P1 validation notes.

All test and capture processes finished. The dedicated build VM was stopped;
no simulator was started.
