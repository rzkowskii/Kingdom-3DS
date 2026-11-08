# Fog lighting and depth

The shipped PC game uses one `Pow-Diffuse` forward base pass for fog.
`Fire` registers its light with the scene's `LightFaker`, which sends
up to 50 entries containing the light's world position and enabled
intensity. The shader adds a warm contribution to the ambient at each
authored mesh vertex. Each light adds `(.49, .425, .34) × intensity ×
max(1 − distance3D / 3, 0)`. The fire prefab moves its point light toward
the camera by `0.25 + 2.75 × clamp(strength, 0, 1)` world units. Mist
uses the ambient-only material.

Both port backends evaluate that lighting at the original fog vertices,
including their scale, flip and world depth, and interpolate the resulting
colors across the triangles. The weather tint multiplies the combined light
while coverage still comes from the fog texture and its fade. Fog uses the
port's active fire attachments and deterministic flicker. The original's
fire activation delays, fade timing, and global registration order are
approximated by the port's existing fire lifecycle and the scene's draw list.

The 3DS uses source-alpha blending and writes depth where fog passes its
alpha cutoff. Fog passes at 3/255 and above after quantization, with
greater-or-equal depth testing for the reversed depth buffer. Both stereo
eyes apply the same raster state. Mist tests depth without writing it.
The 3DS keeps the port's painter-order depth values, and desktop uses
painter-order composition in P13. P14 uses world Z after the first fog draw
on 3DS and adds the PC's fire activation timing. [Fog depth and fire timing
notes](p14-fog-depth-and-fire-timing.md)

## PC evidence

The reference is Kingdom New Lands 1.2.8 for Windows. REA identifies
`LightFaker.LateUpdate` (0x06001017), `UpdateShaderLights` (0x0600101A),
and `CondenseLightsToPositions` (0x0600101B) in `Assembly-CSharp.dll`.
`Fire.SlowUpdate`'s iterator (0x06001975) drives the lamp depth and intensity.
The Z curve is in `sharedassets2.assets` object 396. The D3D9 and D3D11
vertex variants of `Custom/PowerSprite2` in `sharedassets0.assets`
object 102 show the fixed radius and RGB coefficients. The local evidence
is in `review/2026-11-08/p13-fog-lighting-evidence.json` and
`target/p13-fog-lighting-research/managed-rea.json`.

## Validation

The all-target build, 285 workspace tests, nine private PC asset tests, and
strict Clippy passed. The same-tick desktop fixture strips only the fog's
local light for its control. At 38 simulated seconds in `Foggy Summer Night`,
local light changes 11,265 pixels by up to eight RGB8 levels.

The 3DS release embeds the same ten RomFS files as P12 byte for byte.
Both releases loaded the same version 12 save and drew the foggy night
scene in Azahar Vulkan. A separate build with diagnostic geometry
exercised the production raster state and stereo replay. Both eyes
confirmed that covered fog occludes a farther draw, mist leaves it
visible, alpha 2/255 is discarded, alpha 3/255 writes depth, and
equal-depth fog blends. The build VM and emulator sessions stopped.
Physical 3DS rendering and performance remain unverified. The release,
captures, and receipts are under `target/p13-fog-lighting-validation/`.
