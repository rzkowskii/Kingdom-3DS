# Tight cloud meshes and fragment exposure

The six cloud sprites use the vertices and triangle indices from the shipped
Unity sprite records. The extractor verified the vertex and index buffers in
the private export against the original serialized assets. `SpriteRenderer.flipX`
mirrors the mesh around its pivot. Each vertex retains its original texture
coordinate, derived from the sprite's authored pixel-to-atlas transform.

| Sprite | Vertices | Triangles |
| --- | ---: | ---: |
| `clouds_01` | 8 | 6 |
| `clouds_02` | 8 | 6 |
| `clouds_03` | 9 | 7 |
| `clouds_04` | 11 | 9 |
| `clouds_stratus_01` | 7 | 5 |
| `clouds_stratus_02` | 11 | 9 |

The optional `cloud-meshes.bin` stores the geometry by world sprite ID. The 3DS
packer preserves the six crop bounds when the mesh asset is present. The v8
world file stays byte-identical, including its save fingerprint. Older packs
without the mesh asset still render with rectangular geometry.

The shadow camera writes depth only inside the authored triangles. It
perspective-corrects the caster texture lookup before applying the alpha cutoff.
On the receiver, the port follows the compiled PC vertex shader and divides
shadow UV by the shadow camera's W at each mesh vertex. The orthographic main
camera interpolates those UVs and clip Z across the triangle. Two bilinearly
filtered depth taps decide exposure at each fragment.

Desktop evaluates the taps in GLSL. The 3DS computes the same comparison at
400×240 pixel centers and uploads an A8 exposure atlas. The PICA200 combiners
apply exposure to the additive rim while preserving the base color and alpha.
This build used the same center-view mask for both stereo eyes. The next pass
adds [per-eye exposure sampling](p11-stereo-cloud-shadows.md). Half exposure
is quantized to 128/255 on the handheld. A receiver that exceeds atlas
capacity falls back to full exposure.

## Evidence and validation

The local REA bundle `review/2026-11-08/p10-cloud-mesh-evidence.json` retains
148 records. The local mesh receipt `review/2026-11-08/p10-cloud-mesh-source.json`
records buffer hashes and serialized sprite layout. The local fragment receipt
`review/2026-11-08/p10-cloud-fragment-evidence.json` records the shipped shader
instructions, crop checks, and a triangle interpolation reference.

Regressions cover sidecar pairing and malformed geometry, caster and receiver
triangle coverage, mirroring, and the order of projection and interpolation.
The desktop GLSL probe measured zero, half and full exposure in three fragments
of one source texel. Ten live checkpoints passed. The fixed-frame P9 comparison
changed shadowed pixels while the full-exposure renders matched.

The all-target workspace build, 277 workspace tests, eight private PC asset
tests, and strict Clippy passed. The 3DS release embeds the mesh sidecar,
byte-identical v8 world, updated sheet and all seven packed texture pages.
Azahar Vulkan loaded both existing v8 saves and rendered clouds and water
reflections in normal weather and during red moon. Wall-clock-aligned
captures showed a localized rim change with matching sampled sky and cloud
interiors. The emulator and build VM stopped. Physical 3DS rendering and
timing remain pending. Receipts are under `target/p10-cloud-mesh-validation/`.
