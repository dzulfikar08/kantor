#!/usr/bin/env bash
# Synthetic PKI for kantor_esign's mock providers (tests only, never real
# keys): a test root, an issuing CA, the remote signer of the Xignature mock
# (certificate + PKCS #8 key, which the mock uses to sign hashes) and the
# e-meterai stamping identity of the VIDA/Mekari mocks (PKCS #12,
# password kantor-test).
#
#   scripts/esign/gen_fixtures.sh [OUT_DIR]   (default crates/kantor_esign/tests/fixtures)
set -euo pipefail
out=${1:-crates/kantor_esign/tests/fixtures}
mkdir -p "$out"
work=$(mktemp -d "${TMPDIR:-/tmp}/esign-fixtures.XXXXXX")
trap 'rm -rf "$work"' EXIT
cd "$work"

cat > ext.cnf <<'CNF'
[root]
basicConstraints = critical, CA:true
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[ca]
basicConstraints = critical, CA:true, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[leaf]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature, nonRepudiation
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
CNF

key() { openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$1" 2>/dev/null; }
key root.key
key ca.key
key signer.key
key meterai.key

openssl req -x509 -new -key root.key -sha256 -days 7300 \
  -subj "/C=ID/O=Kantor Test/CN=Kantor Test Root CA (mock PSrE)" \
  -extensions root -config <(cat /etc/ssl/openssl.cnf ext.cnf 2>/dev/null || cat ext.cnf) -out root.pem
csr_sign() { # name subject ext issuer_cert issuer_key
  openssl req -new -key "$1.key" -subj "$2" -out "$1.csr"
  openssl x509 -req -in "$1.csr" -CA "$4" -CAkey "$5" -CAcreateserial -sha256 -days 7000 \
    -extfile ext.cnf -extensions "$3" -out "$1.pem" 2>/dev/null
}
csr_sign ca "/C=ID/O=Kantor Test/CN=Kantor Test Issuing CA (mock PSrE)" ca root.pem root.key
csr_sign signer "/C=ID/O=Personal/CN=Mock Signer Xignature" leaf ca.pem ca.key
csr_sign meterai "/C=ID/O=Kantor Test/CN=Mock e-Meterai Stamp" leaf ca.pem ca.key

openssl pkcs8 -topk8 -nocrypt -in signer.key -outform DER -out signer-key.der
cat ca.pem root.pem > chain.pem
openssl pkcs12 -export -inkey meterai.key -in meterai.pem -certfile chain.pem \
  -passout pass:kantor-test -name "Mock e-Meterai Stamp" -out meterai.p12

cp root.pem ca.pem signer.pem signer-key.der meterai.p12 "$OLDPWD/$out/"
echo "wrote $out: root.pem ca.pem signer.pem signer-key.der meterai.p12"
