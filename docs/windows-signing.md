# Signing Kantor for Windows (SignPath Foundation applied 2026-11-13)

Applied to the SignPath Foundation on 2026-11-13 (answer pending; replies go to dzulfikar.at@gmail.com). Nothing is bought or configured yet. Today the Windows installer and zip are **unsigned**, and
the workflows only sign when the secrets below exist.

## What unsigned means for users

- Edge and Chrome may flag `Kantor-Setup-x64.exe` as "not commonly downloaded". The user has to
  choose **Keep**.
- When the installer starts, Microsoft Defender SmartScreen shows "Windows protected your PC".
  The user has to click **More info**, then **Run anyway**. With Smart App Control on (some
  fresh Windows 11 installs), unsigned apps can be blocked outright.
- Managed company PCs often block unsigned installers completely.
- The in-app updater doesn't depend on Authenticode. Each update is checked against the
  SHA-256 in the Ed25519-signed manifest, the same as on Linux. Installers it downloads
  itself don't carry the "downloaded from the internet" mark, so they run silently without
  SmartScreen.

Signing doesn't make the warning go away on day one. SmartScreen reputation builds per
signing identity over downloads and time. Since 2024 that is true for EV certificates too:
they no longer skip the warning. Signing does mean the warning names a verified publisher,
reputation carries over from one release to the next (unsigned files start from zero on
every release), and enterprise and Smart App Control policies accept the files.

## Options

| Option | Cost (Nov 2026) | Can Kantor's owner use it? | Notes |
|---|---|---|---|
| **SignPath Foundation** (free signing for open-source projects) | Free | Probably: Kantor is AGPL on GitHub; apply at signpath.org | OV-level certificate in the Foundation's name, signing through SignPath's GitHub Action from release.yml. Needs a review and reproducible CI builds from a public repo (both true here). The publisher shown is "SignPath Foundation", not you. |
| **Azure Artifact Signing** (formerly Trusted Signing) | $9.99/month (5,000 signatures) | **Not today**: individuals only in the USA and Canada, organisations in the USA, Canada, EU and UK | Cheapest for those who qualify. No hardware token, and there is a GitHub Action (`azure/artifact-signing-action`). Billing starts when the account is created. |
| **OV code-signing certificate** (Certum, Sectigo, SSL.com, DigiCert, GlobalSign…) | ~$150–300/year; Certum's open-source certificate for individuals costs less | Yes, from Indonesia, as an individual or a company | Since June 2023 the key must live on a hardware token or a cloud HSM, so a plain `.pfx` file is no longer issued. In CI that means the CA's cloud signing tool (SSL.com eSigner, Certum SimplySign, DigiCert KeyLocker…) instead of `signtool /f cert.pfx`. |
| **EV certificate** | $400+/year | Yes | No SmartScreen advantage over OV since 2024. Not worth it for this. |
| **Microsoft Store (MSIX)** | Free (one-time Store developer registration) | Yes | The Store re-signs the package, and users see no SmartScreen warning. Updates come from the Store, and the in-app updater must stay off. Needs an MSIX build and Store review, and AGPL terms vs the Store terms need a look (the same question as the Mac App Store). |

**Recommendation:** apply to SignPath Foundation first (free, and fits an AGPL GitHub project).
If you want the publisher to read "Dzulfikar Ats Tsauri" or a company name, buy an OV
certificate with cloud signing. Consider the Microsoft Store later as a second channel.

## How the workflows sign (when you decide)

`scripts/package-windows.sh` signs when **`WINDOWS_CERT_PFX_BASE64`** (base64 of a `.pfx`) and
**`WINDOWS_CERT_PASSWORD`** are set as secrets of the `release` environment. It signs
`kantor.exe`, `kantor.com`, the uninstaller and the installer with `signtool` (SHA-256, RFC 3161
timestamp from `WINDOWS_TIMESTAMP_URL`, DigiCert's by default). Without the secrets it prints a
notice and builds everything unsigned. That path fits a certificate whose key you hold as a
file, such as a self-managed test certificate. The signing branch has never run, because no
certificate exists yet.

Each of the options above signs differently, so the script needs a small change for whichever
one you choose:

- SignPath: upload the unsigned installer and zip as an artifact, and call
  `signpath/github-action-submit-signing-request` in release.yml between building and the
  smoke test.
- Azure Artifact Signing: `azure/artifact-signing-action` with the account, profile and
  federated credentials.
- OV with cloud HSM: the CA's CLI (for example `CodeSignTool sign …` for SSL.com eSigner) in
  place of `signtool /f`.
