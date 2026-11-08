# PC fidelity fixes for combat and save recovery

This change addresses the four P1 findings from the November 8, 2026 review:
stolen loot recovery, captive release from every combat path, airborne archer
targeting, and preservation of unreadable or incompatible saves. The baseline
is upstream `74a3bc66413571526280bc9ad324b87f70d7da63` (v1.2.0).

## PC evidence

Gameplay decisions are based on REA's execution-free inspection of the original
local PC `Assembly-CSharp.dll`, not on the port's comments alone.

- SHA-256: `4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de`
- MVID: `404eb695-3c80-4444-8f66-e84481194f96`
- Member evidence: `ev_e3cd60e93280cde9bfb2b56b620f26e0cc50163595eb6d736acdc12a037df8d4`
- The retained asset export reports bundle version 1.2.8. Its reconstructed
  assembly is different bytes and is not used as the canonical binary.

| Behavior | Original method | Observed CIL relationship |
| --- | --- | --- |
| Release stolen loot on death | `Troll.HandleOnDeath`, `0x0600146f` | Calls `Troll.DropLoot` at IL 34. |
| Clear claims and re-drop loot | `Troll.DropLoot`, `0x0600146e` | Clears both claimers, then calls `Droppable.Drop` at IL 100 with a small upward toss. |
| Preserve the carried object and its owner | `Troll.PickupLoot`, `0x0600146c`; `DroppableShield.OnDisable`, `0x06000d48` | Pickup reparents the live droppable; disabling the shield calls `PayableShield.OnShieldDestroyed`. Theft alone does not free its shop. |
| Recovered coins are no longer player drops | `Droppable.Drop`, `0x06000d3e` | Assigns the new dropper and recalculates player/wildlife origin from its tags. |
| Release victims irrespective of killer | `Squid.HandleOnDeath`, `0x06001366` | Iterates victims and calls `Character.Release` at IL 38. |
| Carried citizens are demoted | `Character.Grab`, `0x06000b6e` | Non-peasants drop coins/items, demote, then enter the grabbed state. |
| Range uses target root position | `Archer.TargetInRange`, `0x060009ee` | Subtracts the archer transform position from the target transform position. |
| Aim uses actual collider center | `Archer.BestShot`, `0x060009f1` | Reads `Collider2D.bounds.center` before solving the trajectory. |

REA method tokens are valid only for the assembly identity above. These static
relationships establish the relevant rules; they do not prove identical Unity
physics, visual animation, hit-rate distributions, or 3DS performance.

The complete local evidence remains in the ignored `review/` directory. Do not
publish the game assembly, extracted assets, or raw reconstruction material.

## Save compatibility and recovery

Save version 6 appends a carried-tool payload containing the original tool ID
and shield owner. Existing enum variants and the serialized `Tool` layout are
unchanged. Version 5 saves remain readable; their kind-only carried tools get
a fresh allocated ID on recovery. Both versions require all mandatory trailing
fields; truncation is not interpreted as a legacy migration. Version 6 saves
require this newer build.

Missing files, unsupported formats, world mismatches, damaged bodies and I/O
failures now remain distinct. A failed startup load pauses with the slot
protected. Autosave, quit-save and run-end deletion use the same protection;
starting over in a protected slot requires its separate NEW → REPLACE action.

A validated backup can resume the run. On the first recovery save, the rejected
readable primary is copied and synced to `<slot>.sav.rejected` (then `.1`, `.2`,
etc. on later recoveries), and the validated `.bak` is retained. These archives
are not rotated or deleted by normal saving or game-over cleanup. An unreadable
primary leaves the recovered run read-only until the player successfully retries
LOAD or saves into another usable slot. Dying before the first recovery save
also preserves the pending original and backup. A healthy primary continues to
use normal rolling-backup behavior.

## Regression requirements

- A stolen coin or tool returns exactly once when its carrier dies; escape
  remains a loss. Recovered coins do not recruit as player-dropped coins.
- Preserve carried tool identity and shield-shop association. Old saved tool
  kinds must remain loadable when their original identity was not recorded.
- Captive release must run through the real arrow, ballista, bear, knight and
  catapult paths, not a test-local copy of the release dispatcher.
- Ground and elevated targets use distinct root positions and collider centers;
  check actual projectile outcomes as well as range and launch-vector decisions.
- Existing corrupt, incompatible and unreadable slots must survive startup,
  manual load, autosave, quit and save-to attempts. A validated backup should
  recover without being replaced by a corrupt main file on the next save.
- Replacing an unavailable slot requires an explicit slot-specific action;
  cancellation must preserve its bytes. Missing slots may start new runs.

## Validation

Final checks on November 8, 2026:

- `cargo build --workspace --all-targets --locked`: passed.
- `cargo test --workspace --locked`: 130 passed, zero failed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- Publication guard, ShellCheck and `git diff --check`: passed.
- 3DS release compile/link and `.3dsx` packaging: passed with an empty RomFS,
  nightly `2026-11-07`, `cargo-3ds 0.1.5` and devkitPro image digest
  `sha256:116afba8df8453961de2936ffab20dd441edf4d682856c1ec8b0e53d7ed0bbf5`.

Tests use the production pickup/collision/dispatch, session and filesystem paths.
The siege fixture repositions a production-fired boulder at the impact frame;
the archer fixtures hold targets at known positions and remove random scatter
through the normal statue state. These are deterministic integration checks,
not recorded gameplay or measurements of overall hit rates.

In isolated source copies, restoring old aiming made both new aiming tests fail;
removing combat recovery produced ten expected failures; and three save mutants
(empty-on-error, backup overwrite, missing archive) each failed their regression.
Same-tick theft/death and truncated v5 game/progress tests also failed before
their fixes. Temporary mutated trees and their test processes were removed.

All 18 new menu strings fit the 320-pixel screen using the actual exported font
metrics. This is a layout calculation, not a device screenshot review.

Local receipts are in ignored `target/p1-validation/`,
`target/p1-save-mutation-proof.json`, `target/p1-save-ui-metrics.json` and
`review/2026-11-08/`. The host toolchain is Rust 1.99.1. The dedicated 3DS
toolchain image is cached locally; its build VM is stopped after validation.

Physical 3DS gameplay, real SD power-loss recovery, audiovisual comparison and
performance measurements remain unverified. No changes to camera, rendering
effects or other P2 fidelity work are included in this first implementation phase.
