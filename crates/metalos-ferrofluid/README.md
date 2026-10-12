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
via `metalos-proto::display` in `build.rs`; the adapter rejects other scale factors to avoid mismatched glyphs.

All Slint calls run on the boot/UI execution context. The runtime uses
`unsafe-single-threaded` for `no_std`; never call it from an interrupt or another
task, move it into a shared driver static, or add an unsafe `Send` implementation.
Input and timers remain local, and submitting via Return or Send queues a message
for the owner. The current kernel bridge displays submissions locally; cloud AI
requests, streaming updates, and agent view commands are not connected yet.
The loop polls at up to 60 Hz until timer and input interrupt wakeups are available.

The soft keyboard lives in the independent [`metalos-keyboard`](../metalos-keyboard/README.md)
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

# Serial debug shell (also selected by the existing shell E2E suite)
cargo run -p xtask -- run --debug-shell

# Adapter/input and rendering tests
cargo test -p metalos-ferrofluid

# Render the same bare-metal adapter to a PPM without a desktop backend
cargo run -p metalos-ferrofluid --example system_ui_preview -- /tmp/system-ui.ppm

# Preview the focused composer and soft keyboard
cargo run -p metalos-ferrofluid --example system_ui_preview -- /tmp/keyboard.ppm --keyboard

# Capture mid-slide displacement or an expanded key
cargo run -p metalos-ferrofluid --example system_ui_preview -- /tmp/liquid.ppm --keyboard --liquid
cargo run -p metalos-ferrofluid --example system_ui_preview -- /tmp/pressed.ppm --keyboard --pressed
```

Slint is pinned to 1.18.1 with default features disabled. There is no desktop
window backend, JVM, browser, or host font access in the bare-metal runtime.
See [Slint's upstream license](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSE.md)
for distribution terms; Quicksand's OFL is included in `assets/fonts` at the workspace root.
