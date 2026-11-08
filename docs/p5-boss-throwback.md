# Boss boulder throwback

The follow-up adds the impact emitters and camera behavior. [Impact effects and camera shake](p6-boulder-impact.md)

The boss and Killer Boss can pick up a lodged catapult boulder and throw the
same stone back at the kingdom. The boulder remains lodged at PC world Y 0.9375
until a boss takes it. The shipped prefab and `Boulder` code have no ten-second
lodged timeout.

While Chase runs, the state machine checks transitions after strictly more
than one second. The order is Puke, Throw, then Bash. The projectile scanner
uses a symmetric 0.6-unit range, refreshes at most six times per second, and
retains up to 12 active references. It checks the first result for a stuck
boulder within 1/32 unit of the hand's X position. Another projectile can mask
that result. Movement pauses for two seconds with the old goal retained, and
the throw coroutine lasts 2.6 seconds.

The two-second Throw clip releases at 0.8333333 seconds. The hand moves from
local (0.4, 0.125) to (0.963, 0.876) at that marker, mirrored with the boss's
facing. The prefab supplies a single random magnitude in [4, 7]. The throw
sets velocity to `(facing * magnitude, magnitude)`. Release advances physics
only for the part of the update after the marker. Death before the marker
keeps the fake boulder attached until the boss's ten-second pool despawn.

The returned body uses the extracted 0.15625-unit radius, gravity 15, 0.02-second
fixed step, linear and angular damping, and authored boulder material. It
starts with 0.3 seconds on the ground-only layer. Then it can damage eligible
targets for 15, counting up to five distinct collider objects. A passenger
absorbs the monarch hit by being dropped. A physical impact or admitted Huge
surface contact shatters the boulder. The throw and break use the shipped
sound fields.

Save version 11 carries held ownership, flight remainder, ghost phase, and
already hit objects. Earlier save layouts remain readable when their world
fingerprint matches. Asset version 5 adds the Boss and Boulder prefab data.
The release marker, controller, and Hand track are checked during extraction.

## Evidence

REA inspected PC 1.2.8 `Assembly-CSharp.dll` with SHA-256
`4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de` and
MVID `404eb695-3c80-4444-8f66-e84481194f96`. The combined local bundle at
`review/2026-11-08/p5-boss-evidence.json` holds 79 records. The asset receipt at
`review/2026-11-08/boss-throwback-assets.json` records prefab and clip hashes.

| Behavior | PC method |
| --- | --- |
| Transition order and scanner | `Boss.Awake` 0x06000af6, `Boss.ShouldThrow` 0x06000aff, `Scanner.GetAny` 0x060012eb |
| Catch, pause, release | `Throw.MoveNext` 0x0600181d, `Boss.OnAnimBoulderRelease` 0x06000b02 |
| Chase checks and death | `StateMachine.Update` 0x06001374, `Boss.HandleOnDeath` 0x06000b06 |
| Fake, launch, collision | `Boulder.SetFake` through the collision callbacks 0x06000b0f–0x06000b14, `Ghost.MoveNext` 0x06001829 |
| Passenger and wallet | `Player.HandleOnReceiveDamage` 0x0600121d |

## Validation and limits

The workspace build, 241 workspace tests, and four private PC asset tests
passed. Regressions cover selection and masking, cadence and priority, mirrored
release and marker timing, interrupted release, friendly and returned hits,
ownership validation, and save migration. Clippy with warnings denied,
ShellCheck, and the public-file guard passed. Five isolated mutations triggered
their intended assertions, covering the Chase cadence, release socket, hit
deduplication, saved ownership, and collision grace period.

The 220-frame native capture stages a real friendly catapult shot, then places
the boss beside the lodged stone. It checks pickup, the same stone's release
and break in both directions, and pre-release death. A separate 200-frame
desktop smoke run checks startup with the converted assets. The captures and
3DS build receipts are in `target/p5-boss-validation/`.

Fresh extraction, `pack3ds`, and the populated release build passed using the
cached devkitPro toolchain. The packaged `.3dsx` contains the exact version 5
world and sheet bytes. The build VM stopped and all test and capture processes
exited.

Unity's native overlap order and contact solver remain unmeasured. The port
uses deterministic snapshot ordering, a specialized ground contact, and
approximate trigger shapes for some actors. Loaded catapult props are absent
from the scanner snapshot. The incoming friendly trajectory and impact
particles and camera shake still have fidelity gaps. Hardware validation
remains outstanding.
