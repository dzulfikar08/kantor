//! Providers the user can pick, their wire format ([`Api`]) and defaults.
//!
//! Defaults favour cheap, fast models; the user can type any model id.

use std::fmt;

use kantor_core::settings::AiSettings;

use crate::AiError;

/// The wire format a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI Chat Completions, and the many services compatible with it.
    OpenAi,
    /// Google Gemini `generateContent`.
    Gemini,
    /// Ollama's native `/api/chat` (local).
    Ollama,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Anthropic,
    OpenAi,
    OpenRouter,
    Groq,
    DeepSeek,
    Gemini,
    Ollama,
    /// Any OpenAI-compatible server (LM Studio, vLLM, LocalAI, a proxy…).
    Custom,
}

pub struct ProviderInfo {
    pub id: ProviderId,
    /// Stable id used in settings and as the keychain entry name.
    pub key: &'static str,
    pub label: &'static str,
    pub api: Api,
    pub default_base_url: &'static str,
    pub default_model: &'static str,
    pub needs_key: bool,
    /// Whether the settings show a base URL field.
    pub base_url_editable: bool,
    /// Where the user gets a key (shown under the key field).
    pub key_hint: &'static str,
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: ProviderId::Anthropic,
        key: "anthropic",
        label: "Anthropic (Claude)",
        api: Api::Anthropic,
        default_base_url: "https://api.anthropic.com",
        default_model: "claude-haiku-5-5",
        needs_key: true,
        base_url_editable: false,
        key_hint: "Create a key at console.anthropic.com",
    },
    ProviderInfo {
        id: ProviderId::OpenAi,
        key: "openai",
        label: "OpenAI",
        api: Api::OpenAi,
        default_base_url: "https://api.openai.com/v1",
        default_model: "gpt-5-mini",
        needs_key: true,
        base_url_editable: true,
        key_hint: "Create a key at platform.openai.com",
    },
    ProviderInfo {
        id: ProviderId::OpenRouter,
        key: "openrouter",
        label: "OpenRouter",
        api: Api::OpenAi,
        default_base_url: "https://openrouter.ai/api/v1",
        default_model: "openai/gpt-5-mini",
        needs_key: true,
        base_url_editable: true,
        key_hint: "Create a key at openrouter.ai/keys",
    },
    ProviderInfo {
        id: ProviderId::Groq,
        key: "groq",
        label: "Groq",
        api: Api::OpenAi,
        default_base_url: "https://api.groq.com/openai/v1",
        default_model: "llama-3.3-70b-versatile",
        needs_key: true,
        base_url_editable: true,
        key_hint: "Create a key at console.groq.com",
    },
    ProviderInfo {
        id: ProviderId::DeepSeek,
        key: "deepseek",
        label: "DeepSeek",
        api: Api::OpenAi,
        default_base_url: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        needs_key: true,
        base_url_editable: true,
        key_hint: "Create a key at platform.deepseek.com",
    },
    ProviderInfo {
        id: ProviderId::Gemini,
        key: "gemini",
        label: "Google Gemini",
        api: Api::Gemini,
        default_base_url: "https://generativelanguage.googleapis.com",
        default_model: "gemini-2.5-flash",
        needs_key: true,
        base_url_editable: false,
        key_hint: "Create a key at aistudio.google.com",
    },
    ProviderInfo {
        id: ProviderId::Ollama,
        key: "ollama",
        label: "Ollama (local)",
        api: Api::Ollama,
        default_base_url: "http://localhost:11434",
        default_model: "llama3.2",
        needs_key: false,
        base_url_editable: true,
        key_hint: "Runs on this computer; nothing leaves it. Install from ollama.com",
    },
    ProviderInfo {
        id: ProviderId::Custom,
        key: "custom",
        label: "Other (OpenAI-compatible)",
        api: Api::OpenAi,
        default_base_url: "",
        default_model: "",
        needs_key: false,
        base_url_editable: true,
        key_hint: "Optional; sent as a Bearer token",
    },
];

impl ProviderId {
    pub fn info(self) -> &'static ProviderInfo {
        PROVIDERS
            .iter()
            .find(|p| p.id == self)
            .expect("every provider is listed")
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PROVIDERS.iter().find(|p| p.key == key).map(|p| p.id)
    }

    pub fn key(self) -> &'static str {
        self.info().key
    }
}

/// Everything a request needs, resolved from settings and the keychain.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub provider: ProviderId,
    pub model: String,
    /// Without a trailing slash.
    pub base_url: String,
    pub api_key: Option<String>,
}

/// Never prints the key.
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl ProviderConfig {
    /// The model and base URL settings for `provider`, with defaults filled in.
    pub fn resolve(
        provider: ProviderId,
        settings: &AiSettings,
        api_key: Option<String>,
    ) -> Result<Self, AiError> {
        let info = provider.info();
        let saved = settings.providers.get(info.key);
        let pick = |v: Option<&String>, default: &str| {
            v.map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or(default)
                .to_owned()
        };
        let model = pick(saved.and_then(|s| s.model.as_ref()), info.default_model);
        let base_url = if info.base_url_editable {
            pick(
                saved.and_then(|s| s.base_url.as_ref()),
                info.default_base_url,
            )
        } else {
            info.default_base_url.to_owned()
        };
        let base_url = base_url.trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(AiError::MissingSetting("base URL"));
        }
        if model.is_empty() {
            return Err(AiError::MissingSetting("model"));
        }
        let api_key = api_key.filter(|k| !k.trim().is_empty());
        if info.needs_key && api_key.is_none() {
            return Err(AiError::MissingKey {
                provider: info.label,
            });
        }
        Ok(Self {
            provider,
            model,
            base_url,
            api_key,
        })
    }

    /// The configured provider, or [`AiError::Disabled`] when AI is off or
    /// no provider is chosen. `key` loads the API key (keychain) on demand.
    pub fn from_settings(
        settings: &AiSettings,
        key: impl FnOnce(ProviderId) -> Option<String>,
    ) -> Result<Self, AiError> {
        if !settings.enabled {
            return Err(AiError::Disabled);
        }
        let provider = ProviderId::from_key(&settings.provider).ok_or(AiError::Disabled)?;
        let api_key = if provider.info().needs_key || provider == ProviderId::Custom {
            key(provider)
        } else {
            None
        };
        Self::resolve(provider, settings, api_key)
    }

    pub fn api(&self) -> Api {
        self.provider.info().api
    }
}

#[cfg(test)]
mod tests {
    use kantor_core::settings::AiProviderSettings;

    use super::*;

    #[test]
    fn ids_round_trip() {
        for p in PROVIDERS {
            assert_eq!(ProviderId::from_key(p.key), Some(p.id));
            assert_eq!(p.id.info().key, p.key);
        }
        assert_eq!(ProviderId::from_key("nope"), None);
    }

    #[test]
    fn disabled_until_switched_on() {
        let mut s = AiSettings {
            provider: "ollama".into(),
            ..Default::default()
        };
        assert_eq!(
            ProviderConfig::from_settings(&s, |_| None),
            Err(AiError::Disabled)
        );
        s.enabled = true;
        let cfg = ProviderConfig::from_settings(&s, |_| None).unwrap();
        assert_eq!(cfg.base_url, "http://localhost:11434");
        assert_eq!(cfg.model, "llama3.2");
    }

    #[test]
    fn overrides_and_missing_key() {
        let mut s = AiSettings {
            enabled: true,
            provider: "openai".into(),
            ..Default::default()
        };
        assert!(matches!(
            ProviderConfig::from_settings(&s, |_| None),
            Err(AiError::MissingKey { .. })
        ));
        s.providers.insert(
            "openai".into(),
            AiProviderSettings {
                model: Some(" gpt-x ".into()),
                base_url: Some("https://proxy.example/v1/".into()),
            },
        );
        let cfg = ProviderConfig::from_settings(&s, |_| Some("sk-1".into())).unwrap();
        assert_eq!(cfg.model, "gpt-x");
        assert_eq!(cfg.base_url, "https://proxy.example/v1");
        assert!(!format!("{cfg:?}").contains("sk-1"), "key never printed");
    }

    #[test]
    fn custom_needs_a_base_url() {
        let s = AiSettings {
            enabled: true,
            provider: "custom".into(),
            ..Default::default()
        };
        assert_eq!(
            ProviderConfig::from_settings(&s, |_| None),
            Err(AiError::MissingSetting("base URL"))
        );
    }
}
