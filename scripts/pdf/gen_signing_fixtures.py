#!/usr/bin/env python3
"""Make the test PKI for Kantor's PDF *signing* tests (pdf_engine::signing).

Everything is synthetic and made fresh on each run: a root CA, an issuing
CA, signers (RSA 2048, ECDSA P-256 and P-384) as PKCS #12 files in several
encodings, a time-stamp authority, an OCSP response and CRLs. Revocation
and TSA addresses point at 127.0.0.1:8977, where
scripts/pdf/signing_test_server.py answers them for end-to-end checks
(kantor pdf sign --tsa … --lt, then pyHanko and pdfsig); the Rust tests
answer the same URLs from memory, so they never touch the network.

    ~/.cache/tap/venv/bin/python scripts/pdf/gen_signing_fixtures.py

writes crates/pdf_engine/tests/fixtures/signing/. The PKCS #12 password is
"kantor-test" (empty for empty.p12). These keys protect nothing: never use
them for anything but tests.
"""

import datetime as dt
import os
import subprocess
import tempfile

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa
from cryptography.hazmat.primitives.serialization import pkcs12
from cryptography.x509 import ocsp
from cryptography.x509.oid import AuthorityInformationAccessOID, ExtendedKeyUsageOID, NameOID

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                   "crates", "pdf_engine", "tests", "fixtures", "signing")
BASE = "http://127.0.0.1:8977"
NOT_BEFORE = dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc)
NOT_AFTER = dt.datetime(2046, 1, 1, tzinfo=dt.timezone.utc)
PASSWORD = b"kantor-test"


def name(cn):
    return x509.Name([
        x509.NameAttribute(NameOID.COUNTRY_NAME, "ID"),
        x509.NameAttribute(NameOID.ORGANIZATION_NAME, "Kantor Signing Test PKI"),
        x509.NameAttribute(NameOID.COMMON_NAME, cn),
    ])


def cert(subject, key, issuer=None, *, ca=False, eku=None, ocsp_url=None, crl_url=None,
         ca_issuer_url=None):
    iss_name, iss_key = issuer or (subject, key)
    b = (x509.CertificateBuilder().subject_name(subject).issuer_name(iss_name)
         .public_key(key.public_key()).serial_number(x509.random_serial_number())
         .not_valid_before(NOT_BEFORE).not_valid_after(NOT_AFTER)
         .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True)
         .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), False)
         .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(iss_key.public_key()),
                        False))
    ku = dict(digital_signature=not ca, content_commitment=not ca and eku is None,
              key_encipherment=False, data_encipherment=False, key_agreement=False,
              key_cert_sign=ca, crl_sign=ca, encipher_only=False, decipher_only=False)
    b = b.add_extension(x509.KeyUsage(**ku), critical=True)
    if eku:
        b = b.add_extension(x509.ExtendedKeyUsage(eku), critical=True)
    aia = []
    if ocsp_url:
        aia.append(x509.AccessDescription(AuthorityInformationAccessOID.OCSP,
                                          x509.UniformResourceIdentifier(ocsp_url)))
    if ca_issuer_url:
        aia.append(x509.AccessDescription(AuthorityInformationAccessOID.CA_ISSUERS,
                                          x509.UniformResourceIdentifier(ca_issuer_url)))
    if aia:
        b = b.add_extension(x509.AuthorityInformationAccess(aia), critical=False)
    if crl_url:
        b = b.add_extension(x509.CRLDistributionPoints([
            x509.DistributionPoint([x509.UniformResourceIdentifier(crl_url)], None, None, None)]),
            critical=False)
    return b.sign(iss_key, hashes.SHA256())


def pem(c):
    return c.public_bytes(serialization.Encoding.PEM)


def der(c):
    return c.public_bytes(serialization.Encoding.DER)


def write(name_, data):
    with open(os.path.join(OUT, name_), "wb") as f:
        f.write(data)


def crl(issuer_cert, issuer_key):
    now = NOT_BEFORE
    return (x509.CertificateRevocationListBuilder().issuer_name(issuer_cert.subject)
            .last_update(now).next_update(NOT_AFTER)
            .add_extension(x509.CRLNumber(1), False)
            .sign(issuer_key, hashes.SHA256()))


def main():
    os.makedirs(OUT, exist_ok=True)
    root_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    root = cert(name("Kantor Signing Test Root"), root_key, ca=True)
    ca_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    ca = cert(name("Kantor Signing Test CA"), ca_key, (root.subject, root_key), ca=True,
              crl_url=f"{BASE}/root.crl", ca_issuer_url=f"{BASE}/root.cer")
    issuer = (ca.subject, ca_key)
    leaf = dict(ocsp_url=f"{BASE}/ocsp", crl_url=f"{BASE}/ca.crl",
                ca_issuer_url=f"{BASE}/ca.cer")

    keys = {
        "rsa": rsa.generate_private_key(public_exponent=65537, key_size=2048),
        "p256": ec.generate_private_key(ec.SECP256R1()),
        "p384": ec.generate_private_key(ec.SECP384R1()),
    }
    names = {"rsa": "Budi Santoso", "p256": "Siti Rahayu", "p384": "Agus Wijaya"}
    signers = {k: cert(name(names[k]), v, issuer, **leaf) for k, v in keys.items()}
    tsa_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    tsa = cert(name("Kantor Test TSA"), tsa_key, issuer, eku=[ExtendedKeyUsageOID.TIME_STAMPING],
               crl_url=f"{BASE}/ca.crl", ca_issuer_url=f"{BASE}/ca.cer")

    write("root.pem", pem(root))
    write("root.cer", der(root))
    write("ca.cer", der(ca))
    write("root.crl", crl(root, root_key).public_bytes(serialization.Encoding.DER))
    write("ca.crl", crl(ca, ca_key).public_bytes(serialization.Encoding.DER))

    aes = (serialization.PrivateFormat.PKCS12.encryption_builder()
           .kdf_rounds(2048).key_cert_algorithm(pkcs12.PBES.PBESv2SHA256AndAES256CBC)
           .hmac_hash(hashes.SHA256()).build(PASSWORD))
    tdes = (serialization.PrivateFormat.PKCS12.encryption_builder()
            .kdf_rounds(2048).key_cert_algorithm(pkcs12.PBES.PBESv1SHA1And3KeyTripleDESCBC)
            .hmac_hash(hashes.SHA1()).build(PASSWORD))
    for k, c in signers.items():
        write(f"{k}.p12", pkcs12.serialize_key_and_certificates(
            names[k].encode(), keys[k], c, [ca, root], aes))
    write("rsa-3des.p12", pkcs12.serialize_key_and_certificates(
        b"Budi Santoso", keys["rsa"], signers["rsa"], [ca], tdes))
    write("tsa.p12", pkcs12.serialize_key_and_certificates(
        b"Kantor Test TSA", tsa_key, tsa, [ca, root], aes))
    write("tsa-key.pem", tsa_key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption()))
    write("tsa.cer", der(tsa))
    write("ca-key.pem", ca_key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption()))
    # No password at all (OpenSSL's "absent password" MAC).
    write("empty.p12", pkcs12.serialize_key_and_certificates(
        b"Siti Rahayu", keys["p256"], signers["p256"], [ca],
        serialization.NoEncryption()))

    # OpenSSL's legacy layout: RC2-40 certificates, 3DES key, SHA-1 MAC.
    with tempfile.TemporaryDirectory() as d:
        kp, cp, chp = (os.path.join(d, n) for n in ("k.pem", "c.pem", "ch.pem"))
        with open(kp, "wb") as f:
            f.write(keys["rsa"].private_bytes(serialization.Encoding.PEM,
                                              serialization.PrivateFormat.PKCS8,
                                              serialization.NoEncryption()))
        with open(cp, "wb") as f:
            f.write(pem(signers["rsa"]))
        with open(chp, "wb") as f:
            f.write(pem(ca))
        out = subprocess.run(
            ["openssl", "pkcs12", "-export", "-legacy", "-inkey", kp, "-in", cp,
             "-certfile", chp, "-name", "Budi Santoso", "-passout", "pass:kantor-test"],
            check=True, capture_output=True).stdout
        write("rsa-rc2.p12", out)

    # A "good" OCSP answer for the RSA signer, valid for twenty years.
    for k in ("rsa",):
        b = (ocsp.OCSPResponseBuilder()
             .add_response(cert=signers[k], issuer=ca, algorithm=hashes.SHA1(),
                           cert_status=ocsp.OCSPCertStatus.GOOD, this_update=NOT_BEFORE,
                           next_update=NOT_AFTER, revocation_time=None, revocation_reason=None)
             .responder_id(ocsp.OCSPResponderEncoding.HASH, ca))
        write(f"ocsp-{k}.der", b.sign(ca_key, hashes.SHA256()).public_bytes(
            serialization.Encoding.DER))
    print("wrote", OUT)


if __name__ == "__main__":
    main()
