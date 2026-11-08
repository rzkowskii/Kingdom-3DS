# Boulder impact effects and camera shake

The boulder produces dust and requests a shake on an admitted physical contact.
The PC gates those callbacks during ghosting and until strictly more than 0.1
seconds after the previous contact. Friendly boulders can bounce or lodge with
dust and the collision sound. A shatter emits fragments and the explosion sound.
An admitted Huge trigger can shatter a boulder without a physical impact. Dust
uses the contact pose, while lodging moves the shake origin to world Y 0.9375.
Both emitters use the callback's velocity sign, with zero facing left.

Returned boulder hits also request a shake at the monarch when damage is
admitted, including a hit that only dismounts the passenger. Crownless and
invulnerable monarchs still count as collider objects. The boss's own bash
requests a shake from its authored BashImpact marker, at 0.6 seconds in both
shipped bash clips, even if the punch finds no target.

## Presentation

The converter extracts the original dust and fragment tiles and the emitters'
motion and color parameters. Dust emits 12 particles with four atlas frames
and alternating color gradients, lasting one to two seconds. Fragments emit
four particles with two possible tiles and last one second. The shared scene
renderer draws both at the boulder's layer with their authored depth offset.

The shipped camera requests a one-second DOTween shake of 0.1875 world units
on each axis, with vibrato 20 and 30 degrees of randomness. It accepts requests
strictly within six units of the published camera position in XY. A new request
replaces the active path from its current offset. The port uses the shipped
waypoint timing, decay, and linear interpolation. Each tick snaps the offset to
the 32-pixel grid and applies it to world projection, parallax, and the water
horizon. A new follow target seeks from the published view. The HUD keeps its
screen coordinates.

Effects advance with the session clock, freeze during pause, and clear on
resume or island change. Presentation randomness is separate from gameplay.
Save v12 records the contact cooldown and accepts the earlier v11 tail. Asset
v6 adds the emitter data and the authored shake settings. The world fingerprint
still requires saves to use their matching converted assets.

## Evidence and limits

The combined REA bundle at `review/2026-11-08/p6-impact-evidence.json` contains
97 records. It includes the PC boulder, damage, and camera callsites, the
Playground particle routines, and the shipped `DOTween.dll` shake generator.
The asset receipt is `review/2026-11-08/p6-boulder-particle-assets.json`.

| Behavior | PC evidence |
| --- | --- |
| Physical versus trigger effects and cooldown | `Boulder.HitObject` 0x06000b12 |
| Monarch and boss shake origins | `Player.HandleOnReceiveDamage` 0x0600121d and `Boss.OnAnimBashImpact` 0x06000b01 |
| Spawn pose and particle parameters | `PoolSpawner.Spawn` 0x0600126a and the shipped Boulder Ground Dust and Boulder Fragments prefabs |
| Particle scheduling, motion and color selection | Playground 0x06000458, 0x06000459, 0x0600046c, 0x0600046e and `Rebirth` 0x06000484 |
| Shake range, duration and movement | `MainCamera.Shake`, the camera in `main.unity`, and shipped DOTween shake waypoint generation |

Unity's callback order, exact post-wall velocity, particle thread scheduling,
and random sequence are not measured. The port uses deterministic contacts,
the 60 Hz scene clock and a 256-particle budget. The previous-frame water
reflection and billboard rasterization remain approximations. Real 3DS
hardware validation remains outstanding.

## Validation

The all-target workspace build, 252 workspace tests, and five private PC asset
tests passed. The regressions exercise contact gating and resting contact,
fragment-only hits, the persisted cooldown, particle lifetimes and tile
selection, shake waypoints and range, and projection and pause behavior.
Clippy with warnings denied, ShellCheck, the public-file guard and whitespace
checks passed.

The 220-frame desktop capture runs normal session ticks with an authored
catapult shot and an injected boss placement. It verifies mirrored dust and
fragment bursts, camera movement, and death before release. The normal
200-frame smoke capture also passed. Receipts and captures are under
`target/p6-impact-validation/`.

Fresh extraction, `pack3ds`, and the populated 3DS release build passed. The
packaged `kingdom.3dsx` contains the exact version 6 world and sheet bytes. The
build VM stopped and the test and capture processes exited.
