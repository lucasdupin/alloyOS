# Quicksand UI font

`Quicksand.ttf` is the original Quicksand variable font from
[Google Fonts](https://github.com/google/fonts/tree/main/ofl/quicksand).
It is licensed under the SIL Open Font License 1.1 (see `Quicksand-OFL.txt`).

Ferrofluid's Slint UI imports this file and uses Quicksand as its window default,
including standard controls. Body text uses medium (500); headings and the
logotype use bold (700). `crates/alloyos-ferrofluid/build.rs` embeds font glyphs
at the shared phone display scale using Slint's software-renderer compiler.
The runtime does not need font files, a host font service, or network access.

The old custom coverage atlases and their generator have been removed.
To regenerate the embedded Slint assets, rebuild `alloyos-ferrofluid`.

Render a preview with the actual Slint adapter:

```sh
cargo run -p alloyos-ferrofluid --example system_ui_preview -- /tmp/system-ui.ppm
```
