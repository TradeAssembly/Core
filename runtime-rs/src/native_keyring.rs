//! Explicit native stores preserve the existing service/user lookup identity.
//! No fallback, migration, credential read, or global store replacement occurs here.

use std::sync::{Arc, LazyLock};

static STORE: LazyLock<keyring::Result<Arc<keyring::CredentialStore>>> =
    LazyLock::new(native_store);

pub(crate) fn entry(service: &str, user: &str) -> keyring::Result<keyring::Entry> {
    match &*STORE {
        Ok(store) => store.build(service, user, None),
        Err(_) => Err(keyring::Error::NoDefaultStore),
    }
}

fn native_store() -> keyring::Result<Arc<keyring::CredentialStore>> {
    #[cfg(target_os = "macos")]
    return Ok(apple_native_keyring_store::keychain::Store::new()?);

    #[cfg(target_os = "windows")]
    return Ok(windows_native_keyring_store::Store::new()?);

    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    return Ok(dbus_secret_service_keyring_store::Store::new()?);

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        all(
            unix,
            not(any(target_os = "macos", target_os = "ios", target_os = "android"))
        )
    )))]
    Err(keyring::Error::NoDefaultStore)
}
