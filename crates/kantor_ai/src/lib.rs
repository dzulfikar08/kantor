//! Kantor's AI foundation: providers, HTTP clients, prompts and API-key
//! storage. No GPUI dependency; the UI runs [`Client::complete`] on a worker
//! thread and streams the text back.
//!
//! Privacy rules, enforced by the callers and the prompt builders here:
//! - nothing is sent while the master switch in settings is off;
//! - requests go only to the provider the user chose (no Kantor service);
//! - features send only what they need (sheet headers and a few sample rows,
//!   the selected paragraphs), never whole files;
//! - API keys live in the OS keychain and are never logged.
//!
//! A hosted provider (e.g. "Paparan AI" credits) is one more [`ProviderId`]
//! with its own [`Api`] wire format in [`client`].

pub mod client;
pub mod error;
pub mod keys;
pub mod prompt;
pub mod provider;
pub mod sample;
pub mod transport;

pub use client::{Cancel, ChatRequest, Client};
pub use error::AiError;
pub use provider::{Api, PROVIDERS, ProviderConfig, ProviderId, ProviderInfo};
pub use transport::{HttpRequest, HttpResponse, Transport, UreqTransport};
