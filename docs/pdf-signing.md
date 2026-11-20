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

## Password-protected PDFs

A password-protected PDF is signed as Acrobat signs it: with the password
it was opened with, and it stays protected with the same passwords. The
signature, and for B-LT/B-LTA the DSS and the document time stamp, are
incremental updates encrypted with the file's own security handler (RC4
40/128-bit, AES-128 or AES-256, `/V` 1, 2, 4 or 5) and file key, pointing at
the file's original `/Encrypt` dictionary and keeping its `/ID`. Every new
string and stream is encrypted with its object's key, except the signature
dictionary's `/Contents` (ISO 32000-2 §7.6.2: it is never encrypted); the
byte range and its digest cover the encrypted bytes as written. The owner's
restrictions count, as in Acrobat's *Document Restrictions Summary*: with
the open (user) password, signing needs the owner to allow commenting
(`/P` bit 6) or filling in form fields (bit 9, security handlers of revision
3 and later); otherwise Kantor says so and asks for the owner password,
which always signs. Files protected with a certificate (`Adobe.PubSec`)
rather than a password aren't supported, nor are files with an ISO/TS 32004
integrity code (`/KDFSalt`), which Kantor can't extend.

In the app the password is the one you typed when opening the file, and the
signed copy opens without asking again. On the command line, give it with
`--password PW`, `--pdf-password-file F` or `KANTOR_PDF_PASSWORD`. A batch
takes one password for all its protected files (the dialog's *PDF password*,
or `--password`), or each file's own from a `Password` column (also `PDF
password`, `Kata sandi`) in the fill step's table or in a table given with
`kantor pdf batch --passwords TABLE`; passwords are used for that run only
and never saved. `kantor pdf verify --password PW` and the Signatures panel
check protected files too.

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
| `--password PW`, `--pdf-password-file F` | a password-protected PDF's password (or `KANTOR_PDF_PASSWORD`) |

Several input files and `-o DIR` sign them all with one password prompt
(bulk signing); a file that fails is reported and the others are still
signed (exit status 1 if any failed). Passwords are never accepted as
arguments, where other users could see them in the process list.

## In a batch

*Tools ▸ Batch…* has a **Sign with Certificate** step (*Add Step ▸ Sign
with Certificate*) with the same choices as the dialog: the certificate (a
`.p12`/`.pfx` file, a token, or the system store), a time stamp (B-T), also
validation data (B-LT), certify or not, and where the signature shows: a
box on the first, the last or a given page at one of nine places (as the
stamp step places pictures, upright on turned pages), an empty signature
field by name, or invisible; reason, location and a signature picture
(PNG or JPEG from *My Signatures & Stamps*, or a file).

- It is always the **last** step, and a batch has at most one: the
  signature covers everything the batch did. The dialog keeps it last; a
  batch file with steps after it is refused.
- When the batch runs, Kantor asks **once** for the certificate's password
  or the token's PIN, unlocks the key and signs every file with it. The
  password is used for that run only: it is never written to the batch
  file, the settings or a log, and a batch file that holds a `password` or
  `pin` is refused.
- Each file gets its result: signed, or not signed and why. A file is not
  signed when its owner's restrictions don't allow signing with the
  password the batch has for it, when its certification allows no
  changes, when the step certifies but the file is already signed, when a
  new visible box would change a certified file's pages (sign an empty
  field or invisibly instead), when the named field is missing or already
  signed, or when the page isn't in the file. Earlier signatures stay
  valid: everything is added as incremental updates.

`kantor pdf batch steps.json FILES -o DIR` runs the same step; the
password comes from `--password-file F` (`-` for standard input) or
`KANTOR_P12_PASSWORD`, a token PIN from `--pin-file F` or `KANTOR_PIN`, and
`--trust FILE` adds roots for B-LT. In the batch file:

```json
{"type": "sign",
 "certificate": {"file": "certs/me.p12"},
 "level": "B-T",
 "tsa": "https://tsa.example.com/",
 "certify": 2,
 "appearance": {"kind": "box", "page": "last",
                "place": {"anchor": "bottom-right", "dx": 36, "dy": 36},
                "width": 180, "height": 60},
 "reason": "Approved", "location": "Jakarta",
 "image": "my-stamps/signature.sig.png"}
```

| Key | |
|---|---|
| `certificate` | `{"file": PATH}` (relative to the batch file), `{"token": {"module": PATH, "name": TEXT}}` or `{"system": {"name": TEXT}}`; `name` picks the certificate whose name contains it |
| `level` | `B-B` (default), `B-T`, `B-LT`, `B-LTA`; all but B-B need `tsa` |
| `certify` | 1, 2 or 3 (left out: an approval signature) |
| `appearance` | `{"kind": "invisible"}` (default), `{"kind": "field", "name": NAME}`, or `{"kind": "box", "page": "first"\|"last"\|N, "place": …, "width": 180, "height": 60}`; `place` as in the stamp step (an `anchor` with `dx`/`dy`, an `x`/`y` point, or an `example`) |
| `reason`, `location`, `contact`, `image`, `pss` | as for `kantor pdf sign` |

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

Password-protected files: `cargo test -p kantor_pdf sign` and `-p pdf_engine
protected` sign each security handler (RC4 40/128, AES-128, AES-256) up to
B-LTA, with the user and the owner password, and check the restrictions.
End to end (2026-11-20), qpdf- and pyHanko-encrypted copies of a synthetic
PDF (each handler, an empty user password, object streams, restricted
permissions, one signed by pyHanko first), signed by `kantor pdf sign`
B-B/B-T/B-LTA, were checked with pyHanko 0.38 (decrypted with the user and
owner passwords: intact, valid and trusted, LTV hard-fail from the DSS),
`qpdf --check --password`, poppler's `pdfsig -upw/-opw` and `pdftoppm`
(opens only with the password).

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
