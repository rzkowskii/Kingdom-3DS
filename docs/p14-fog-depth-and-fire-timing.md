# Fog depth and fire timing

The PC main camera has a 24-bit depth buffer and looks along positive Z.
The fog shader writes depth at its coverage cutoff. Render order still
comes from the material queue, sorting layer, order, and distance. The
shipped scene uses the Default sorting layer. Its near-snow particles
use order 1, while ordinary sprite renderers use order 0.

The 3DS batches the world before the first fog draw using the existing
painter-order depth. From that fog draw onward, it submits the world in
the same order but tests against a separate range based on scene Z. This
lets covered fog reject a later draw that is farther away, including
opaque atlas sprites. Nearer and coplanar draws can still blend, and
transparent fog texels leave the depth open. Both stereo eyes replay
those same depth values. The touch screen, screen overlays, and the
reflection-only camera have their own composition paths.

The ground light approximation composites before fog. Flames and coin
indicators use their attachment depth, so they can show through fog and
foreground scenery. Desktop keeps painter-order composition. The 3DS
world-depth path targets fog and leaves the wider set of Unity material
depth writes and render queues outside this change.

## Fire activation

An auto-start fire emits on the first active scene tick if its time-of-day
condition is met. Night-only fires schedule independent delays of up to
five seconds for the dusk start and dawn stop. The torch follows the
ambient threshold and restarts its lamp fade when enabled. A statue's
charge activates its fire attachments.

Starting a fire enables the flame and water glint immediately. Its lamp
waits one second, then fades in over one second. The port samples the
lamp every nine simulation ticks, matching the PC's 0.15-second
`SlowUpdate` interval. Each update rolls the next flicker as
`(previous + uniform(−.3, .3)) × .8`, then scales that bonus by the
ambient curve. Stopping the fire turns off its light and glint. The
port removes its flame sprite at that point. The PC's remaining
particles can finish their lifetimes.

Fire timers and their random stream belong to the scene, independent of
gameplay randomness. Loading or changing islands restarts them. A scene
paused before its first tick initializes its fires when simulation resumes.

## PC evidence

The reference is Kingdom New Lands 1.2.8 for Windows. The raw scene and
shader evidence is in `review/2026-11-08/p14-fog-depth-evidence.json`.
It records the main camera in `level2` object 265, its transform in
object 186, and the sorting-layer table in `globalgamemanagers` object 3.
`MainCamera.SetRenderSize` (0x0600104B) allocates depth for the main
target. `MainCamera.OnPostRender` (0x0600104A) copies color for the
reflection camera. `Payable.AddCoinIndicators` (0x06001143) and
`FloatingCoinInRow` (0x06001148) place indicators 0.005 units in front
of their payable. The retained REA records are under
`target/p14-fog-depth-research/`.

`Fire.HandleOnDayStart` (0x06000DFE) and `HandleOnDayEnd`
(0x06000DFF) schedule the dawn and dusk events. `StartFire`
(0x06000E05), `StopFire` (0x06000E06), and `DelayLight`
(0x06000E07) separate flame emission from the lamp fade.
`Statue.SetFires` (0x060013A7) drives the dormant statue attachments.
The fire evidence is in `review/2026-11-08/p14-fire-timing-evidence.json`
and `target/p14-fire-research/managed-rea.json`.

## Validation

The all-target build, 290 workspace tests, nine private asset tests, and
strict Clippy passed. A headless fixture using the shipped assets and a
version 12 night save recorded the torch on the first active tick, its
lamp ramp after one second, and independent start and stop deadlines for
three copies of the authored Farmhouse1 fire.

The packed 3DS release embeds the same v8 world and RomFS assets as P13.
It loaded the same foggy-night save in Azahar Vulkan. A separate stereo
diagnostic using the real fog mesh verified eleven samples per eye,
including later opaque and blended sprites behind fog, clear texels,
coplanar and nearer sprites, and screen overlays. The gameplay comparison
uses wall-clock checkpoints. The depth diagnostic uses a fixed scene.
Both emulator runs and the build VM stopped. Physical 3DS performance
remains unverified. The release and receipts are under
`target/p14-fog-fire-validation/`.
