# Ferrofluid System UI

The boot logotype uses subtle, layered turbulence and falling white droplets during loading. The scanout
pass samples the pristine Slint text with bilinear filtering and repaints only
the logo and droplet region between UI updates; the effect stops on workspace transition.

Ferrofluid owns the Slint platform adapter and the single fullscreen System UI
window. The kernel supplies monotonic time, Linux evdev input, and VirtIO-GPU
scanout. The default graphical boot launches this window after the splash.

`src/system_ui.rs` installs Slint's platform, creates `MinimalSoftwareWindow`,
loads `ui/system-ui.slint`, and renders into a persistent RGB buffer. Only damaged
pixels are converted into opaque ARGB8888 for GPU presentation. Fonts and widget
assets are embedded at build time, using the imported Quicksand variable font and the Fluent dark style. Quicksand
is the window default for text and standard controls, with medium (500) body
text and bold (700) headings.
The current display uses 1206x2622 physical pixels at a fixed 3x scale, configured
via `alloyos-proto::display` in `build.rs`; the adapter rejects other scale factors to avoid mismatched glyphs.

All Slint calls run on the UI thread of the protected `/system/ferrofluid` EL0
executable, with a private address space. The runtime uses `unsafe-single-threaded`
for `no_std`; never call it from an interrupt or another task, move it into a
shared driver static, or add an unsafe `Send` implementation. Kernel drivers
retain hardware ownership and expose authorized input, monotonic time and bounded
frame presentation syscalls. The kernel does not link Slint or Ferrofluid.

The Welder host owns conversation and dashboard state. Bounded display IPC carries
submissions, replies, streaming text, model selection, tiles and actions. A display
fault or expired execution lease retires its process; the supervisor reloads its
retained validated ELF and restores the host’s current state with bounded retry/backoff.
The loop polls at up to 60 Hz until input interrupt wakeups are available.

Rebuild/package UI code, fonts and assets with `cargo xtask processes --release`;
this leaves the kernel executable unchanged when the shared ABI is unchanged.
Enter `/display restart` in the composer to restart Ferrofluid without rebooting.
The supervisor retains the validated executable bytes for restart, avoiding a
large contiguous filesystem-read allocation after normal UI/app activity.
Stop the guest before updating its CobaltFS image, then launch it again to load the
new display executable. A complete build is `cargo xtask build --release`. Protected executables use
optimized, stripped builds even when the kernel uses the default debug profile.

The soft keyboard lives in the independent [`alloyos-keyboard`](../alloyos-keyboard/README.md)
project. System UI imports its Slint component and supplies editor focus, boot
suspension, and transcript scroll position. The component owns key layout,
press feedback, reveal/scroll policy, and dismissal. Typed actions are forwarded
to the focused Slint editor; the keyboard crate's displacement map handles scene
sampling and inverse-mapped touches. Ferrofluid owns scanout and repaints the
map's returned damage band. See the keyboard project for independent tests and
the standalone playground.

Slint is the sole UI toolkit. The previous custom compositor, framebuffer, font
renderer, widgets, and simulated UI IPC have been removed. `auth.rs` retains the
agent capability gate independently of rendering. Debug-shell builds use serial
I/O only; shell output is never painted into System UI.

```sh
# Boot System UI
cargo run -p xtask -- run

# Rebuild and package protected executables without building the kernel
cargo xtask processes --release

# Full process runtime and recovery acceptance
python3 scripts/test-process-runtime.py

# Serial debug shell (also selected by the existing shell E2E suite)
cargo run -p xtask -- run --debug-shell

# Adapter/input and rendering tests
cargo test -p alloyos-ferrofluid

# Render the same bare-metal adapter to a PPM without a desktop backend
cargo run -p alloyos-ferrofluid --example system_ui_preview -- /tmp/system-ui.ppm

# Preview the focused composer and soft keyboard
cargo run -p alloyos-ferrofluid --example system_ui_preview -- /tmp/keyboard.ppm --keyboard

# Capture mid-slide displacement or an expanded key
cargo run -p alloyos-ferrofluid --example system_ui_preview -- /tmp/liquid.ppm --keyboard --liquid
cargo run -p alloyos-ferrofluid --example system_ui_preview -- /tmp/pressed.ppm --keyboard --pressed
```

Slint is pinned to 1.18.1 with default features disabled. There is no desktop
window backend, JVM, browser, or host font access in the bare-metal runtime.
See [Slint's upstream license](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSE.md)
for distribution terms; Quicksand's OFL is included in `assets/fonts` at the workspace root.

The workspace centers on one ordered conversation containing text and app tiles.
Pinned tiles live in a left-edge drawer with an independent vertical scroll.
Drag the edge handle to pull it out; release snaps it open or closed. Pull its
exposed boundary left, tap the backdrop, use its close button, or press Escape
to dismiss it. The keyboard releases focus when an edge pull starts.

The drawer rotates the keyboard's liquid displacement field onto the horizontal
axis, including inverse-mapped input and restoration of the warped damage band.
While visible, scanout captures the pristine workspace with the drawer omitted,
builds a downsampled separable backdrop blur, and slightly fades the whole
chat scene into the workspace background. The drawer has no background or
border; sharp pinned tiles float above the blurred, liquid-transformed chat. Blur never samples a previous blurred frame. A settled
sidebar does not schedule continuous redraws. The blurred backdrop is cached
during pulls and scrolling; chat changes, pending replies, and keyboard motion
refresh it.

Tile management (pin, unpin, move to top, and resource refresh) lives under the
**…** menu, while app actions remain visible. Tap the model name in the header to change models. The standalone transcript and tile inputs remain compatibility
fallbacks when no ordered chat items exist.

Tap the model name in the header to switch models. Model changes are disabled
while a reply is pending.

Chat text and streaming transcripts support a small Markdown subset on the
`no_std` runtime: ATX headings (`#` through `######`), unordered lists, ordered
lists (preserving their numbers), block quotes, fenced code blocks, and bold
whole lines, list items, and inline spans (`**bold**` followed by normal text).
Inline bold uses font measurements from Slint to wrap styled words. Code contents remain literal, including unfinished
fences during streaming. Other inline formatting, links, images, and tables
remain visible as source text. Slint's built-in Markdown parser requires `std`
and is not used by the bare-metal UI.
