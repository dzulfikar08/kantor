# OneDrive and Google Drive: what the owner registers

Kantor opens and saves files on Microsoft OneDrive (personal, work and school accounts, through Microsoft Graph) and Google Drive (Drive API v3), next to WebDAV drives (Nextcloud, OpenCloud). Users sign in in their own browser (OAuth 2.0 authorization code with PKCE, the redirect comes back to a one-shot listener on `127.0.0.1`/`localhost`, RFC 8252). The refresh token is kept in the system keychain like WebDAV app passwords and single sign-on (`kantor_ai::keys::CLOUD`, or `cloud-keys.toml` when there is no keychain).

Kantor needs one app registration with each provider. Until a client id is set, that provider doesn't appear in **Cloud Accounts**.

| Build variable | What it is | Secret? |
|---|---|---|
| `KANTOR_MS_CLIENT_ID` | Application (client) ID of the Microsoft Entra app registration | No |
| `KANTOR_MS_TENANT` | Optional. `common` (default: personal and work/school accounts), `organizations`, `consumers`, or one tenant id | No |
| `KANTOR_GOOGLE_CLIENT_ID` | Client ID of the Google Cloud OAuth client (type **Desktop app**) | No |
| `KANTOR_GOOGLE_CLIENT_SECRET` | That client's secret. Google gives desktop clients a secret, but it ships in every copy of the app and Google doesn't treat it as confidential | No (but keep it out of the repository anyway) |

The values are compiled in (`option_env!` in `crates/kantor_cloud/src/oauth.rs`). The release, nightly and App Store workflows pass them from **repository variables** of the same names (Settings → Secrets and variables → Actions → Variables), except `KANTOR_GOOGLE_CLIENT_SECRET`, a repository **secret** (Google calls a desktop client's secret non-confidential, but it stays out of logs). The same variables set when Kantor *runs* take precedence, so a registration can be tried on a local build without rebuilding:

```sh
KANTOR_MS_CLIENT_ID=… KANTOR_GOOGLE_CLIENT_ID=… KANTOR_GOOGLE_CLIENT_SECRET=… kantor
```

## Microsoft Entra (OneDrive)

1. Sign in to <https://entra.microsoft.com> with an account that may register apps (any Microsoft account can, in its own default directory). **Identity → Applications → App registrations → New registration**.
2. **Name:** `Kantor`.
3. **Supported account types:** *Accounts in any organizational directory (Any Microsoft Entra ID tenant – Multitenant) and personal Microsoft accounts (e.g. Skype, Xbox)*. This is what `common` needs. (Only one organisation: pick single tenant and build with `KANTOR_MS_TENANT=<tenant id>`.)
4. **Redirect URI:** platform **Public client/native (mobile & desktop)**, URI `http://localhost`. Entra ignores the port for `localhost` redirects of public clients, which is what the loopback listener needs (its port is random). Don't add a *Web* or *SPA* platform.
5. Register, then copy **Application (client) ID** into `KANTOR_MS_CLIENT_ID`.
6. No client secret and no certificate: Kantor is a public client and uses PKCE. (*Allow public client flows* on the **Authentication** page can stay *No*: it only governs the device-code and password flows, which Kantor doesn't use for OneDrive.)
7. **API permissions → Add a permission → Microsoft Graph → Delegated permissions:** `Files.ReadWrite`, `offline_access`, `User.Read` (also add `openid` and `profile`, which Kantor asks for to show the account name). None needs admin consent: each user consents at the first sign-in. Some organisations only allow apps an administrator approved; their admin can grant consent for the tenant on this page (*Grant admin consent*) or from the enterprise application.
8. Optional, recommended before shipping: **Branding & properties** (logo, home page `https://kantor.paparanai.id`, terms and privacy URLs) and **publisher verification** (a verified Microsoft Partner Network id); without it users see "unverified" on the consent screen of work accounts.

What Kantor does with it: `Files.ReadWrite` is the signed-in user's own OneDrive (browse everything in it, open, save, create folders). Files *shared with* the user (and SharePoint libraries) are not shown; that would need `Files.ReadWrite.All` / `Sites.ReadWrite.All`, which work tenants often restrict.

## Google Cloud (Google Drive)

1. <https://console.cloud.google.com> → create a project, e.g. `Kantor`.
2. **APIs & Services → Library:** enable **Google Drive API**.
3. **Google Auth Platform → Branding** (the OAuth consent screen): app name `Kantor`, support e-mail, logo, home page `https://kantor.paparanai.id`, privacy policy and terms URLs, authorised domain `paparanai.id`, developer contact.
4. **Audience:** user type **External**. While the publishing status is *Testing*, only the listed test users (up to 100) can sign in and their refresh tokens expire after 7 days; choose **Publish app** when ready.
5. **Data access → Add or remove scopes:** `https://www.googleapis.com/auth/drive.file` (*See, edit, create and delete only the specific Google Drive files you use with this app*), plus `openid`, `.../auth/userinfo.email` and `.../auth/userinfo.profile`. `drive.file` is a **non-sensitive** scope: publishing needs only the brand verification (domain ownership through Search Console, a few days), **not** the restricted-scope security assessment that `drive` or `drive.readonly` would require.
6. **Clients → Create client → Application type: Desktop app**, name `Kantor desktop`. Copy the client ID and secret into `KANTOR_GOOGLE_CLIENT_ID` and `KANTOR_GOOGLE_CLIENT_SECRET`. Desktop clients accept loopback redirects (`http://127.0.0.1:<any port>`) without registering them.

### The `drive.file` limitation (what users see)

With `drive.file` Kantor only sees files and folders it created, or that were opened with it. So:

- **Open from Cloud** on a Google account lists those files; at the top it shows the ones whose folder Kantor can't see (often "My Drive" itself), and Kantor's own folders (e.g. **Kantor**) with their contents. A short note in the browser says so.
- **Save to Cloud** (File menu, command palette) uploads the document; at the top of Google Drive it goes into a **Kantor** folder that Kantor creates. To work on a file Kantor didn't make, the user opens it on the computer and saves it to the cloud.
- Google's file picker (which grants access to any file the user picks) only runs in a web page. A later version could serve the picker from the loopback listener in the user's browser; that needs an API key and the *Google Picker API* enabled on the same project.

## How saving works (both providers)

- Every save is checked against the version the working copy came from (OneDrive `eTag` in `If-Match`, answered `412` on a change; Google Drive's `version`, checked right before the write since Drive v3 has no conditional writes). A change by someone else opens the usual conflict dialog (keep theirs, overwrite, or save a copy).
- Files of 4 MiB (OneDrive) or 8 MiB (Google Drive) and more go through an upload session in 5 MiB parts. A dropped link resumes from the last part the server has, also after Kantor restarts.
- Neither service offers file locks to apps, so documents are never shown read-only for someone else; the version check above protects saves.
- Save to Cloud shows the upload's progress in the status bar (percent and MB). A failed cloud save or open stays in the status bar until the next key press or click; later messages replace it, older ones never do.
- Cloud documents autosave (10 s after the last change, then upload) unless the status bar switch is turned off on a cloud document (`[cloud] autosave = false` in settings.toml).

## Testing a registration

1. Set the variables (above) and start Kantor.
2. **Cloud Accounts →** the *OneDrive* / *Google Drive* button **→ Sign in with Microsoft / Google**. The browser opens; after consent the tab says you can go back to Kantor. Declining consent, closing the browser page (the sign-in then stops after 5 minutes) or **Cancel** in Kantor counts as cancelled: Cloud Accounts shows a neutral note, not an error.
3. **Open from Cloud**, open a file, edit it, wait for the status bar chip to say *Synced*; check the new version in the web UI. Edit the same file in the web UI, then save in Kantor: the conflict dialog appears.
4. **Save to Cloud** a local document; on Google Drive it appears in the *Kantor* folder.
5. Revoke access (<https://account.live.com/consent/Manage> or <https://myaccount.google.com/permissions>), edit and save in Kantor: the chip says the sign-in ended and the edits stay on the computer; **Sign in again** in Cloud Accounts, then **Sync Now**.

The mock-server tests (`crates/kantor_cloud/tests/providers.rs`) cover listing, download, small and resumable uploads (resumed after a drop), conflicts, token refresh, client secrets, a revoked sign-in and the PKCE browser sign-in, without the real services.
