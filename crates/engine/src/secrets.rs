//! API keys: environment variable first, then the macOS Keychain.

use anyhow::Result;
use writer_llm::ProviderKind;

/// Returned (inside `anyhow::Error`) when no key is configured, so a UI can
/// ask for one instead of just showing the message.
#[derive(Debug, thiserror::Error)]
#[error("no API key for {provider}. Set {} or run `writer key set {provider}`.", provider.api_key_env())]
pub struct MissingKey {
    pub provider: ProviderKind,
}

#[cfg(target_os = "macos")]
const SERVICE: &str = "app.writer.writer";

pub fn api_key(provider: ProviderKind) -> Result<String> {
    if let Ok(key) = std::env::var(provider.api_key_env())
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_owned());
    }
    if let Some(key) = keychain::get(provider)? {
        return Ok(key);
    }
    Err(MissingKey { provider }.into())
}

pub fn store_key(provider: ProviderKind, key: &str) -> Result<()> {
    keychain::set(provider, key)
}

pub fn delete_key(provider: ProviderKind) -> Result<()> {
    keychain::delete(provider)
}

#[cfg(target_os = "macos")]
mod keychain {
    use super::SERVICE;
    use anyhow::{Context, Result};
    use writer_llm::ProviderKind;

    fn entry(provider: ProviderKind) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &provider.to_string()).context("opening the keychain")
    }

    pub fn get(provider: ProviderKind) -> Result<Option<String>> {
        match entry(provider)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("reading the API key from the keychain"),
        }
    }

    pub fn set(provider: ProviderKind, key: &str) -> Result<()> {
        entry(provider)?
            .set_password(key)
            .context("saving the API key to the keychain")
    }

    pub fn delete(provider: ProviderKind) -> Result<()> {
        match entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("deleting the API key from the keychain"),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod keychain {
    use anyhow::{Result, bail};
    use writer_llm::ProviderKind;

    pub fn get(_: ProviderKind) -> Result<Option<String>> {
        Ok(None)
    }

    pub fn set(provider: ProviderKind, _: &str) -> Result<()> {
        bail!(
            "keychain storage is only available on macOS for now; set {} instead",
            provider.api_key_env()
        )
    }

    pub fn delete(provider: ProviderKind) -> Result<()> {
        set(provider, "")
    }
}
