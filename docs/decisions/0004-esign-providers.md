# 0004: e-meterai and certified signatures through licensed providers' public APIs

- **Status:** accepted (2026-11-20). Built and tested against mock servers; no live provider account yet.
- **Decides:** which Indonesian e-meterai distributors and PSrE (licensed certification providers) Kantor talks to, and how (`crates/kantor_esign`).

## Context

Kantor's PDF app signs with the user's own certificate and keeps e-meterai signatures valid, but two things need a licensed partner:

- **e-meterai**: Peruri's electronic duty stamp (Bea Meterai Rp 10.000, UU 10/2020). Peruri sells it only through distributors and channels (VIDA, Mekari, Pajakku, Mitracomm, Finnet, Privy, OnlinePajak, …). The stamp itself is a PAdES signature by Peruri's e-meterai certificate, drawn as the meterai picture with its serial number and QR code.
- **certified electronic signatures** (*tanda tangan elektronik tersertifikasi*): the signer's certificate comes from a PSrE licensed by Komdigi (formerly Kominfo) and chains to Root CA Indonesia, after e-KYC of the signer.

The user has no provider account yet. The integration had to be built from public documentation only (no sign-up, no credentials, no contact), so that it works the day credentials exist.

## What is public (research of 2026-11-20)

Labels: **public-official** (published by the provider, readable without login), **public-unofficial** (readable without login, from a third party), **gated** (marketing only, or NDA / partner login).

| Provider | Role | API docs | Auth | Sandbox | Hash signing |
|---|---|---|---|---|---|
| **VIDA** ([docs.vida.id](https://docs.vida.id/identity-stack/sign/e-meterai-stamp/api-integration/e-meterai-api/api-reference)) | PSrE, e-meterai channel | public-official: paths, headers, JSON, doc-type codes, response codes | OAuth2 client credentials (Keycloak) + `X-PARTNER-ID` | yes | `docType=hash` listed on Sign Inline, shape not documented; the "HashSign API" page is empty |
| **Mekari Sign** ([Postman collection](https://documenter.getpostman.com/view/21582074/2s93K1oecc), [HMAC](https://developers.mekari.com/docs/kb/hmac-authentication)) | e-meterai channel, reseller of a PSrE (Tilaka) | public-official: full collection with examples | HMAC-SHA256 (`Date` + `Authorization: hmac …`), or OAuth2 by arrangement | yes, HMAC credentials self-serve in the sandbox Developer Center | no (document upload, envelope flows) |
| **Xignature** ([support.xignature.co.id](https://support.xignature.co.id/xignature-api-documentation/introduction), OpenAPI spec on GitHub) | PSrE | public-official | `api-key` header, per-signer `pat` | staging `api.xignature.dev` (trial after NDA or purchase) | **yes**: `POST /sign/hash` returns a raw RSA signature, `GET /certificate` the chain |
| Peruri e-Meterai (e-meterai.co.id) | issuer | not public: access needs a contract with Peruri or a distributor | – | – | n/a |
| Peruri Sign | PSrE | gated (its API Suite site is gone; a test gateway form only) | – | – | "HASH" file type in the test form, undocumented |
| Tilaka | PSrE | gated (sandbox collections only leaked in a third-party repository) | OAuth2 | `sb-api.tilaka.id` | advertised, undocumented |
| Privy | PSrE, e-meterai channel | public docs cover identity checks only; signing and e-meterai APIs unpublished | – | – | – |
| BSrE (BSSN) | PSrE for government | per-instance, government only | Basic | – | no |
| Digisign | PSrE | gated (docs site down, nothing public) | – | – | – |
| Pajakku, Mitracomm, Finnet, OnlinePajak, Sertisign | e-meterai distributors | gated (NDA before sandbox) | – | via NDA | n/a |

Prices: no provider publishes API prices. Retail e-meterai is the Rp 10.000 duty plus a Rp 2.000–3.500 service fee (beli-meterai.id, Sertisign, meterai-elektronik.com); Xignature's personal-plan certificate is Rp 11.000.

No public API returns a finished CMS for a hash. Xignature returns the raw signature plus the certificate chain, which is enough: Kantor already builds PAdES signatures itself (Sign with Certificate).

**Stamp size.** No official rule was found. Public integrations use a square of about 1.6–2.8 cm (Mekari's default 80 pt); VIDA fixes the size itself and takes only the lower-left corner. Kantor proposes 70 pt (≈ 2.5 cm) and accepts 45–85 pt (1.6–3 cm).

## Decision

A provider-neutral crate, `kantor_esign` (no GPUI):

- `EMeteraiProvider` (`doc_types`, `balance`, `test`, `stamp`) and `RemoteSigner` (`test`, `request_otp`, and one of three flows: `identity` = the provider signs raw hashes and Kantor builds the PAdES signature; `hash_signer` = the provider returns a CMS for a digest, plugged into `pdf_engine::signing::sign_external`; `sign_document` = document upload).
- Concrete providers only where the public documentation is enough to implement faithfully:
  - **VIDA e-Meterai API** (token → quota / doctype → upload `{codeDocument, page, llx, lly, refNum, document}` → poll `docstamp/{refNum}`, code 31 = pending, 25 = retry status up to three times).
  - **Mekari Sign** e-meterai (`POST /documents/stamp` with a `meterai` annotation → poll `GET /documents/{id}` for `stamping_status` → `GET /documents/{id}/download`), HMAC-signed requests. Polling instead of the callback: a desktop app has no public address.
  - **Xignature** hash signing (`generate_pat` → `certificate` → `sign/hash`): the document never leaves the computer.
  - Every other provider above is a documented stub that says why it can't be used yet.
- HTTP through the same blocking ureq + rustls stack as kantor_ai and kantor_cloud. Credentials in the OS keychain (kantor_ai's `ESIGN` vault; 0600 fallback file without a keychain). Settings `[[pdf.esign_providers]]` with `allow_network` per provider: off, nothing is sent even with credentials saved.
- Every result is checked before it's saved: it must be a PDF, and if the input was signed, either the input is an exact prefix of the result (incremental update) or Kantor's checker confirms the earlier signatures are still intact; otherwise `BreaksSignatures`, unless the user agreed after the warning. Providers known to rebuild files (`rewrites_file`) get the warning before anything is sent.

### Assumptions to confirm with a sandbox account

- VIDA: the stamp gateway's production address and the production token address aren't in the docs (the sandbox gateway `sandbox-stamp-gateway.np.vida.id` comes from a community client). Kantor asks for them when production is chosen. `llx`/`lly` are taken as PDF points from the page's bottom left.
- Mekari: positions are taken as measured from the page's top left in the declared canvas, which Kantor sets to the page's size in points as displayed (`/Rotate` applied). The document type isn't a request field; it comes from the Mekari account. A single download is taken to return the PDF (a JSON link is followed too).
- Xignature: `hash` is base64 of the hex SHA-512 digest of the CMS signed attributes, as its example (128 hex digits); the padding isn't stated, so each signature is verified against the signer's certificate as RSA PKCS #1 v1.5 with SHA-512 before use. No OTP step is documented.

## Testing

`kantor_esign::mock` runs a local HTTP server per provider, written from the documented request and response shapes (status codes, error bodies, pending answers, HMAC check, PEM without line breaks as in Xignature's example). The e-meterai mocks stamp with a synthetic "Peruri" certificate (a real PAdES signature as an incremental update); the Xignature mock signs digests with a synthetic key (`scripts/esign/gen_fixtures.sh` makes the test PKI in `crates/kantor_esign/tests/fixtures`). Results are verified with Kantor's checker in the tests, and the signing path was checked with pyHanko 0.38 (`INTACT:TRUSTED` for the Xignature signature, and for both signatures of a signed-then-stamped file) and poppler's pdfsig ("Signature is Valid"). `KANTOR_ESIGN_OUT=DIR cargo test -p kantor_esign` keeps those files.

## Consequences

- When an account exists: VIDA, Mekari Sign or Xignature should work after entering credentials, apart from the assumptions above; each is one small module to adjust.
- Providers that only open their docs after an NDA (Peruri direct, Pajakku, Privy, Tilaka…) need that agreement first; their stubs are where the code goes.
- Certified signing per OTP can't run unattended in a Batch; Xignature documents none.
- A partner enquiry draft for a distributor and a PSrE was written outside the repository for the user.
