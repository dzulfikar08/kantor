# Kantor on the Mac App Store

Kantor for macOS comes in two builds from the same source:

| | Download (`Kantor.dmg`) | Mac App Store |
|---|---|---|
| Built by | `scripts/build-dmg.sh` (release.yml) | `scripts/build-mas.sh` (mas.yml) |
| Cargo | default features | `--features app-store` |
| Signed | Developer ID, notarised | Apple Distribution; Apple re-signs it |
| Sandbox | no | App Sandbox |
| Updates | its own updater | the App Store |
| `kantor` command line, `kantor mcp` | yes | `kantor mcp` only (see below) |
| PKCS #11 token modules | yes | no: tokens through the keychain |

Bundle id `com.dzulfikar.kantor`, team `F6Q6UFUK69`, free, no in-app purchases,
no account. Both builds can't be installed side by side under the same name: the
store copy replaces `/Applications/Kantor.app`.

## What the `app-store` feature changes, and why

Code asks `kantor_core::APP_STORE` (the feature) or `kantor_core::SANDBOXED`
(the feature on macOS). Without the feature nothing below changes.

- **No self-update** (guideline 2.4.5): the update check, download and *Restart to
  Update* never run, and *Check for Updates…* / *Restart to Update* aren't in the
  palette. The palette's footer says "updates from the App Store".
- **No outside programs** (guideline 2.5.2): the store build starts no other program.
  - The updater's `tar`, `codesign`, `spctl`, `open` and installer runs are never
    reached (no update ever starts).
  - GPUI's "open with the system" ran `/usr/bin/open`; with the `mas` feature of the
    vendored `gpui-pre-macos` it calls `NSWorkspace openURL:` (KANTOR_PATCHES.md §3).
    That is what Print uses to hand its PDF to Preview.
  - `kantor mcp` doesn't start Kantor itself when it isn't running (it would run as a
    child of the agent); it tells the agent to ask the user to open Kantor.
  - PKCS #11 modules are libraries from outside the app and aren't loaded
    (`kantor_keystore::pkcs11::AVAILABLE`); *Add Token Module…* is hidden. Smart cards
    and USB tokens macOS supports show up as keychain identities (CryptoTokenKit),
    which the `smartcard` entitlement allows.
  - The Batch "plugin" steps (Sign with Certificate, e-Meterai, provider signing) are
    Rust code inside Kantor, not external programs; they stay.
  - `qpdf`, `pdftoppm`, `uv` appear only in tests.
- **Private API**: GPUI 0.3.11 names three private selectors
  (`_opaqueRectForWindowMoveWhenInTitlebar`, `_windowRestorationOptions`, `_zoomFill:`);
  the `mas` feature leaves them out (vendor/gpui-pre-macos/KANTOR_PATCHES.md §1). It
  doesn't use `CGS…`/`SLS…` functions any more. `build-mas.sh check-private` fails a
  build whose binary links `CGS`/`SLS` symbols or names a selector starting with `_`;
  both `bundle` and `package` run it, and so does CI.
- **Paths**: settings, session, bookmarks, recovery copies, cloud working copies,
  caches and the agent socket all live in the container
  (`~/Library/Containers/com.dzulfikar.kantor/Data`). `kantor_core::dirs::home_dir()`
  is the container there (worked out from the real `HOME` when `kantor` is run from
  Terminal), `XDG_*` variables are ignored. Save panels for untitled documents start
  in the user's real home (`user_home_dir`).
- **Fonts**: the interface and layout use CoreText and fontique, which find fonts
  macOS knows, user-installed ones too. The two `fontdb` users (typed signatures,
  metafile text) look in `$HOME/Library/Fonts`, which is the container's; the store
  build adds the real `~/Library/Fonts` (the sandbox lets apps read it).
- **Keychain**: AI keys and cloud tokens go to the login keychain through the legacy
  SecKeychain API (apple-native-keyring-store `keychain`), which works in the sandbox
  for the app's own items. A key saved by the download build has that build's
  signature in its access list, so the store build asks once ("Kantor wants to use…")
  before reading it. Signing identities are found with `SecItemCopyMatching`.
- **Network**: `network.client` only. Kantor never listens on a network port.
- **Printing**: Kantor makes a PDF and opens it in the system's viewer; it never runs
  `NSPrintOperation`, so there is no `print` entitlement.

### Files in the sandbox

A sandboxed app may use a file the user picked in an open or save panel, opened from
Finder (Open With, double-click, Dock) or dropped on its window, until it quits. For
everything Kantor opens again later it keeps an app-scoped **security-scoped bookmark**
(`crates/kantor_sandbox`, stored in `bookmarks.json` in the state folder, newest 256):

- **Open documents and Recent files**: every document that opens gets a bookmark; the
  last session's tabs reopen through theirs (a tab without one stays closed). Save As
  bookmarks the new file.
- **Batch**: input files and folders, the result folder, stamp pictures, value tables
  and certificate files picked in the Batch window. Before a batch runs, Kantor checks
  every input, picture, table and the result folder ("next to the originals" means
  each input's folder) and asks for what it can't use.
- **Mail merge**: the data source picked in Select Recipients; a document's linked
  data source reopens through it.
- **Certificate files** (`.p12`/`.pfx`) in Sign with Certificate.

When Kantor knows a path but has no access (a Recent file from before, a mail-merge
source linked by someone else, a saved batch, a path an agent names), it shows
**Grant File Access**, as Word for Mac does: "Kantor needs your permission to open
“name”", and *Grant Access…* opens an open panel on the file's folder with that
message. What the user picks is bookmarked.

**Saving.** Kantor saves through a temporary file and a rename. A file the user picked
may be writable while its folder isn't; then the save writes the file in place
(`kantor_core::fsx`). New files next to a document (Batch results next to the
originals) need the folder, which Kantor asks for.

Kantor doesn't load linked (external) pictures from disk, so they need nothing.

### Command line and MCP from Terminal

Running `/Applications/Kantor.app/Contents/MacOS/kantor` from Terminal runs it in the
same sandbox and container as the app, without the user's file grants:

- `kantor FILE…` opens the app; each file Kantor can't use yet asks for access.
- `kantor pdf …`, `kantor convert …`, `kantor slides …`, `kantor docs merge`,
  `kantor sheets merge` are refused with a message pointing to the download
  (they only read and write files named on the command line, which the sandbox
  doesn't allow).
- `kantor mcp` works: it talks to the running app over the socket in the container
  (`<container>/run/kantor-<pid>.sock`, short enough for the 104-byte socket path
  limit). Tools on open documents work as in the download. `open_document` and
  `save_document` with a path work for files Kantor has access to; otherwise Kantor
  shows Grant File Access to the user and the tool tells the agent to try again.
  `kantor mcp` doesn't start Kantor.
- `kantor --version`, `--help` work.

## Entitlements

`packaging/macos/Kantor.mas.entitlements`:

| Entitlement | Why |
|---|---|
| `com.apple.security.app-sandbox` | Required on the Mac App Store. |
| `com.apple.security.files.user-selected.read-write` | Open and save the documents the user chooses (panels, Finder, drag and drop). |
| `com.apple.security.files.bookmarks.app-scope` | Reopen last session's documents, Recent files, Batch folders, mail-merge data sources and certificate files in later runs. |
| `com.apple.security.network.client` | Optional online features the user turns on: AI providers with the user's own key, cloud storage (WebDAV, Nextcloud), `IMAGE`/`WEBSERVICE` spreadsheet functions, signature time stamps and revocation checks, e-signature providers. |
| `com.apple.security.smartcard` | Signing PDFs with a smart card or USB token through the keychain. |
| `com.apple.application-identifier`, `com.apple.developer.team-identifier` | Added by `build-mas.sh package` from the provisioning profile. |

Left out: `print` (unused, see above), `network.server`, `files.downloads`, JIT and
unsigned-memory exceptions, `keychain-access-groups`.

## Export compliance

`ITSAppUsesNonExemptEncryption` is `false`. Kantor's cryptography is used for HTTPS
(rustls), digital signatures and their checks (PDF signing, update manifests),
hashes, opening password-protected PDFs and Office files, and keeping an existing
PDF's protection when it is saved again. Kantor offers no way to encrypt a file that
wasn't already protected, and its primary function is editing office documents, not
information security (EAR Category 5 Part 2, Note 4, and authentication/signature
uses). If App Store Connect's questionnaire concludes otherwise, build with
`KANTOR_MAS_NON_EXEMPT_ENCRYPTION=1` and file the annual self-classification report.

## Build and upload

Needs a Mac with Xcode, `rsvg-convert` (`brew install librsvg`) and both Rust targets.

```sh
# Unsigned, sandboxed (ad-hoc) universal app + zip, for a local smoke test:
BUILD_NUMBER=$(date +%s) ./scripts/build-mas.sh bundle dist
open dist/Kantor.app        # runs sandboxed: ~/Library/Containers/com.dzulfikar.kantor

# Sign with Apple Distribution, embed the profile, make the .pkg, validate:
MAS_PROFILE=Kantor_Mac_App_Store.provisionprofile ASC_KEY_ID=… ASC_ISSUER_ID=… \
  ./scripts/build-mas.sh package dist/Kantor.app dist          # add --upload to upload
```

`package` needs in the keychain an **Apple Distribution** identity and a **3rd Party
Mac Developer Installer** (Mac Installer Distribution) identity, a **Mac App Store**
provisioning profile for `com.dzulfikar.kantor`, and the App Store Connect API key at
`~/.appstoreconnect/private_keys/AuthKey_<ASC_KEY_ID>.p8`. It reads the profile's
application and team identifiers into the entitlements (codesign doesn't take them
from the profile; App Store Connect rejects the upload with error 90886 otherwise),
checks they are `F6Q6UFUK69.com.dzulfikar.kantor`, embeds the profile, signs with the
hardened runtime and a secure timestamp, builds `Kantor-<version>.pkg` with
`productbuild` and runs `xcrun altool --validate-app` (or `--upload-app`). There is no
notarisation: Apple signs the store copy again. `CFBundleVersion` must grow with each
upload: `BUILD_NUMBER`, default the Unix time.

### CI (`.github/workflows/mas.yml`)

- **push to main / pull requests touching the store build**: builds the arm64 store
  bundle (release, unsigned), runs the private-API check, checks the CLI refusal and
  the entitlements, and uploads `Kantor-mas-<version>.zip` as an artifact.
- **manual run or `v*` tag** (environment `release`): universal bundle, signed `.pkg`,
  validated by App Store Connect. It uploads only when the manual run's `upload` input
  is ticked, or for tags when the repository variable `MAS_UPLOAD_ON_TAG` is `true`.

Secrets in the `release` environment: `APPLE_DISTRIBUTION_P12_BASE64`,
`APPLE_DISTRIBUTION_P12_PASSWORD`, `MAC_INSTALLER_P12_BASE64`,
`MAC_INSTALLER_P12_PASSWORD`, `MAS_PROFILE_BASE64` (new), and the existing
`ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_BASE64`.

## App Review notes (paste into App Store Connect)

> Kantor is a free office suite (spreadsheets, documents, presentations, PDF) with no
> account, no sign-in and no in-app purchases. All features work offline on local files.
>
> Entitlements:
> - User-selected files (read/write): opening and saving the documents the user
>   chooses in open/save panels, opens from Finder or drags onto the window.
> - App-scoped bookmarks: reopening the documents of the last session and the Recent
>   list, folders chosen for PDF batch processing, mail-merge data sources and
>   certificate files the user chose earlier.
> - Outgoing network connections: only for optional features the user turns on: AI
>   assistance with the user's own API key, cloud storage on the user's own WebDAV or
>   Nextcloud server, spreadsheet functions that fetch web data (IMAGE, WEBSERVICE;
>   asked before each workbook), and time stamps / certificate revocation checks /
>   e-signature providers when signing PDFs. Kantor accepts no incoming connections.
> - Smart cards: signing PDFs with a certificate on a smart card or USB token, through
>   the keychain.
>
> Kantor doesn't update itself in this version and runs no other programs. To try it:
> open the app, choose a template on the Home screen, or open any .xlsx, .docx, .pptx
> or .pdf file.

## License

Kantor is AGPL-3.0-or-later. Its author, Dzulfikar Ats Tsauri, holds the copyright to
all of it, so the App Store copy is distributed by the copyright holder, who isn't
bound by the AGPL's terms for their own code; the source stays public under the AGPL.
This holds only while every line is the author's: outside contributions would need a
contributor licence agreement (or copyright assignment) that allows distribution on
the App Store, before they are merged. Third-party crates keep their own permissive
licences (`THIRD_PARTY_LICENSES.txt` is in the app bundle).

## Checking on a real Mac (after CI builds it)

1. Unzip the CI artifact, `open Kantor.app`: it starts; `codesign -d --entitlements -`
   shows the sandbox; Activity Monitor's "Sandbox" column says Yes.
2. Open a file with ⌘O in `~/Documents`, edit, Save: the file changes (also check a
   folder where only the file was granted: Save in place works, no `.kantor-tmp` left).
3. Quit and reopen: the tab comes back. Click a Recent file from another folder that
   was opened before this build: Grant File Access appears and the panel starts in its
   folder.
4. Open With / double-click in Finder, drag a file on the window, a picture into Docs.
5. Batch: add a folder, run "next to the originals"; reopen Kantor, load the saved
   batch, run: it asks only for what it lacks.
6. Mail merge with a linked CSV in another folder, reopen the document.
7. Print (Preview opens the PDF); Export PDF.
8. AI settings: save a key, restart, the key is there (keychain in the sandbox).
9. Cloud: sign in to a Nextcloud, open and save a file.
10. Sign a PDF with a keychain identity (and a smart card if one is at hand); with a
    `.p12` picked in the dialog, then again after a restart.
11. Fonts: a font installed only in `~/Library/Fonts` appears in the font list, in a
    typed signature and in the PDF of a document that uses it.
12. Terminal: `…/Contents/MacOS/kantor convert a b` is refused; `kantor mcp` with an
    agent while the app runs: list documents, edit a cell, open a file by path (Grant
    File Access shows), save. Check the socket appears in
    `~/Library/Containers/com.dzulfikar.kantor/Data/run/`.
13. Window ▸ Zoom / double-click the title bar, full screen, window dragging by the
    title bar (the private-selector patch).
14. `./scripts/build-mas.sh package … ` validates with App Store Connect.
