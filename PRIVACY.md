# Privacy

Kantor is a desktop app. It has no account, no analytics and no telemetry, and it doesn't send
your documents anywhere. It talks to the network in only two cases:

1. **Update checks.** At most once a day, Kantor fetches a small signed manifest from
   `https://kantor.paparanai.id/update/stable.json` (redirected to the GitHub release). The
   request carries only Kantor's version, your operating system and CPU type, in the
   User-Agent (for example `Kantor/0.4.0 (windows; x86_64)`). No identifier, no usage data,
   nothing about your files. If an update is available, Kantor downloads it from GitHub.
   Turn this off with `[update] enabled = false` in `settings.toml`; Kantor then makes no
   update requests at all. Details: [README, Updates](README.md#updates).
2. **AI features, only if you turn them on.** AI is off by default. If you choose a provider
   (Anthropic, OpenAI, Google and others, or Ollama on your own computer) and run an AI
   command, Kantor sends only what that command needs to the provider you chose, under that
   provider's terms. Details: [README, AI features and privacy](README.md#ai-features-optional-and-privacy).

The update server (Cloudflare Pages and GitHub) may keep standard request logs (IP address,
time, User-Agent) under their own policies. The Kantor project doesn't collect or keep them.

Everything else stays on your computer: settings, recovery copies of unsaved work
(see [README, Your work is safe](README.md#your-work-is-safe)), and the local MCP connection that AI
assistants on your computer can use, which accepts only connections from your own user account.

Questions: open an issue at https://github.com/dzulfikar08/kantor/issues.
