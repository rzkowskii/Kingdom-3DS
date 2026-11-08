# Red-moon lighting

The port resolves `EnemyManager.redMoon` from the shipped PC scene and applies
the linked `Red Moon` preset. It affects moon tint, moon glow, stars, and two
cloud colors. The existing weather stack supplies the ambient light, sky,
water, and precipitation. The linked preset is distinct from the asset named
`Night Rain Red Moon`.

The fade begins when a live portal accepts a wave with the authored boss flag,
including the shipped retaliation waves and an accepted wave with zero enemies.
The associated boss-wave sound plays at admission. A repeat admission retargets
the active fader from its current weight. Portal openings and direct defense
waves leave this overlay alone.

The fader takes ten active seconds to reach full weight. The day-start event
releases the reference and begins a ten-second fade to removal, even while
bosses remain alive. A new admission during the exit can add a new layer, as in
the PC. Save loading rebinds the last matching fader, including one already
fading out. Save format v12 retains its layout. Asset format v7 carries the
scene's preset reference, and saves still require matching world assets.

Stars start enabled and unmasked. Every five active seconds the port samples
the blended value, hides the field at or below 0.5, and otherwise masks the
rounded fraction `(1 - stars) * 2`. The port applies this to a procedural
110-star field. The PC uses two emitters of 128 and 16 slots and may drain
remaining native particles after disabling them. Cloud colors blend in state
and feed the sky layers described in the [cloud rendering notes](p8-cloud-rendering.md).
Existing wave scheduling retains the port's portal routing rules.

## Evidence and validation

The retained bundle at `review/2026-11-08/p7-redmoon-evidence.json` contains
124 REA records. The extraction receipt at
`review/2026-11-08/p7-redmoon-assets.json` records source hashes and the
authored mask `0x00406c00`.

| Behavior | Shipped PC method |
| --- | --- |
| Admission and retaliation | `EnemyManager.SpawnWave` 0x06000d7a and `SpawnRetaliation` 0x06000d76 |
| Dawn and reload | `HandleOnDayStart` 0x06000d6a, `ClearRedMoon` 0x06000d78, `OnLevelLoaded` 0x06000d67 |
| Linear weight and stack | `BlenderStateFader.TweenTo` and `Step` 0x06000ab9–aba, `Blender.Blend` through `ApplyData` 0x06000aa0–aa4 |
| Moon and star display | `CelestialBody.Update` 0x06000b59, `Stars.SlowUpdate.MoveNext` 0x06001b97, `RefreshMaskSorting` 0x06000439 |

The all-target workspace build, 260 workspace tests, and six private PC asset
tests passed. Regressions cover admission, empty and rejected waves, retaliation,
dawn with a living boss, weather stacking, save rebinding, and star sampling.
Clippy with warnings denied, ShellCheck, publication file checks and whitespace
checks passed.

The ten desktop checkpoints compare an authored boss wave with the same weather
and wave with its boss flag cleared. They use normal session ticks, a staged
portal admission and dawn boundary, sparse scenery for a clear view of the
moon, and the desktop water reflection path.
Both versions agree on every channel outside the overlay. The red-moon fader
reaches full weight, removes visible stars, and clears at dawn. The 200-frame
desktop smoke run also passed. Receipts and captures are in
`target/p7-redmoon-validation/`. Fresh extraction and the populated 3DS release
passed. The packaged `kingdom.3dsx` contains the exact version 7 world and sheet
bytes. The build VM stopped and the test and capture processes exited.
Hardware validation remains outstanding.
