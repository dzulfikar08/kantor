# Privacy

Kantor is a desktop app. It has no account, no analytics and no telemetry, and it doesn't send
your documents anywhere. It talks to the network in only these cases:

1. **Update checks.** At most once a day, Kantor fetches a small signed manifest from
   `https://kantor.paparanai.id/update/stable.json` (redirected to the GitHub release). The
   request carries only Kantor's version, your operating system and CPU type, in the
   User-Agent (for example `Kantor/0.4.0 (windows; x86_64)`). No identifier, no usage data,
   nothing about your files. If an update is available, Kantor downloads it from GitHub.
   Turn this off with `[update] enabled = false` in `settings.toml`; Kantor then makes no
   update requests at all. Details: [README, Updates](README.md#updates).
2. **AI features, only if you turn them on.** AI is off by default. If you choose a provider
   (Paparan AI, Anthropic, OpenAI, Google and others, or Ollama on your own computer) and run an AI
   command, Kantor sends only what that command needs to the provider you chose, under that
   provider's terms. Details: [README, AI features and privacy](README.md#ai-features-optional-and-privacy).
3. **Formulas that download, in Sheets.** A workbook's `IMAGE` and `WEBSERVICE` formulas name
   addresses on the internet, and Kantor fetches them as Excel does: the picture an `IMAGE`
   formula shows, the text a `WEBSERVICE` formula returns. So opening a workbook can contact
   the servers its author chose, which see your IP address, the time and the User-Agent
   (`Kantor/<version> (<os>; <cpu>)`), and the address itself may carry data from the
   workbook. Nothing else is sent: no cookies, no file contents beyond what the formula puts
   in its address. A workbook you open from a file asks first, with a bar above the grid
   ("This workbook downloads pictures or data from the internet. Allow / Block"); until you
   click Allow it downloads nothing (pictures saved in the file still show). New workbooks
   and formulas you type download right away. Downloads are kept in memory until Kantor
   quits, never on disk. Turn this off everywhere with *Let Formulas Download from the
   Internet* in the command palette (`Ctrl K`) or `[sheets] web_formulas = false` in
   `settings.toml`: the formulas then show `#BLOCKED!`. `kantor convert` never downloads.
4. **Checking PDF signatures online, only when you click it.** A signed PDF is checked with what
   the file holds. When a certificate's issuer or revocation status is missing, the Signatures
   panel offers *Check Online*; only then does Kantor download the missing issuer certificates
   and ask the certificate authorities named in the certificates (their OCSP responders and
   certificate revocation lists) whether the certificates were revoked. Those servers see your
   IP address, the time, the User-Agent and which certificate is asked about (its serial number
   and issuer), not the document. Turn the offer off with *Let Signature Checks Use the
   Internet* in the command palette or `[pdf] online_signature_checks = false` in
   `settings.toml`. `kantor pdf verify` goes online only with `--online`.
5. **Cloud drives, only the ones you add.** Files you open from a WebDAV account go to and from
   that server.
6. **Google Drive and Microsoft OneDrive, only if you sign in.** When you add one of these accounts
   in *Cloud Accounts*, you sign in in your own browser, with Google or Microsoft, never with
   Kantor. Kantor then talks to Google or Microsoft directly from your computer. There is no Kantor
   server in between, and nothing goes to the Kantor project.
   - **What Kantor can reach.** On Google Drive it asks only for the `drive.file` permission: the
     files and folders Kantor created or that you opened with it, not the rest of your Drive. On
     OneDrive it asks for `Files.ReadWrite`: your own OneDrive. With both it also gets your name and
     email address, which *Cloud Accounts* shows so you can tell accounts apart.
   - **What it does with it.** Lists folders, opens the files you choose, saves your edits back
     (when cloud autosave is on, a few seconds after you stop typing; turn it off with
     `[cloud] autosave = false` in `settings.toml`) and creates the files and folders you ask for.
     Nothing else: Kantor doesn't read other files, doesn't keep copies anywhere but your computer,
     doesn't show ads, doesn't sell or share the data, and doesn't use it to train AI models.
   - **AI features.** If you turned on an AI provider and run an AI command on a document you
     opened from Drive or OneDrive, that command sends what it needs to the provider you chose, as
     in point 2. Nothing is sent to an AI provider unless you run such a command.
   - **Sign-in token.** Google or Microsoft gives Kantor a token so you stay signed in. It is kept
     in your system's keychain (or, where there is none, in a file only your user account can
     read) and is sent only back to Google or Microsoft.
   - **Removing access.** Removing the account in *Cloud Accounts* deletes the token from your
     computer and, for Google, revokes it. You can also revoke Kantor's access at any time at
     <https://myaccount.google.com/permissions> or <https://account.live.com/consent/Manage>
     (work and school accounts: <https://myapps.microsoft.com>).
   - Kantor's use and transfer of information received from Google APIs adheres to the
     [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
     including the Limited Use requirements.

The update server (Cloudflare Pages and GitHub) may keep standard request logs (IP address,
time, User-Agent) under their own policies. The Kantor project doesn't collect or keep them.

Everything else stays on your computer: settings, recovery copies of unsaved work
(see [README, Your work is safe](README.md#your-work-is-safe)), and the local MCP connection that AI
assistants on your computer can use, which accepts only connections from your own user account.

Questions: open an issue at https://github.com/dzulfikar08/kantor/issues.
