# PC fidelity: portal transitions, recoil and remaining P2 findings

Historical checkpoint: [the next physics pass](p3-catapult-physics.md) replaces
the constrained catapult model, adds squid scan/chase timing and supersedes this
document's asset/save versions. The validation below records this earlier pass.

This continues [the asset and animation phase](p2-assets-animation.md). It
implements portal transition clocks and catapult body recoil, plus the bounded
camera, reflection, audio, squid-targeting and diagnostic fixes from F07–F12.
The PC source of truth is the original local Kingdom New Lands 1.2.8 assembly
inspected with REA, together with its exported Unity assets and project settings.

## Behavior changes

### Portal animation and saves

Portal gameplay state and presentation state are separate. Opening takes the
authored 1.1000001 seconds; Close or Destroy triggers received during opening
wait for its exit into Idle. Destroy takes priority over Close there. Closing
takes 1.2 seconds, Destroy takes 1.6875 seconds, and Idle loops over two seconds.
An Open trigger can restart a destroyed/rebuilt portal. A surviving hit opens a
closed portal through the same trigger path. Removing a portal removes its clock.

Extraction verifies the actual controller's transitions, trigger conditions,
speeds, exit time, zero blend durations and offsets. A changed or unsupported
graph fails conversion instead of silently using these rules. Rendering samples
the simulation-owned clip phase, including after pause and save/resume.

Save v8 appends a validated animation extension containing portal phases and
catapult recoil state. The existing serialized game-state prefix stays intact.
Versions 5–7 load through their original layout; old portals migrate to their
authoritative endpoint, and old catapults start with no recoil. Missing,
truncated, nonfinite, inconsistent and trailing extension data are rejected.
Legacy and progress decoding also require an exact payload, rather than ignoring
trailing bytes. Same-tick catapult death removes its recoil before saving.

### Catapult body recoil

The arm-impact marker applies the PC angular impulse, `-0.5 * direction * mass`,
at 0.16666667 seconds. Extraction retains the chassis polygon, body/wheel masses,
wheel radii and hinge anchors, physics timestep and gravity. It verifies the
supported configuration, including drag and joint setup. Unity class 233 is
recognized as `HingeJoint2D`, so the raw prefab importer retains these joints.

The solver derives inertia from that geometry and advances at the authored
0.02-second physics step. The chassis rocks about the leading wheel and settles
under gravity. Body pose is applied after mirroring. Free wheel sprites translate
with their anchors while remaining upright. The projectile's Spoon socket uses
the same live body pose as drawing. Interruption cannot create a delayed impact;
already applied recoil continues independently until it settles or the body dies.

This is a constrained flat-ground model: it uses the PC impulse and geometry but
does not reproduce Unity's full three-body hinge/contact solver, wheel slip or
horizontal body translation. Derived inertia and motion have not been compared
with a PC runtime physics trace. There is no invented recoil spring or sine wave.

### Camera (F07)

The extracted MainCamera curves replace velocity-based anticipation. Lead depends
on time facing one direction, including while standing still: zero through three
seconds, then the authored Hermite curve toward two world units. Reversal is
limited by the PC's five-units-per-second lead speed. Target changes reset lead
and seek from the current view using the distance curve. Changing the lead flag
on the same target also resets anticipation. Pixel rounding uses ties-to-even at
32 pixels per world unit.

Teleporter, departing boat and dropped crown targets disable lead. An inactive
crown preserves the ongoing seek; explicit null targeting clears it. A lost run
returns the view to the monarch after four seconds. Camera progression shares
the fixed simulation clock, so catch-up and pause behave consistently. Resume
snaps to the selected target, avoiding a sweep from the new-session position.

### Reflections, audio and diagnostics (F08–F10, F12)

- Preserve Unity layer 29 geometry and its transform ancestors. Draw it only
  into the reflection capture, without clearing the copied scene or adding a
  second UI/overlay pass. Keep the PC's previous-frame capture order.
- On 3DS, retain independent left/right reflection textures and compose extras
  in an offscreen color/depth target for each eye. The visible framebuffer is
  unaffected. Failed allocation releases partial resources and disables captures
  with a diagnostic. Added capture storage is approximately 1.2 MiB of VRAM.
- Validate streamed-audio headers and declared channel lengths before buffer
  allocation. Read both channels before publishing a chunk; a failed read or
  submission ends the stream and unblocks queued music. A partial stereo chunk
  cannot be published.
- Emit coin-ground sound at the coin's impact position. Preserve the legacy
  serialized event discriminants when adding the positioned event.
- Restrict X/Y rendering diagnostics to debug mode and restore normal rendering
  defaults when leaving it.

### Squid citizen selection (F11)

Squids now use an independent citizen snapshot with actual height, so tower
archers and stunned citizens remain prey. It excludes citizens that are inert,
embarked or already grabbed. Same-tick grabs reserve the chosen citizen before
another squid can select it.

REA corrected the initial review's proposed eligibility filtering: PC code first
selects the closest scanner result or a single overlap collider, then checks
whether that citizen can be grabbed. An ineligible knight can therefore block
that attempt; it must not be skipped to grab a farther citizen. Stunned and inert
are distinct. Equal-depth overlap ties use stable population order here; Unity's
tie order, scanner refresh cadence/result cap and full flight physics remain
outside this correction.

## Evidence

Original `Assembly-CSharp.dll` SHA-256:
`4489a4901fa821def1176c5f58ed53cbe9e4cd129c0f064d61d534a27d1dc0de`.
MVID: `404eb695-3c80-4444-8f66-e84481194f96`.
Reconstructed C# helps navigate, but original CIL and authored asset data are the
evidence for behavior. No original PC executable was run for this phase.

| Behavior | Original evidence |
| --- | --- |
| Portal trigger paths | `Portal.Open` / `Close`, `0x06001280` / `0x06001281`; authored controller and clips |
| Catapult impulse and direction | `Catapult.ArmImpactFromAnimation`, `0x06000b49`; prefab and Physics2D/Time settings |
| Camera curves and target reset | `MainCamera.Update` / `SetTarget`, `0x0600104c`–`0x0600104e`; MainCamera asset |
| Reflection capture order | `MainCamera.OnPostRender`, `0x0600104a`; layer 29 prefab parts |
| Coin sound source position | `Coin.OnCollisionEnter2D`, `0x06000bc7` |
| Single selected citizen, then eligibility | `Squid.ShouldGrabCitizen` / `GrabCitizen`, `0x06001362` / `0x06001363`; scanner/collider code |

The final REA bundle has 38 retained records and is stored locally at
`review/2026-11-08/p2-followup-evidence.json`. Focused evidence receipts include
`catapult-recoil-evidence.json`, `p2-portal-citizens-evidence.json` and
`p2-audio-reflection-evidence.json` in that directory. These private inputs and
receipts remain ignored by Git.

## Asset compatibility and validation

Re-extract and repack together using the commands in the preceding phase's
document. The world/sheet envelope is now **asset version 2**; the clip envelope
remains version 1. Camera, recoil and reflection data change the world payload.
Old or mixed assets fail explicitly. The world fingerprint continues protecting
existing saves: a save tied to different `world.bin` bytes is preserved as a world
mismatch. Save v8's legacy decoding does not bypass that protection. Keep the old
build/assets for existing runs until a separate world migration is available.

Regression coverage exercises controller exit/trigger ordering, fixed-step
recoil and mirrored drawing, live projectile sockets, interruption/death,
save migration and corruption, camera catch-up/targeting/pause, reflection pass
isolation and eye selection, stream failures/music recovery, positioned coin
events, citizen height/eligibility and debug gating. Nineteen isolated mutations
reintroduce the old errors and fail their intended regressions.

Native motion fixtures use actual extracted sprites and production simulation
at 400×240: 360 frames at 30 Hz, with portal opening/destruction and catapult recoil
in both directions. They inject initial states/triggers and hold the camera;
they are silent validation fixtures, not PC comparison recordings or performance
measurements. The receipt records animation and recoil phase for every frame.
Separate desktop smoke testing uses normal simulation with saving/audio off.

Validation outputs are ignored under `target/p2-followup-validation/`, including
the silent motion video, screenshots, command logs, mutation receipts and build
results. Full extraction retains 244 clips and 52 markers; the report still lists
94 unsupported features from the prior phase. Real packing produces seven
texture pages (14.0 MiB) and 296 audio clips for SD storage.

General Unity Animator blending remains unsupported. 3DS GPU/DSP behavior, VRAM
headroom and performance require hardware validation; desktop captures and a
successful 3DS compile do not establish them.

Final checks on November 8, 2026:

- Workspace all-target build and Clippy with warnings denied: passed.
- Workspace tests: 189 passed, zero failed; both opt-in private-reference tests
  passed separately against the local PC export.
- Nineteen isolated mutations: all failed their intended regression tests;
  temporary workspaces were removed.
- Full PC asset extraction and real `pack3ds` conversion: passed.
- Native motion capture: 360 frames completed; opening, queued destruction,
  recoil and mirrored poses inspected alongside the recorded phases. A separate
  200-frame desktop smoke capture completed and was inspected.
- 3DS release compile/link and `.3dsx` packaging with populated RomFS: passed,
  using nightly `2026-11-07`, `cargo-3ds 0.1.5` and the cached devkitPro toolchain
  recorded in the P1 notes. The final artifact checksum is in
  `target/p2-followup-validation/3ds-artifact.json`.
- Publication file guard, ShellCheck and `git diff --check`: passed.

All test and capture processes finished, and the dedicated build VM stopped.
