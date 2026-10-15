# alloyOS compiler patch (Slint 1.18.1)

The Rust generator emits bitmap glyph bytes as a byte-string literal instead of
one integer expression per byte. The `[u8; N]` static and all glyph metadata are
unchanged. This avoids millions of Rust AST/MIR nodes for the embedded fonts;
it changes generated source representation, not the rasterized image or renderer.

Patch: `generator/rust.rs`, `generate_resources`, `BitmapFontData` arm.
Validate with the Ferrofluid screenshot suite and the cached boot-wordmark test.
Keep this crate pinned with the other Slint 1.18.1 dependencies.
