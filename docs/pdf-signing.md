# Signing PDFs with a certificate

Kantor makes digital signatures as Acrobat's *Use a certificate* does: a
CMS signature over the document's bytes, from your own certificate, stored
in the PDF as [PAdES](https://www.etsi.org/deliver/etsi_en/319100_319199/31914201/)
(ETSI EN 319 142-1). Picture signatures from *Fill & Sign* are something
else: they draw your signature on the page but prove nothing.

## In the app

*Sign with Certificate…* is in Fill & Sign's *Sign* menu and in the command
palette. Save your changes first: you sign the document as saved.

1. **Certificate.** Pick one from the list, or *Open Certificate File…*
   (`.p12` / `.pfx`), or *Add Token Module…* for a USB token or smart card
   whose PKCS #11 module Kantor didn't find by itself. Kantor looks for
   OpenSC, SafeNet eToken and SoftHSM modules in their usual places, the
   modules you added before, and on macOS and Windows the keychain or the
   *Personal* certificate store (Windows also reaches smart cards with a
   Windows minidriver this way). The details show who issued it, until when
   it is valid and its key.
2. **Password or PIN**, typed into the dialog. It is passed to the file
   decoder or the token once and dropped: Kantor never saves or logs it.
   Tokens with their own PIN pad ask on the reader; the system store asks
   in its own window.
3. **Appearance**: *Draw a Box* (then drag a box on the page), an empty
   signature field of the document, or *Invisible*. A visible signature
   shows "Digitally signed by …", the date, the reason and location, and
   optionally a signature from *My Signatures & Stamps* on the left.
4. **Certify** (optional): the first signature of a document can certify
   it, allowing afterwards no changes, form filling and signing, or also
   comments (DocMDP P = 1, 2, 3).
5. **Add a time stamp** asks the time-stamp server (TSA) you name for an
   RFC 3161 time stamp of the signature (PAdES B-T): proof of when it was
   signed, from a clock other than yours. **Add validation data (LTV)**
   also downloads the OCSP answers and CRLs of every certificate on the
   chain and stores them, with the certificates, in the document (B-LT), so
   the signature can be checked years later without the internet. Both go
   online, so they work only when *Let Signature Checks Use the Internet*
   is on (command palette).

*Sign…* asks where to save the signed copy (`name-signed.pdf` by default)
and opens it. The signature is added as an incremental update: the original
bytes stay as they were, so any earlier signatures remain valid.

Kantor remembers the reason, location, time-stamp server, the boxes you
ticked and the certificate files you used (their paths, never passwords) in
`settings.toml` under `[pdf]`: `signing_reason`, `signing_location`,
`signing_tsa_url`, `signing_timestamp`, `signing_ltv`, `signing_visible`,
`certificate_files`, `pkcs11_modules`.

## On the command line

```sh
kantor pdf sign contract.pdf -o contract-signed.pdf \
    --p12 me.p12 --password-file ~/.secret/p12-password \
    --page 3 --rect 380,60,180,60 --image my-signature.png \
    --reason "Approved" --location "Jakarta" \
    --tsa https://tsa.example.com/ --lt
```

| Option | |
|---|---|
| `--p12 FILE` | a PKCS #12 file; the password is the first line of `--password-file F` (`-` reads standard input) or `KANTOR_P12_PASSWORD` |
| `--pkcs11 MODULE` | a token through its PKCS #11 module; PIN from `--pin-file F` or `KANTOR_PIN` |
| `--system` | the macOS keychain or the Windows certificate store |
| `--cert TEXT` | pick the certificate whose name contains TEXT (tokens and stores) |
| `--field NAME` | sign this empty signature field |
| `--page N --rect X,Y,W,H` | a new visible signature (points from the page's bottom left) |
| `--image PNG` | a picture beside the text |
| `--reason`, `--location`, `--contact` | |
| `--certify 1\|2\|3` | certify: no changes / form filling and signing / also comments |
| `--pss` | RSASSA-PSS instead of PKCS #1 v1.5 |
| `--tsa URL` | RFC 3161 time stamp (B-T) |
| `--lt` | add validation data (B-LT) |
| `--lta` | then a document time stamp (B-LTA; needs `--tsa`) |
| `--trust FILE` | extra roots for building the chain for `--lt` |

Several input files and `-o DIR` sign them all with one password prompt
(bulk signing); a file that fails is reported and the others are still
signed (exit status 1 if any failed). Passwords are never accepted as
arguments, where other users could see them in the process list.

## What is written

- `/Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached`, `/M`, `/Name`,
  `/Reason`, `/Location`, `/ContactInfo`, `/Prop_Build`.
- A detached CMS SignedData: signed attributes content-type,
  message-digest and ESS signing-certificate-v2 (no signing-time, as PAdES
  asks); the signer's certificate and the chain the file or token holds;
  RSA PKCS #1 v1.5 or PSS (SHA-256) or ECDSA (SHA-256 on P-256, SHA-384 on
  P-384). The time stamp is an unsigned `signatureTimeStampToken`.
- B-LT: a `/DSS` with `/Certs`, `/OCSPs` and `/CRLs` (and the `ESIC`
  extension level), in its own incremental update.
- B-LTA: a `/DocTimeStamp` (`/SubFilter /ETSI.RFC3161`) after the DSS.
- Certification: `/Reference` with a DocMDP transform and `/Perms /DocMDP`.

The cryptography is ring's (already used for TLS); PKCS #12 decryption uses
the RustCrypto `aes`, `des`, `rc2` and `cbc` crates (MIT/Apache-2.0).
PKCS #12 files with PBES2 (PBKDF2 + AES or 3DES) and the older SHA-1/3DES
and RC2-40 encodings open; PBMAC1-protected files don't yet. PKCS #11
modules are loaded at run time (`kantor_keystore` crate, no C library at
build time); the macOS keychain uses Security.framework and Windows
CryptoAPI/CNG.

## Checked against

Every output kind (B-B with each key type and PSS, visible and invisible,
certified then approved, B-T, B-LT, B-LTA) is checked by Kantor's own
checker in `cargo test -p pdf_engine signing`, with a synthetic test PKI
(`scripts/pdf/gen_signing_fixtures.py`). For end-to-end checks against
pyHanko and poppler's `pdfsig`, run `scripts/pdf/signing_test_server.py`
(a local TSA, OCSP responder and CRL server for that PKI) and sign with
`--tsa http://127.0.0.1:8977/tsa --lt --trust …/root.pem`.

## Bundled roots

`crates/pdf_engine/assets/roots/` holds the national roots the operating
systems lack, each with its SHA-256 checked by a test:

| Root | SHA-256 | Checked against |
|---|---|---|
| Root CA Indonesia DS G1 (Kominfo/Komdigi) | `B5:F3:66:72:…:CF:42:FC:DF` | rootca.id repository |
| OSD LU Kelas 2 (Lembaga Sandi Negara / BSSN, BSrE) | `21:78:09:5B:3C:C0:F6:FA:CB:9E:6F:07:FE:EC:EE:63:27:A8:9C:66:81:AF:43:BA:95:12:6B:68:86:7D:02:B2` | BSrE's certificate repository (bsre.bssn.go.id/repository/sertifikat, which lists this fingerprint), the same file at the issuer address in its certificates (cvs-bsre.bssn.go.id/osdlukelas2.crt), the copy embedded in the Ministry of Finance's signed regulations, and serial 6D23A5B861C67FA8 / SHA-1 57:04:D8:81:…:95:59:48:A7 on a regional government's PDFSign page (2026-11-20) |

OSD LU Kelas 2 was valid from 2016-08-18 to 2026-08-18. BSrE now issues
from "BSRE CA DS G1", which chains to Root CA Indonesia DS G1 (already
bundled). BSrE's "Root BSRE CA SSL/TLS" is for web servers and is not
bundled.
