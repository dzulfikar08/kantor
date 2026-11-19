#!/usr/bin/env python3
"""Signed PDF fixtures for kantor_pdf's incremental-save tests, and a check
that a file's signatures are still intact.

Everything is synthetic: a one-page PDF written by hand, a throwaway
self-signed test certificate, signatures made with pyHanko.

    uv run --no-project --with pyhanko python scripts/pdf/signed_fixtures.py make DIR
    uv run --no-project --with pyhanko python scripts/pdf/signed_fixtures.py check FILE CERT [strict]

`make` writes into DIR: test-cert.pem, signed-approval.pdf (a plain
signature), signed-certify-p1.pdf / -p2.pdf / -p3.pdf (certified with
DocMDP permissions 1, 2 and 3; p2 also has an empty text field `name`).
`check` validates every signature of FILE against CERT and prints one line
per signature (`intact valid level docmdp_ok`), exit status 1 if any
signature is broken (with `strict`, also if DocMDP doesn't allow the
changes).
"""

import datetime
import io
import sys
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


def base_pdf(with_field: bool) -> bytes:
    """A one-page PDF with a line of text (and optionally a text field)."""
    content = b"BT /F1 24 Tf 72 700 Td (Signed test document) Tj ET"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R"
        + (b" /AcroForm << /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >>" if with_field else b"")
        + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R"
        + (b" /Annots [6 0 R]" if with_field else b"")
        + b" >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
    ]
    if with_field:
        objs.append(
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [72 600 300 624] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>"
        )
    out = io.BytesIO()
    out.write(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, o in enumerate(objs, 1):
        offsets.append(out.tell())
        out.write(b"%d 0 obj\n" % i + o + b"\nendobj\n")
    xref = out.tell()
    out.write(b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1))
    for off in offsets:
        out.write(b"%010d 00000 n \n" % off)
    out.write(b"trailer\n<< /Size %d /Root 1 0 R /ID [<4b414e544f52> <4b414e544f52>] >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref))
    return out.getvalue()


def make_cert(dir: Path):
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Kantor Test Signer")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=3650))
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .add_extension(
            x509.KeyUsage(True, True, False, False, False, True, False, False, False), critical=True
        )
        .sign(key, hashes.SHA256())
    )
    (dir / "test-cert.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    (dir / "test-key.pem").write_bytes(
        key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
    )


def make(dir: Path):
    from pyhanko.sign import fields, signers
    from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter

    dir.mkdir(parents=True, exist_ok=True)
    make_cert(dir)
    signer = signers.SimpleSigner.load(str(dir / "test-key.pem"), str(dir / "test-cert.pem"))
    jobs = [
        ("signed-approval.pdf", False, None),
        ("signed-certify-p1.pdf", False, fields.MDPPerm.NO_CHANGES),
        ("signed-certify-p2.pdf", True, fields.MDPPerm.FILL_FORMS),
        ("signed-certify-p3.pdf", False, fields.MDPPerm.ANNOTATE),
    ]
    for name, with_field, perm in jobs:
        w = IncrementalPdfFileWriter(io.BytesIO(base_pdf(with_field)))
        meta = signers.PdfSignatureMetadata(
            field_name="Signature1",
            certify=perm is not None,
            docmdp_permissions=perm or fields.MDPPerm.FILL_FORMS,
        )
        out = signers.sign_pdf(w, meta, signer=signer)
        (dir / name).write_bytes(out.getvalue())
    (dir / "test-key.pem").unlink()
    print(f"wrote fixtures to {dir}")


def check(path: Path, cert_path: Path, strict: bool) -> int:
    from pyhanko.keys import load_cert_from_pemder
    from pyhanko.pdf_utils.reader import PdfFileReader
    from pyhanko.sign.validation import validate_pdf_signature
    from pyhanko_certvalidator import ValidationContext

    cert = load_cert_from_pemder(str(cert_path))
    vc = ValidationContext(trust_roots=[cert])
    bad = 0
    with open(path, "rb") as f:
        r = PdfFileReader(f)
        for sig in r.embedded_signatures:
            st = validate_pdf_signature(sig, vc)
            level = st.modification_level.name if st.modification_level else "NONE"
            print(f"{sig.field_name}: intact={st.intact} valid={st.valid} level={level} docmdp_ok={st.docmdp_ok}")
            # pyHanko's diff policy has no rule for comments and stamps (a
            # TODO in its form_field_rules.py): they report level OTHER even
            # when added correctly. `strict` also asks for DocMDP approval
            # (form filling).
            if not (st.intact and st.valid):
                bad += 1
            elif strict and not st.docmdp_ok:
                bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "make":
        make(Path(sys.argv[2]))
    elif len(sys.argv) >= 4 and sys.argv[1] == "check":
        sys.exit(check(Path(sys.argv[2]), Path(sys.argv[3]), "strict" in sys.argv[4:]))
    else:
        print(__doc__)
        sys.exit(2)
