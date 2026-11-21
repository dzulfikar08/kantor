//! Shared, UI-agnostic pieces of Kantor. No GPUI dependency.

/// The Mac App Store build (`kantor --features app-store`): no self-update,
/// no outside programs, files only through the App Sandbox's rules
/// (docs/app-store.md). Every crate sees the same value: the feature is
/// turned on here, through `kantor`'s `app-store` feature.
pub const APP_STORE: bool = cfg!(feature = "app-store");

/// Running inside the macOS App Sandbox (the store build on macOS).
pub const SANDBOXED: bool = APP_STORE && cfg!(target_os = "macos");

/// The store build's bundle identifier, which names its sandbox container.
pub const BUNDLE_ID: &str = "com.dzulfikar.kantor";

pub mod dirs;
pub mod find;
pub mod fonts;
pub mod fsx;
pub mod fuzzy;
pub mod layout;
pub mod recovery;
pub mod session;
pub mod settings;
