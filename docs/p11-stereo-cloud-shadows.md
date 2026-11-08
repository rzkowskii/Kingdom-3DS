# Per-eye cloud shadow sampling

The shipped PC main camera uses a single orthographic view. Its build settings
have an empty VR device list. `ShadowCamera.OnPreRender` binds one light-space
view-projection matrix. The compiled receiver shader divides its shadow UVs by
W at each mesh vertex and interpolates them across the main view before testing
two filtered depth taps. The port uses the same caster map, projected UVs,
clip Z and depth bias for both eyes. The 3DS stereo presentation adds an
eye-specific screen-space shift. The REA and shipped-asset evidence is stored
locally at `review/2026-11-08/p11-stereo-cloud-evidence.json`.

The shared scene pass builds one depth map and packs a receiver rectangle that
covers the visible bounds in both eyes. It rasterizes each view at its native
400×240 pixel centers, with the same disparity weight that moves the displayed
cloud. The 3DS uploads each mask to a separate A8 texture and reuses the CPU
buffer between uploads. Its vertex shader uses the shifted screen position
for both rasterization and exposure lookup. Right-eye replay binds the right
mask, and the reflection pass captures the shaded image for each eye.

A zero slider uses one mask. The second GPU texture is allocated on first use
in stereo. If that allocation fails, the right eye uses full rim exposure
while the left keeps its shadows. A receiver that exceeds atlas capacity also
uses full exposure. The release keeps the byte-identical v8 world, sheet,
mesh sidecar and texture pages from the tight-mesh build.

## Validation

The regression fixtures exercise a fractional shift that makes the same
screen pixel fully lit in one eye and fully shadowed in the other. A second
fixture covers a receiver visible only in the right eye and checks that
returning to mono clears the stereo layout. The fixtures also check shared
caster depth and matching zero-disparity masks.

The all-target workspace build, 279 workspace tests, eight private PC asset
tests, and strict Clippy passed. The populated 3DS release loaded both
existing v8 saves in Azahar Vulkan. Side-by-side captures at a 67% guest
slider showed distinct eye views with clouds and water reflections in both
ordinary weather and red moon. Each of the six captures measured a six-pixel
sky disparity. An aligned capture from the previous build showed localized
rim changes in both eyes. The emulator comparison aligns wall-clock captures,
while the regression fixtures compare exact pixel phases. The build VM and
emulator stopped. Physical 3DS rendering and performance remain pending.
The release and receipts are under `target/p11-stereo-cloud-validation/`.
