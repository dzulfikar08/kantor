#!/usr/bin/env python3
"""Write the password-protected fixtures (CC0, synthetic): tests/corpus/encrypted/,
read by crates/kantor_crypt/tests/fixtures.rs and the engines' password tests.

Every file holds the made-up text "Rahasia Kantor" and opens with the
password `kantor`:

* agile.docx / agile.xlsx / agile.pptx: Office 2010+ "Agile" encryption
  (AES-256, SHA-512) written by msoffcrypto-tool.
* standard.docx / standard.xlsx: Office 2007 "Standard" encryption (AES-128
  ECB, SHA-1), written by this script with `cryptography` and
  msoffcrypto-tool's compound-file writer, then checked by msoffcrypto-tool.
* lo.docx / lo.xlsx / lo.pptx: OOXML files LibreOffice 25.8 encrypts itself.
* aead.odt / aead.ods / aead.odp: LibreOffice 25.8's default ODF encryption
  (ODF 1.4 extended: the whole package in `encrypted-package`, AES-256-GCM,
  Argon2id).
* cbc.odt / cbc.ods / cbc.odp: per-entry AES-256-CBC with PBKDF2 and a
  SHA-256 start key (LibreOffice 3.4 - 24.2; saved here as ODF 1.3 extended).
* blowfish.odt: per-entry Blowfish CFB with PBKDF2 and a SHA-1 start key
  (OpenOffice.org; saved here as ODF 1.0/1.1).

Regenerate with
`uv run --no-project --with msoffcrypto-tool==5.4.2 --with cryptography --with python-docx --with openpyxl --with python-pptx python3 tests/corpus/encrypted/make_fixtures.py tests/corpus/encrypted [LO program dir]`
where the LibreOffice program dir defaults to the extracted 25.8 AppImage
(~/.cache/lo/squashfs-root/opt/libreoffice25.8/program; extract it with
`lo.AppImage --appimage-extract`). Salts are random, so bytes differ
between runs.
"""
import hashlib
import io
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

import docx
import msoffcrypto
import openpyxl
import pptx
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from msoffcrypto.method.container.ecma376_encrypted import ECMA376Encrypted

PASSWORD = "kantor"
out = Path(sys.argv[1] if len(sys.argv) > 1 else "tests/corpus/encrypted").resolve()
lo_program = Path(
    sys.argv[2]
    if len(sys.argv) > 2
    else Path.home() / ".cache/lo/squashfs-root/opt/libreoffice25.8/program"
)
out.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp(prefix="kantor-enc-", dir=Path.home() / ".cache"))


def plain_files():
    d = docx.Document()
    d.add_heading("Rahasia Kantor", 1)
    d.add_paragraph("Laporan rahasia untuk tes kata sandi.")
    d.save(work / "secret.docx")
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Data"
    ws["A1"] = "Rahasia Kantor"
    ws["B2"] = 42
    wb.save(work / "secret.xlsx")
    p = pptx.Presentation()
    s = p.slides.add_slide(p.slide_layouts[1])
    s.shapes.title.text = "Rahasia Kantor"
    s.placeholders[1].text = "Slide rahasia"
    p.save(work / "secret.pptx")


def agile(ext):
    with open(work / f"secret.{ext}", "rb") as f:
        o = msoffcrypto.OfficeFile(f)
        with open(out / f"agile.{ext}", "wb") as g:
            o.encrypt(PASSWORD, g)


def aes_ecb(key, data, encrypt=True):
    c = Cipher(algorithms.AES(key), modes.ECB())
    x = c.encryptor() if encrypt else c.decryptor()
    return x.update(data) + x.finalize()


def standard_key(salt):
    h = hashlib.sha1(salt + PASSWORD.encode("utf-16-le")).digest()
    for i in range(50000):
        h = hashlib.sha1(struct.pack("<I", i) + h).digest()
    hfinal = hashlib.sha1(h + struct.pack("<I", 0)).digest()
    buf1 = bytes(a ^ 0x36 for a in hfinal) + b"\x36" * (64 - 20)
    return hashlib.sha1(buf1).digest()[:16]


def standard(ext):
    data = (work / f"secret.{ext}").read_bytes()
    salt = os.urandom(16)
    key = standard_key(salt)
    verifier = os.urandom(16)
    vhash = hashlib.sha1(verifier).digest() + b"\0" * 12
    csp = "Microsoft Enhanced RSA and AES Cryptographic Provider\0".encode("utf-16-le")
    header = struct.pack("<IIIIIIII", 0x24, 0, 0x660E, 0x8004, 128, 0x18, 0, 0) + csp
    info = struct.pack("<HHI", 3, 2, 0x24) + struct.pack("<I", len(header)) + header
    info += struct.pack("<I", 16) + salt + aes_ecb(key, verifier)
    info += struct.pack("<I", 20) + aes_ecb(key, vhash)
    padded = data + b"\0" * (-len(data) % 16)
    package = struct.pack("<Q", len(data)) + aes_ecb(key, padded)
    buf = io.BytesIO()
    ECMA376Encrypted(package, info).write_to(buf)
    (out / f"standard.{ext}").write_bytes(buf.getvalue())
    # msoffcrypto-tool reads it back to the same package.
    with open(out / f"standard.{ext}", "rb") as f:
        o = msoffcrypto.OfficeFile(f)
        o.load_key(password=PASSWORD)
        back = io.BytesIO()
        o.decrypt(back)
    assert back.getvalue() == data, "Standard round trip"


LO_SAVE = r'''
import os, subprocess, sys, time, uno
from com.sun.star.beans import PropertyValue
def pv(n, v):
    p = PropertyValue(); p.Name = n; p.Value = v; return p
prog = os.path.dirname(sys.executable)
out, odf_version, profile = sys.argv[1], sys.argv[2], sys.argv[3]
pipe = "kantorpw%d" % os.getpid()
proc = subprocess.Popen([os.path.join(prog, "soffice"), "--headless", "--invisible", "--norestore",
    "-env:UserInstallation=" + uno.systemPathToFileUrl(profile), "--accept=pipe,name=%s;urp;" % pipe])
local = uno.getComponentContext()
resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
for _ in range(240):
    try:
        ctx = resolver.resolve("uno:pipe,name=%s;urp;StarOffice.ComponentContext" % pipe); break
    except Exception:
        time.sleep(0.5)
smgr = ctx.ServiceManager
cp = smgr.createInstanceWithContext("com.sun.star.configuration.ConfigurationProvider", ctx)
acc = cp.createInstanceWithArguments("com.sun.star.configuration.ConfigurationUpdateAccess",
    (pv("nodepath", "/org.openoffice.Office.Common/Save/ODF"),))
acc.setPropertyValue("DefaultVersion", int(odf_version))
acc.commitChanges()
desktop = smgr.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
try:
    for spec in sys.argv[4:]:
        src, flt, name = spec.split("|")
        doc = desktop.loadComponentFromURL(uno.systemPathToFileUrl(src), "_blank", 0, (pv("Hidden", True),))
        dst = uno.systemPathToFileUrl(os.path.join(out, name))
        doc.storeToURL(dst, (pv("FilterName", flt), pv("Password", "kantor")))
        doc.close(True)
finally:
    try:
        desktop.terminate()
    except Exception:
        pass
    proc.wait(timeout=120)
'''

FILTERS = {
    "odt": "writer8",
    "ods": "calc8",
    "odp": "impress8",
    "docx": "MS Word 2007 XML",
    "xlsx": "Calc MS Excel 2007 XML",
    "pptx": "Impress MS PowerPoint 2007 XML",
}
SOURCE = {"odt": "docx", "ods": "xlsx", "odp": "pptx", "docx": "docx", "xlsx": "xlsx", "pptx": "pptx"}


def libreoffice(odf_version, names):
    script = work / "lo_save.py"
    script.write_text(LO_SAVE)
    specs = []
    for name in names:
        ext = name.rsplit(".", 1)[1]
        specs.append(f"{work / ('secret.' + SOURCE[ext])}|{FILTERS[ext]}|{name}")
    profile = work / f"profile-{odf_version}"
    subprocess.run(
        [str(lo_program / "python"), str(script), str(out), str(odf_version), str(profile), *specs],
        check=True,
        timeout=600,
    )


plain_files()
for ext in ("docx", "xlsx", "pptx"):
    agile(ext)
for ext in ("docx", "xlsx"):
    standard(ext)
# 13 = ODF 1.4 extended (LibreOffice 25.8's default), 11 = ODF 1.3 extended,
# 2 = ODF 1.0/1.1.
libreoffice(13, ["aead.odt", "aead.ods", "aead.odp", "lo.docx", "lo.xlsx", "lo.pptx"])
libreoffice(11, ["cbc.odt", "cbc.ods", "cbc.odp"])
libreoffice(2, ["blowfish.odt"])
shutil.rmtree(work)
for f in sorted(out.iterdir()):
    print(f"{f.name:16} {f.stat().st_size:7}")
