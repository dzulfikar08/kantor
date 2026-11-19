#!/usr/bin/env python3
"""Make the signed-PDF fixtures for pdf_engine's signature checker.

Everything is synthetic: a made-up test PKI (keys made fresh on each run), a
one-page PDF written by hand, and signatures made with pyHanko. Nothing here
touches the network: the time-stamp authority is pyHanko's local
DummyTimeStamper, and OCSP responses and CRLs are built with `cryptography`.

    python3 -m venv ~/.cache/tap/venv && ~/.cache/tap/venv/bin/pip install pyhanko==0.38.1
    ~/.cache/tap/venv/bin/python scripts/pdf/gen_signed_fixtures.py

writes crates/pdf_engine/tests/fixtures/signatures/*.pdf and test-root.pem
(the root the tests trust). Re-running changes every file (new keys); commit
them together.
"""

import datetime as dt
import io
import os
import sys

from asn1crypto import keys as a_keys, x509 as a_x509, ocsp as a_ocsp, crl as a_crl
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa
from cryptography.x509 import ocsp
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID, AuthorityInformationAccessOID
from pyhanko.pdf_utils import generic as g
from pyhanko.pdf_utils.incremental_writer import IncrementalPdfFileWriter
from pyhanko.sign import fields, signers
from pyhanko.sign.signers.pdf_signer import PdfSignatureMetadata, PdfTimeStamper
from pyhanko.sign.timestamps.dummy_client import DummyTimeStamper
from pyhanko.sign.validation.dss import DocumentSecurityStore
from pyhanko_certvalidator.registry import SimpleCertificateStore

OUT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "..", "crates", "pdf_engine", "tests", "fixtures", "signatures",
)
NOT_BEFORE = dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc)
NOT_AFTER = dt.datetime(2046, 1, 1, tzinfo=dt.timezone.utc)
OCSP_URL = "http://ocsp.kantor-test.invalid"
CRL_URL = "http://crl.kantor-test.invalid/issuing.crl"


def name(cn, org="Kantor Test PKI"):
    return x509.Name([
        x509.NameAttribute(NameOID.COUNTRY_NAME, "ID"),
        x509.NameAttribute(NameOID.ORGANIZATION_NAME, org),
        x509.NameAttribute(NameOID.COMMON_NAME, cn),
    ])


def make_cert(subject, key, issuer=None, issuer_key=None, ca=False, eku=None,
              ocsp_url=None, crl_url=None, ku=None):
    issuer = issuer or (subject, key)
    iss_name, iss_key = issuer
    b = (x509.CertificateBuilder()
         .subject_name(subject)
         .issuer_name(iss_name)
         .public_key(key.public_key())
         .serial_number(x509.random_serial_number())
         .not_valid_before(NOT_BEFORE)
         .not_valid_after(NOT_AFTER)
         .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True)
         .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
         .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(iss_key.public_key()),
                        critical=False))
    if ca:
        ku = ku or dict(digital_signature=False, key_cert_sign=True, crl_sign=True)
    else:
        ku = ku or dict(digital_signature=True, content_commitment=True)
    full = dict(digital_signature=False, content_commitment=False, key_encipherment=False,
                data_encipherment=False, key_agreement=False, key_cert_sign=False,
                crl_sign=False, encipher_only=False, decipher_only=False)
    full.update(ku)
    b = b.add_extension(x509.KeyUsage(**full), critical=True)
    if eku:
        b = b.add_extension(x509.ExtendedKeyUsage(eku), critical=True)
    if ocsp_url:
        b = b.add_extension(x509.AuthorityInformationAccess([
            x509.AccessDescription(AuthorityInformationAccessOID.OCSP,
                                   x509.UniformResourceIdentifier(ocsp_url))]), critical=False)
    if crl_url:
        b = b.add_extension(x509.CRLDistributionPoints([
            x509.DistributionPoint([x509.UniformResourceIdentifier(crl_url)], None, None, None)]),
            critical=False)
    return b.sign(issuer_key or iss_key, hashes.SHA256())


def rsa_key():
    return rsa.generate_private_key(public_exponent=65537, key_size=2048)


def to_a_cert(c):
    return a_x509.Certificate.load(c.public_bytes(serialization.Encoding.DER))


def to_a_key(k):
    return a_keys.PrivateKeyInfo.load(k.private_bytes(
        serialization.Encoding.DER, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption()))


def base_pdf(form=False):
    """A one-page PDF (uncompressed, so a test can change a byte of the text)."""
    content = b"BT /F1 24 Tf 72 720 Td (Kantor signed test document) Tj ET\n" \
              b"BT /F1 12 Tf 72 690 Td (Synthetic fixture for the signature checker.) Tj ET\n"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R" + (b" /AcroForm 6 0 R" if form else b"") + b" >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >>" + (b" /Annots [7 0 R]" if form else b"") + b" >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"endstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    if form:
        objs.append(b"<< /Fields [7 0 R] /DA (/Helv 0 Tf 0 g) "
                    b"/DR << /Font << /Helv 5 0 R >> >> >>")
        objs.append(b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [72 600 300 624] "
                    b"/P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>")
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
    out.write(b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n"
              % (len(objs) + 1, xref))
    return out.getvalue()


def sign(data, signer, *, field="Signature1", subfilter=fields.SigSeedSubFilter.PADES,
         certify=False, perms=None, timestamper=None, reason="I approve this document",
         location="Jakarta"):
    w = IncrementalPdfFileWriter(io.BytesIO(data))
    meta = PdfSignatureMetadata(field_name=field, subfilter=subfilter, reason=reason,
                                location=location, certify=certify,
                                docmdp_permissions=perms or fields.MDPPerm.FILL_FORMS)
    out = signers.sign_pdf(w, meta, signer=signer, timestamper=timestamper)
    return out.getvalue()


def add_stamp(data):
    """Add a Stamp annotation (with an appearance) in a new revision."""
    w = IncrementalPdfFileWriter(io.BytesIO(data))
    pref = w.root["/Pages"]["/Kids"][0]
    page = pref.get_object()
    ap = g.StreamObject(g.DictionaryObject({
        g.NameObject("/Type"): g.NameObject("/XObject"),
        g.NameObject("/Subtype"): g.NameObject("/Form"),
        g.NameObject("/BBox"): g.ArrayObject([g.FloatObject(v) for v in (0, 0, 120, 40)]),
    }), stream_data=b"1 0 0 RG 3 w 2 2 116 36 re S")
    apref = w.add_object(ap)
    annot = w.add_object(g.DictionaryObject({
        g.NameObject("/Type"): g.NameObject("/Annot"),
        g.NameObject("/Subtype"): g.NameObject("/Stamp"),
        g.NameObject("/Rect"): g.ArrayObject([g.FloatObject(v) for v in (400, 600, 520, 640)]),
        g.NameObject("/F"): g.NumberObject(4),
        g.NameObject("/P"): pref,
        g.NameObject("/AP"): g.DictionaryObject({g.NameObject("/N"): apref}),
    }))
    annots = page.get("/Annots")
    if isinstance(annots, g.IndirectObject):
        arr = annots.get_object()
        arr.append(annot)
        w.mark_update(annots)
    else:
        arr = g.ArrayObject(list(annots or []))
        arr.append(annot)
        page[g.NameObject("/Annots")] = arr
        w.update_container(page)
    out = io.BytesIO()
    w.write(out)
    return out.getvalue()


def change_content(data):
    """Replace the page's content stream in a new revision."""
    w = IncrementalPdfFileWriter(io.BytesIO(data))
    page = w.root["/Pages"]["/Kids"][0].get_object()
    new = w.add_object(g.StreamObject(stream_data=b"BT /F1 24 Tf 72 720 Td (Changed after signing) Tj ET\n"))
    page[g.NameObject("/Contents")] = new
    w.update_container(page)
    out = io.BytesIO()
    w.write(out)
    return out.getvalue()


def fill_name(data, value):
    """Fill the "Name" text field in a new revision (value only, like a form filler)."""
    w = IncrementalPdfFileWriter(io.BytesIO(data))
    for ref in w.root["/AcroForm"]["/Fields"]:
        f = ref.get_object()
        if f.get("/T") == "Name":
            f[g.NameObject("/V")] = g.TextStringObject(value)
            w.mark_update(ref)
    out = io.BytesIO()
    w.write(out)
    return out.getvalue()


def add_dss(data, certs=(), ocsps=(), crls=()):
    buf = io.BytesIO(data)
    from pyhanko.pdf_utils.reader import PdfFileReader
    r = PdfFileReader(buf)
    sig = r.embedded_signatures[-1]
    DocumentSecurityStore.add_dss(buf, sig.pkcs7_content, certs=list(certs), ocsps=list(ocsps),
                                  crls=list(crls))
    return buf.getvalue()


def main():
    os.makedirs(OUT, exist_ok=True)
    now = dt.datetime.now(dt.timezone.utc)

    root_k = rsa_key()
    root = make_cert(name("Kantor Test Root CA"), root_k, ca=True)
    inter_k = ec.generate_private_key(ec.SECP256R1())
    inter = make_cert(name("Kantor Test Issuing CA"), inter_k, (root.subject, root_k), ca=True)
    iss = (inter.subject, inter_k)

    leaf_k = rsa_key()
    leaf = make_cert(name("Budi Santoso", "PT Contoh Uji"), leaf_k, iss)
    leaf_ec_k = ec.generate_private_key(ec.SECP256R1())
    leaf_ec = make_cert(name("Siti Rahma", "PT Contoh Uji"), leaf_ec_k, iss)
    leaf_rev_k = rsa_key()
    leaf_rev = make_cert(name("Dewi Lestari", "PT Contoh Uji"), leaf_rev_k, iss, ocsp_url=OCSP_URL)
    leaf_crl_k = rsa_key()
    leaf_crl = make_cert(name("Agus Wijaya", "PT Contoh Uji"), leaf_crl_k, iss, crl_url=CRL_URL)
    leaf_bad_k = rsa_key()
    leaf_bad = make_cert(name("Rudi Hartono", "PT Contoh Uji"), leaf_bad_k, iss, ocsp_url=OCSP_URL)
    tsa_k = rsa_key()
    tsa = make_cert(name("Kantor Test Time Stamping"), tsa_k, (root.subject, root_k),
                    eku=[ExtendedKeyUsageOID.TIME_STAMPING],
                    ku=dict(digital_signature=True, content_commitment=True))
    other_root_k = rsa_key()
    other_root = make_cert(name("Unknown Root CA", "Somebody Else"), other_root_k, ca=True)
    stranger_k = rsa_key()
    stranger = make_cert(name("Stranger", "Somebody Else"), stranger_k,
                         (other_root.subject, other_root_k))

    with open(os.path.join(OUT, "test-root.pem"), "wb") as f:
        f.write(root.public_bytes(serialization.Encoding.PEM))

    store = SimpleCertificateStore.from_certs([to_a_cert(inter)])

    def signer(cert, key, chain=store):
        return signers.SimpleSigner(signing_cert=to_a_cert(cert), signing_key=to_a_key(key),
                                    cert_registry=chain)

    ts = DummyTimeStamper(to_a_cert(tsa), to_a_key(tsa_k),
                          certs_to_embed=SimpleCertificateStore.from_certs([to_a_cert(tsa)]))

    def ocsp_resp(cert, status, rev_time=None):
        b = ocsp.OCSPResponseBuilder().add_response(
            cert=cert, issuer=inter, algorithm=hashes.SHA1(), cert_status=status,
            this_update=now, next_update=now + dt.timedelta(days=7),
            revocation_time=rev_time,
            revocation_reason=x509.ReasonFlags.key_compromise if rev_time else None,
        ).responder_id(ocsp.OCSPResponderEncoding.HASH, inter)
        der = b.sign(inter_k, hashes.SHA256()).public_bytes(serialization.Encoding.DER)
        return a_ocsp.OCSPResponse.load(der)

    def crl_of_inter(revoked=()):
        b = (x509.CertificateRevocationListBuilder().issuer_name(inter.subject)
             .last_update(now).next_update(now + dt.timedelta(days=7)))
        for c in revoked:
            b = b.add_revoked_certificate(
                x509.RevokedCertificateBuilder().serial_number(c.serial_number)
                .revocation_date(dt.datetime(2026, 6, 1, tzinfo=dt.timezone.utc)).build())
        return a_crl.CertificateList.load(b.sign(inter_k, hashes.SHA256())
                                          .public_bytes(serialization.Encoding.DER))

    base = base_pdf()
    files = {}
    files["unsigned"] = base
    valid = sign(base, signer(leaf, leaf_k))
    files["valid"] = valid
    files["valid_pkcs7"] = sign(base, signer(leaf_ec, leaf_ec_k),
                                subfilter=fields.SigSeedSubFilter.ADOBE_PKCS7_DETACHED)
    i = valid.index(b"(Kantor signed test document)")
    files["tampered"] = valid[:i + 1] + b"k" + valid[i + 2:]
    files["wrong_cert"] = sign(base, signer(leaf, stranger_k))
    files["untrusted"] = sign(base, signers.SimpleSigner(
        signing_cert=to_a_cert(stranger), signing_key=to_a_key(stranger_k),
        cert_registry=SimpleCertificateStore.from_certs([to_a_cert(other_root)])))
    files["timestamped"] = sign(base, signer(leaf, leaf_k), timestamper=ts)
    files["doc_timestamp"] = PdfTimeStamper(ts).timestamp_pdf(
        IncrementalPdfFileWriter(io.BytesIO(valid)), "sha256").getvalue()
    files["annot_after"] = add_stamp(valid)
    files["content_after"] = change_content(valid)
    certified = sign(base, signer(leaf, leaf_k), certify=True, perms=fields.MDPPerm.NO_CHANGES)
    files["docmdp_p1_violated"] = add_stamp(certified)
    files["docmdp_p1_ltv"] = add_dss(certified, certs=[to_a_cert(root)])
    form = base_pdf(form=True)
    form_cert = sign(form, signer(leaf, leaf_k), certify=True, perms=fields.MDPPerm.FILL_FORMS)
    files["form_filled"] = fill_name(form_cert, "Budi")
    files["two_signatures"] = sign(valid, signer(leaf_ec, leaf_ec_k), field="Signature2")

    rev_signed = sign(base, signer(leaf_rev, leaf_rev_k), timestamper=ts)
    files["revocation_unknown"] = rev_signed
    files["ltv_ocsp"] = add_dss(rev_signed, certs=[to_a_cert(inter)],
                                ocsps=[ocsp_resp(leaf_rev, ocsp.OCSPCertStatus.GOOD)])
    crl_signed = sign(base, signer(leaf_crl, leaf_crl_k))
    files["ltv_crl"] = add_dss(crl_signed, crls=[crl_of_inter([leaf_bad])])
    bad_signed = sign(base, signer(leaf_bad, leaf_bad_k))
    files["revoked"] = add_dss(bad_signed, ocsps=[ocsp_resp(
        leaf_bad, ocsp.OCSPCertStatus.REVOKED, dt.datetime(2026, 6, 1, tzinfo=dt.timezone.utc))])

    for k, v in files.items():
        with open(os.path.join(OUT, k + ".pdf"), "wb") as f:
            f.write(v)
        print(f"{k}.pdf  {len(v)} bytes")


if __name__ == "__main__":
    sys.exit(main())
