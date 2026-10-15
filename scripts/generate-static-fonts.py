#!/usr/bin/env python3
"""Generate the two shipped Quicksand faces (requires fonttools).

Static faces prevent Slint's software renderer from rasterizing the variable
font's implicit 400 weight as well as the UI's 500 and 700 weights.
"""
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

fonts = Path(__file__).resolve().parents[1] / "assets/fonts"
for weight, style in [(500, "Medium"), (700, "Bold")]:
    font = instantiateVariableFont(TTFont(fonts / "Quicksand.ttf"), {"wght": weight}, inplace=True)
    family = "Quicksand" if weight == 500 else "Quicksand Bold"
    for name_id, value in [(1, family), (16, family), (2, style), (17, style), (6, f"Quicksand-{style}")]:
        font["name"].setName(value, name_id, 3, 1, 0x409)
    font.save(fonts / f"Quicksand-{style}.ttf")
