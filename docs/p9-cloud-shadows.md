# Cloud shadows and cumulus exposure

The shipped `ShadowCamera` follows the cumulus controller. Its height is half
the higher of the sun and moon, and it looks toward the controller from 18.31
units behind it. The camera uses a 30° vertical field of view, a 0.3–20 depth
range, and a 512×256 depth target. The PC replacement shader writes the nearest
depth for source alpha of at least 0.1 and sprite opacity of at least 0.5.
Both cloud layers cast. The shipped material inventory limits this pass to
the two cloud prefabs.

The receiver bilinearly samples encoded depth with clamped edges. It takes two
samples two rows apart and tests each against projected clip Z with a strict
0.05 bias. That yields full, half, or zero exposure for the cumulus rim.
The comparison leaves cloud base color and opacity alone. Stratus contributes
its silhouette to the pass. Its binary alpha gives it no visible highlight.

The shared renderer rasterizes caster silhouettes from the converted atlas
and reuses the depth buffer and exposure atlas across frames. It evaluates
receiver exposure at sprite texel centers, including offscreen casters within
the shadow camera. Desktop samples the mask in its cloud material. The 3DS
uploads the active rows of an A8 atlas and applies the mask in one sprite draw
with three PICA200 texture-combiner stages. The water uses the same shaded
clouds from the normal reflection capture. The existing v8 world and sheet
remain byte-identical, so saves from the cloud-rendering build still load.

The receiver uses the port's sprite quads and nearest exposure masks. The PC
evaluates exposure per fragment on its tight sprite meshes. The depth math
follows the recovered D3D9 pass. The original D3D11 depth binding was inspected
statically, with no runtime depth readback.

## Evidence and validation

The local REA bundle `review/2026-11-08/p9-cloud-shadow-evidence.json` retains
142 records. The local caster receipt `review/2026-11-08/p9-shadow-caster.json`
records the shipped replacement shader, material scan, and threshold math. The
local projection receipt `review/2026-11-08/p9-shadow-projection.json` records
the serialized camera, depth target, and receiver instructions.

The workspace build and 273 tests passed, with seven private PC asset tests.
New regressions cover projection, the cast cutoffs, flipped silhouettes,
nearest-depth selection, bilinear filtering and the two-sample threshold.
Ten desktop checkpoints showed darkened rims with unchanged alpha. At full
red moon the mask affected 1,123 sky pixels. The same-weather control affected
1,145. These comparisons keep the frame fixed and toggle exposure only.

The release rebuilt with the previous v8 RomFS. Both cloudy-night saves from
the prior build loaded in Azahar and rendered the clouds and their reflections
with the Vulkan backend. Aligned rim samples darkened in the ordinary and
boss scenes, while sampled cloud interiors, stratus and open sky matched.
Azahar captures were taken at wall-clock times. The desktop comparison used
the same game frame. The build VM and emulator runs stopped. Physical 3DS
rendering and timing are still pending. Build and capture receipts are under
`target/p9-cloud-shadow-validation/`.
