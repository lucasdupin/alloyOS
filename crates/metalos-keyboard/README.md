# metalOS Keyboard

An independent `no_std` workspace project for the soft keyboard. It owns the
transparent white-label layout, Shift and symbol pages, tap pulses, reveal and
scroll animation, dismissal, typed key actions, and liquid displacement map.
Character taps also lift a large white preview above the finger on a small
borderless dark surface, then fade it away. The preview holds long enough for
quick taps, keeps its original character when Shift resets, and leaves hit
areas fixed. Action keys keep their label expansion without a character popup.
It does not depend on Ferrofluid, a transcript, an editor, or an AI provider.

`ui/keyboard.slint` exports `SoftKeyboard`. The host supplies `requested`
(editor focus), `suspended` (e.g. boot), and `scroll-offset` in logical pixels.
The component exposes `shown` and animated `reveal`; anchor its `y` to
`parent.height - self.reveal` and reserve `reveal` above it for the host content.
`scroll-retreat` defaults to 156px, and keyboard height defaults to 260px.
`key(string)` emits text or the reserved `backspace` and `return` actions;
`SoftKey` translates these into typed actions and Slint key text. `dismiss()`
lets the host relinquish editor focus; `reopen()` clears manual dismissal.

Dependents use the Cargo dependency `metalos-keyboard`. The build script exports
`DEP_METALOS_KEYBOARD_SLINT_LIBRARY`, which the host's Slint compiler registers
as the `metalos-keyboard` library. It also exports
`DEP_METALOS_KEYBOARD_FONT_SIZES`; append this to `SLINT_FONT_SIZES` before
compiling the host scene. These embedded bitmap sizes are required for visible
label scaling on the software renderer. Import the component with
`import { SoftKeyboard } from "@metalos-keyboard/keyboard.slint";`.

`LiquidDisplacement` is the rendering interface. Call `advance(time, reveal,
suspended)` after Slint advances animations. Its returned physical row range
covers both old and new deformation and must be repainted, including the final
settling frame. Sample the pristine Slint RGB buffer with `pixel()` and map
pointer coordinates with `source()` before dispatching input. The host keeps
ownership of the window, framebuffer, and damage presentation. Never sample a
previously displaced output buffer.

The standalone playground compiles and renders independently of System UI:

```sh
cargo test -p metalos-keyboard --features preview
cargo check -p metalos-keyboard --target aarch64-unknown-none
cargo run -p metalos-keyboard --features preview --example keyboard_preview -- /tmp/keyboard.ppm
cargo run -p metalos-keyboard --features preview --example keyboard_preview -- /tmp/pressed.ppm --pressed
cargo run -p metalos-keyboard --features preview --example keyboard_preview -- /tmp/liquid.ppm --liquid
cargo run -p metalos-keyboard --features preview --example keyboard_preview -- /tmp/scrolled.ppm --scroll
```

`ui/preview.slint` is a small keyboard-only scene, enabled by the optional
`preview` feature so its fonts and scene are excluded from production builds. Layouts and gestures belong
in `ui/keyboard.slint`; motion and sampling in `src/motion.rs`; action contracts
in `src/lib.rs`. Slint remains pinned to the workspace's 1.18.1 software renderer
and single-context execution model. All UI calls must stay on the UI owner.
