# Sky clouds and red-moon tints

The extractor follows `Level/CloudsStratus` and `Level/CloudsCumulus` to their
controllers, prefabs, material and six sprite variants. It keeps their depth,
height, scale and parallax curves. Stratus sits at Z 50–52 and cumulus at Z 42–52,
in front of the sky and celestial bodies. Both layers feed the water reflection.

The target population is the authored maximum times the blended weather
density, truncated to an integer. The controller creates, reuses or fades one
instance at a time on its half-second cooldown. Clouds fade over eight seconds,
move with the authored wind and camera parallax, and wrap at ±16 world units.
The port uses a separate deterministic random stream for presentation and clears
the cloud decks on scene reload, following the PC loader. Individual placements
will differ from Unity's random stream.

The red-moon fader blends `cloudStratusColor` and `cloudHighlightColor`.
Cumulus base color and both density fields come from the underlying weather.
Stratus has binary source alpha, so its surviving texels receive only the
ambient-lit stratus color. On cumulus, the PC fragment shader adds the highlight
in proportion to inverse source alpha and the exposure from two shadow-map taps.

Both backends use the recovered coverage rule with nearest sampling. The
extractor clears source alpha below 0.01 and maps surviving alpha to
`0.5 + 0.5 × sourceAlpha`. The 3DS atlas preserves that coverage in RGBA8 and
skips the recolor-mask split for these sprites. The handheld uses texture
combiners for the ambient base and highlight in one draw. The original vertex
shader doubles Unity's half-strength legacy ambient register. The port uses
the authored ambient color and the layer's fade opacity directly.
[Unity 2017 shader variables](https://docs.unity3d.com/2017.1/Documentation/Manual/SL-UnityShaderVariables.html)
[Unity built-in shader include](https://github.com/chsxf/unity-built-in-shaders/blob/v2017.1.2f1/Shaders/CGIncludes/UnityShaderVariables.cginc)

The initial build used full cumulus highlight exposure. The next pass implements
the depth-map comparison described in the [cloud shadow notes](p9-cloud-shadows.md).

## Evidence

The local REA bundle `review/2026-11-08/p8-cloud-evidence.json` retains 138
records from the shipped PC assembly. The local asset receipt
`review/2026-11-08/p8-cloud-assets.json` records the source hashes and scene links.
The local shader receipt `review/2026-11-08/p8-cloud-shader.json` records the
D3D9 and D3D11 programs, source alpha histograms, and numeric checks.

| Behavior | PC evidence |
| --- | --- |
| Density, reuse, movement, fades | `CloudController.Update`, `CreateCloud`, `SetRandomPosition`, `Cloud.DoUpdate` |
| Camera anchor and snap | `Parallax2D.Update` 0x06001135 and `Dejiggler.RoundToPixels` |
| Stable sprite and flip | `RandomizeSprite.Randomize` 0x060012be |
| Wind target, approach and timer | `World.Update` |
| Cumulus exposure and reflection | `ShadowCamera.Awake` and `OnPreRender` 0x06001315–316, `MainCamera.OnPostRender` |
| Scene reload | `Loader` coroutine `MoveNext` 0x06001a35 |

Asset format v8 carries both cloud definitions and uses the converted alpha
coverage. Save format v12 still checks the matching world asset fingerprint.

## Validation

The all-target workspace build, 268 workspace tests, and seven private PC
asset tests passed. Regressions exercise the controller cooldown, density
rounding, variant reuse, fade envelope, wind, camera parallax, tint routing,
alpha conversion and 3DS mask packing. Strict Clippy, ShellCheck, publication
file checks and whitespace checks passed.

Ten desktop captures compare an authored boss wave and a control wave under
the same cloudy weather. The capture checks matched cloud geometry and
isolates each red-moon color by restoring its same-tick control value. At full
weight, the cumulus highlight changes 7,043 sky pixels and the stratus tint
changes 1,721. Both effects clear after the dawn fade. A separate 200-frame
desktop smoke run passed.

The 3DS release build contains the exact v8 world, sheet and seven texture
pages from the fresh asset pack. The packaged executable reached gameplay and
autosaved in Azahar. Two isolated saves with the same frozen weather exercised
the Vulkan 3DS rendering path. The captures show the stratus tint and cumulus
rim color, with the cloud shapes reflected in the water. The build, capture
and asset receipts are under `target/p8-cloud-validation/`. Physical 3DS
validation remains outstanding.
