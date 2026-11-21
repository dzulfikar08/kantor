# e-Meterai and certified signatures with your provider account

Kantor stamps Indonesian e-meterai and makes certified electronic signatures through the account you have with a licensed provider. Kantor has no service of its own in between: documents (or, for hash signing, only the document's hash) go straight from your computer to the provider, and only when you stamp or sign.

| Provider | What | How Kantor uses it |
|---|---|---|
| VIDA | e-Meterai | VIDA's e-Meterai API: the document is uploaded, VIDA (through Peruri) stamps it, Kantor downloads the result |
| Mekari Sign | e-Meterai | Mekari eSign Open API (HMAC credentials) |
| Xignature | certified signatures | Xignature Open API: Kantor builds the signature and sends only the hash; the document never leaves your computer |

Peruri, Pajakku, Mitracomm, Finnet, Privy, Tilaka, Peruri Sign, VIDA Sign and Digisign are listed too, with the reason Kantor can't use them yet (their API documentation isn't public). See [decisions/0004-esign-providers.md](decisions/0004-esign-providers.md).

## Set up a provider

In a PDF: Fill & Sign ▸ *Sign* ▸ *e-Sign Providers…* (or the command palette).

1. Pick the provider and type the credentials it gave you:
   - VIDA: *Client ID*, *Client secret*; for production also the *token address* and the stamp gateway's *Server address* VIDA gives you.
   - Mekari Sign: the *HMAC client ID* and *secret* from Mekari's Developer Center (the sandbox one is self-serve).
   - Xignature: the *API key*.
2. Tick *Allow Kantor to send documents to …*. Without it nothing is sent, even with credentials saved.
3. Leave *Use the provider's sandbox* on while testing; turn it off for real stamps (sandbox stamps aren't legally valid).
4. *Save and Test* checks the credentials with the provider.

Credentials are kept in the system keychain (or, without one, in `esign-keys.toml` next to the settings, readable only by you), never in `settings.toml`.

From the command line:

```sh
kantor pdf providers                                  # list, with each provider's credential fields
kantor pdf providers set vida --credentials-file vida.json --allow-network
#   vida.json: {"client_id": "…", "client_secret": "…"}   (or key=value lines)
kantor pdf providers set vida --production --server https://… # VIDA's production gateway
kantor pdf providers test vida
kantor pdf providers doc-types vida
kantor pdf providers remove vida
```

## Add an e-meterai

*Sign* ▸ *Add e-Meterai…*: choose the provider and the document type (*jenis dokumen*; VIDA lists Peruri's types, Mekari takes it from your account), optionally the document number, then *Place on Page* and click where the meterai goes (or drag a square of 1.6–3 cm). *Stamp…* asks where to save the stamped copy, sends the document and opens the result. Each stamp uses one e-meterai from your quota.

```sh
kantor pdf emeterai kontrak.pdf --provider vida --doc-type VD010 --page last --position bottom-right
kantor pdf emeterai *.pdf -o stamped/ --provider mekari --doc-type account --rect 400,80,70,70
```

## Sign with a certified signature

*Sign* ▸ *Sign with Provider…*: your account at the provider (for Xignature, your Xignature ID), where the signature shows (a box you draw, an empty signature field, or invisible), reason and location. *Send Code* asks the provider for a one-time password when its flow uses one.

```sh
kantor pdf sign kontrak-emeterai.pdf --provider xignature --signer MY-XIGNATURE-ID --page 1 --rect 72,72,180,60
```

## Password-protected PDFs

*Sign with Provider…* signs a password-protected PDF with the password it was opened with, and the signed copy keeps that password and protection (RC4, AES-128 or AES-256): with hash signing (Xignature) only the document's hash goes to the provider. The same rules as *Sign with Certificate…* apply: the owner password always works; the open (user) password only when the file's owner allowed commenting or filling in forms. Providers that take the whole document can't sign a protected file; Kantor says so before sending anything. On the command line: `kantor pdf sign --provider … --password PW` (or `KANTOR_PDF_PASSWORD`, or `--pdf-password-file F`).

An e-meterai provider needs the whole document, so a protected file can't be stamped as it is. *Add e-Meterai…* offers *Stamp an unprotected copy*: Kantor removes the password from a copy with the owner (permissions) password (asked in the dialog if the file was opened with the open password), checks it, and only then sends it; the original file isn't changed. The stamped copy has no password, and it can't be given one again: a PDF's encryption covers every string and stream in the file, so protecting it would rewrite the bytes the e-meterai's signature covers (an incremental update can't encrypt the revisions before it). A file that is both signed and protected is refused, because removing its password would break its signatures. Every refusal happens before anything is sent, so no e-meterai is spent on it.

```sh
kantor pdf emeterai locked.pdf --provider vida --doc-type VD010 --unprotected-copy --password OWNER-PASSWORD
```

## Signatures already in the document

A stamp or signature is added as an incremental update, so earlier signatures stay valid. Kantor checks every result before saving it: if the provider rebuilt the file and an earlier signature no longer verifies, nothing is saved, unless you ticked *Stamp anyway* / *Sign anyway* after the warning (or passed `--allow-breaking-signatures`).

## Batch

*Tools ▸ Batch…* ▸ *Add step*: *Add e-Meterai* (bottom right of the first or last page of every file) and *Sign with Provider* (invisible, always the last step). In batch files:

```json
{"type": "emeterai", "provider": "vida", "doc_type": "VD010", "page": "last",
 "place": {"anchor": "bottom-right", "dx": 36, "dy": 36}, "size": 70}
{"type": "provider-sign", "provider": "xignature", "signer": "MY-XIGNATURE-ID",
 "appearance": {"kind": "invisible"}, "reason": "Approved"}
```

Password-protected files use the batch's password (one for all, or each file's from a `Password` column). *Sign with Provider* signs them with it and they stay protected (with the permission rules above); *Add e-Meterai* skips them unless its *Stamp unprotected copies of password-protected files* box is ticked (`"unprotected_copy": true`) and the password is the owner password. Each skipped file says why in the results, and stamped copies say they have no password.

`kantor pdf batch steps.json folder/` runs them with the saved provider setup (`--allow-network` allows sending for that run).

## Not tried on live services yet

The integrations were built from the providers' public documentation and tested against local mock servers of those APIs, not yet with a real account. Details the documentation leaves open (VIDA's production addresses, Mekari's coordinate origin, Xignature's hash encoding) follow the providers' examples; see the decision record.
