#!/usr/bin/env python3
"""Check files Kantor encrypted (written by `KANTOR_CRYPT_OUT=DIR cargo test
-p kantor_crypt --test fixtures`): msoffcrypto-tool decrypts each
kantor-*.docx/.xlsx/.pptx with the password `kantor` to exactly the package
Kantor encrypted (DIR/NAME.plain), and LibreOffice opens each with the
password and finds the text "Rahasia Kantor".

Run with a Python that has msoffcrypto-tool and LibreOffice's `uno`
module (CI: a venv with --system-site-packages over Ubuntu's python3-uno):
`python3 tests/corpus/encrypted/check_output.py DIR [soffice]`.
"""
import io
import os
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path

import msoffcrypto

out = Path(sys.argv[1]).resolve()
soffice = sys.argv[2] if len(sys.argv) > 2 else "soffice"
files = sorted(out.glob("kantor-*.*x"))
assert len(files) == 3, files
failed = 0

for f in files:
    with open(f, "rb") as fh:
        o = msoffcrypto.OfficeFile(fh)
        o.load_key(password="kantor")
        buf = io.BytesIO()
        o.decrypt(buf)
    same = buf.getvalue() == (out / (f.name + ".plain")).read_bytes()
    print(f"msoffcrypto-tool {f.name}: {'ok' if same else 'DIFFERENT'}")
    failed += not same

import uno  # noqa: E402  (LibreOffice's Python bridge)
from com.sun.star.beans import PropertyValue  # noqa: E402


def pv(n, v):
    p = PropertyValue()
    p.Name = n
    p.Value = v
    return p


profile = Path(tempfile.mkdtemp())
pipe = "kantorcheck%d" % os.getpid()
proc = subprocess.Popen(
    [soffice, "--headless", "--invisible", "--norestore",
     "-env:UserInstallation=" + profile.as_uri(), "--accept=pipe,name=%s;urp;" % pipe]
)
local = uno.getComponentContext()
resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
for _ in range(240):
    try:
        ctx = resolver.resolve("uno:pipe,name=%s;urp;StarOffice.ComponentContext" % pipe)
        break
    except Exception:
        time.sleep(0.5)
desktop = ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
filters = {".docx": "writer8", ".xlsx": "calc8", ".pptx": "impress8"}
try:
    for f in files:
        doc = desktop.loadComponentFromURL(
            f.as_uri(), "_blank", 0, (pv("Hidden", True), pv("Password", "kantor"))
        )
        if doc is None:
            print(f"LibreOffice {f.name}: DIDN'T OPEN")
            failed += 1
            continue
        dst = out / (f.stem + "-lo." + {".docx": "odt", ".xlsx": "ods", ".pptx": "odp"}[f.suffix])
        doc.storeToURL(dst.as_uri(), (pv("FilterName", filters[f.suffix]),))
        doc.close(True)
        text = zipfile.ZipFile(dst).read("content.xml").decode()
        ok = "Rahasia Kantor" in text
        print(f"LibreOffice {f.name}: {'ok' if ok else 'TEXT MISSING'}")
        failed += not ok
finally:
    try:
        desktop.terminate()
    except Exception:
        pass
    proc.wait(timeout=120)
sys.exit(1 if failed else 0)
