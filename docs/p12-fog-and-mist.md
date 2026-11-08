# Fog and mist rendering

The shipped PC scene runs both effects through `CloudController`, `Cloud`,
`Parallax2D` and `RandomizeSprite`. Each deck reads its own Blender density
and color. Density times the authored population limit, truncated to an
integer, sets the target population. The shared controller applies the
half-second spawn and fade cadence, camera parallax, horizontal wrapping,
and world wind.

| Layer | World Z | Maximum | Lifetime | Fade | Art |
| --- | --- | ---: | ---: | ---: | --- |
| Fog | −5 to −2 | 30 | 3,600 s | 8 s | `fog_01` through `fog_04` |
| Mist | 1.5 to 4.5 | 25 | 12 s | 2 s | `mist_0` through `mist_2` |

The foreground fog doubles its sprite scale and can move at 1.25 times the
camera's horizontal delta. Mist uses unit scale, a stronger wind multiplier,
and rises then sinks across its lifetime. Both layers draw their original
tight meshes with nearest texture sampling. The sorted scene draws mist among
the ground layers and fog across the foreground. Both enter the reflection
capture with the rest of the world.

The shipped `Pow-Diffuse` variant multiplies fog RGB by ambient lighting and
discards fragments below 0.01 coverage. The source fog textures have binary
alpha, so the port can reject the whole draw below that threshold. Mist's
`Ambient Only` shader multiplies both RGB and alpha by ambient light. The port
uses ordinary source-alpha blending for both. Fog uses the ambient part of
`Pow-Diffuse` and the shared scene sort in P12. P13 adds the local light
calculation inside the PC's single pass and enables fog depth writes on 3DS.
[Fog lighting and depth notes](p13-fog-lighting.md)

The optional `atmosphere.bin` stores both authored controllers and their seven
meshes. It uses a separate sprite table, so the v8 world file and existing
save fingerprints are preserved. Older v8 packs load with the atmosphere
asset absent. The visual decks restart with the scene on load or island
change and use a separate random stream from the sky and gameplay.

## Evidence and validation

The local evidence is stored in `review/2026-11-08/`.

- Runtime and scene receipt `p12-fog-runtime.json`
- Retained REA bundle `p12-atmosphere-evidence.json`
- Compiled PC material receipt `p12-fog-material-evidence.json`
- Shipped sprite mesh receipt `p12-atmosphere-mesh-source.json`

Regressions cover independent weather channels, fade timing, foreground
parallax, mist's vertical motion and expiry, material alpha rules, and
sidecar and mesh validation.

The all-target workspace build, 283 workspace tests, nine private PC asset
tests, and strict Clippy passed. The desktop replay compares fog only, mist
only, both layers, and a clear view at the same tick of `Foggy Summer Evening`.
Fog changed 39,601 pixels and mist changed 14,643, including 13,050 in the water
reflection. The fresh 3DS pack preserves all seven atmospheric sprites' alpha
and crop bounds. The release embeds the v8 world, atlas metadata, cloud mesh
and atmosphere sidecars, and all six texture pages.

The prior and updated 3DS releases loaded the same evening save in Azahar
Vulkan. The updated build shows haze developing near the ground and in the
water. At 38 seconds the comparison changed 31,828 top-screen samples by
at least four RGB8 levels, including 18,915 reflection samples. The upper
canopy differed by that amount at one pixel. These captures align wall-clock
times. The desktop fixture compares the layers at one exact game tick. The
build VM and both test emulator processes stopped. Physical 3DS rendering
and performance remain unverified. The release and receipts are under
`target/p12-atmosphere-validation/`.
