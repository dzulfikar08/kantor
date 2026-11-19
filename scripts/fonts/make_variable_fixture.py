#!/usr/bin/env python3
"""Make tests/fonts/KantorVarTest.ttf: a small variable font for tests.

It is Plus Jakarta Sans (SIL OFL 1.1, no Reserved Font Name; licence in
tests/fonts/OFL.txt) cut down to Basic Latin, with its `wght` axis
(200..800, default 400), HVAR and named instances kept, and renamed
"Kantor Var Test" so tests never pick up an installed Plus Jakarta Sans.

Needs fontTools (`pip install fonttools`):

    python3 scripts/fonts/make_variable_fixture.py /path/to/PlusJakartaSans[wght].ttf

The source is the variable TTF from https://github.com/tokotype/PlusJakartaSans
(fonts/variable/PlusJakartaSans[wght].ttf) or Google Fonts.
"""

import os
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

FAMILY = "Kantor Var Test"
PS_FAMILY = "KantorVarTest"


def main() -> None:
    src = sys.argv[1]
    out = os.path.join(
        os.path.dirname(__file__), "..", "..", "tests", "fonts", "KantorVarTest.ttf"
    )
    font = TTFont(src)
    assert "fvar" in font, "not a variable font"
    opts = subset.Options()
    opts.layout_features = ["kern"]
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    opts.hinting = False
    opts.desubroutinize = True
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=range(0x20, 0x7F))
    sub.subset(font)

    name = font["name"]
    for rec in list(name.names):
        if rec.nameID in (1, 4, 6, 16, 17, 25) or rec.nameID == 3:
            name.removeNames(nameID=rec.nameID)
    name.setName(FAMILY, 1, 3, 1, 0x409)
    name.setName(f"{FAMILY} Regular", 4, 3, 1, 0x409)
    name.setName(f"{PS_FAMILY}-Regular", 6, 3, 1, 0x409)
    name.setName(f"{PS_FAMILY}-Regular;fixture", 3, 3, 1, 0x409)
    name.setName(PS_FAMILY, 25, 3, 1, 0x409)
    if "STAT" in font:
        # STAT names point at name records that stay; nothing to rename.
        pass
    font.save(out)
    print(out, os.path.getsize(out), "bytes")


if __name__ == "__main__":
    main()
