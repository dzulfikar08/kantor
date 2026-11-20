#!/usr/bin/env python3
"""Answer the test PKI's addresses (127.0.0.1:8977) for end-to-end signing
checks: a time-stamp authority (/tsa), an OCSP responder (/ocsp) and the
certificates and CRLs. Uses the keys from gen_signing_fixtures.py; for
tests only.

    ~/.cache/tap/venv/bin/python scripts/pdf/signing_test_server.py &
    kantor pdf sign in.pdf -o out.pdf --p12 …/rsa.p12 --password-file pw \\
        --tsa http://127.0.0.1:8977/tsa --lta --trust …/root.pem
"""

import datetime as dt
import glob
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from asn1crypto import keys as a_keys, tsp, x509 as a_x509
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.serialization import pkcs12
from cryptography.x509 import ocsp
from pyhanko.sign.timestamps.dummy_client import DummyTimeStamper

DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                   "crates", "pdf_engine", "tests", "fixtures", "signing")


def read(name):
    with open(os.path.join(DIR, name), "rb") as f:
        return f.read()


CA = x509.load_der_x509_certificate(read("ca.cer"))
CA_KEY = serialization.load_pem_private_key(read("ca-key.pem"), None)
TSA = read("tsa.cer")
TSA_KEY = read("tsa-key.pem")
CERTS = {}
for p in glob.glob(os.path.join(DIR, "*.p12")):
    pw = b"" if p.endswith("empty.p12") else b"kantor-test"
    _, c, _ = pkcs12.load_key_and_certificates(open(p, "rb").read(), pw)
    CERTS[c.serial_number] = c

STAMPER = DummyTimeStamper(
    tsa_cert=a_x509.Certificate.load(TSA),
    tsa_key=a_keys.PrivateKeyInfo.load(
        serialization.load_pem_private_key(TSA_KEY, None).private_bytes(
            serialization.Encoding.DER, serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption())),
    certs_to_embed=[a_x509.Certificate.load(read("ca.cer"))],
)


class Handler(BaseHTTPRequestHandler):
    def reply(self, body, ctype):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        name = self.path.strip("/")
        if name in ("root.crl", "ca.crl", "root.cer", "ca.cer"):
            return self.reply(read(name), "application/octet-stream")
        self.send_error(404)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path == "/tsa":
            req = tsp.TimeStampReq.load(body)
            return self.reply(STAMPER.request_tsa_response(req).dump(),
                              "application/timestamp-reply")
        if self.path == "/ocsp":
            req = ocsp.load_der_ocsp_request(body)
            cert = CERTS.get(req.serial_number)
            if cert is None:
                return self.send_error(404)
            now = dt.datetime.now(dt.timezone.utc)
            resp = (ocsp.OCSPResponseBuilder()
                    .add_response(cert=cert, issuer=CA, algorithm=hashes.SHA1(),
                                  cert_status=ocsp.OCSPCertStatus.GOOD, this_update=now,
                                  next_update=now + dt.timedelta(days=7),
                                  revocation_time=None, revocation_reason=None)
                    .responder_id(ocsp.OCSPResponderEncoding.HASH, CA)
                    .sign(CA_KEY, hashes.SHA256()))
            return self.reply(resp.public_bytes(serialization.Encoding.DER),
                              "application/ocsp-response")
        self.send_error(404)

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 8977), Handler).serve_forever()
