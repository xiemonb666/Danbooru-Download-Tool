use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    Danbooru,
    Vllm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretStatus {
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    StorageUnavailable,
    StorageFailure,
    VerificationFailed,
}

pub trait CredentialVault: Send + Sync {
    fn get(&self, kind: SecretKind) -> Result<Option<String>, SecretError>;
    fn set(&self, kind: SecretKind, value: &str) -> Result<(), SecretError>;
    fn delete(&self, kind: SecretKind) -> Result<(), SecretError>;
}

#[derive(Debug, Default)]
pub struct SystemCredentialVault;

impl SystemCredentialVault {
    const SERVICE: &'static str = "DanbooruDownloadToolPro";

    pub fn account(kind: SecretKind) -> &'static str {
        match kind {
            SecretKind::Danbooru => "danbooru-api-key",
            SecretKind::Vllm => "vllm-api-key",
        }
    }

    fn entry(kind: SecretKind) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(Self::SERVICE, Self::account(kind))
            .map_err(|_| SecretError::StorageUnavailable)
    }
}

impl CredentialVault for SystemCredentialVault {
    fn get(&self, kind: SecretKind) -> Result<Option<String>, SecretError> {
        match Self::entry(kind)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretError::StorageFailure),
        }
    }

    fn set(&self, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        Self::entry(kind)?
            .set_password(value)
            .map_err(|_| SecretError::StorageFailure)
    }

    fn delete(&self, kind: SecretKind) -> Result<(), SecretError> {
        match Self::entry(kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(SecretError::StorageFailure),
        }
    }
}

pub struct SecretManager {
    session: Mutex<HashMap<SecretKind, String>>,
    vault: Option<Arc<dyn CredentialVault>>,
}

impl SecretManager {
    pub fn session_only() -> Self {
        Self {
            session: Mutex::new(HashMap::new()),
            vault: None,
        }
    }

    pub fn with_vault(vault: Arc<dyn CredentialVault>) -> Self {
        Self {
            session: Mutex::new(HashMap::new()),
            vault: Some(vault),
        }
    }

    pub fn set_session(&self, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        self.session
            .lock()
            .map_err(|_| SecretError::StorageFailure)?
            .insert(kind, value.to_owned());
        Ok(())
    }

    pub fn status(&self, kind: SecretKind) -> Result<SecretStatus, SecretError> {
        let in_session = self
            .session
            .lock()
            .map_err(|_| SecretError::StorageFailure)?
            .contains_key(&kind);
        let configured = if in_session {
            true
        } else if let Some(vault) = &self.vault {
            vault.get(kind)?.is_some()
        } else {
            false
        };
        Ok(SecretStatus { configured })
    }

    pub fn set_persistent(&self, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        let vault = self.vault.as_ref().ok_or(SecretError::StorageUnavailable)?;
        vault.set(kind, value)?;
        match vault.get(kind)? {
            Some(stored) if stored == value => {
                self.session
                    .lock()
                    .map_err(|_| SecretError::StorageFailure)?
                    .remove(&kind);
                Ok(())
            }
            _ => {
                let _ = vault.delete(kind);
                Err(SecretError::VerificationFailed)
            }
        }
    }

    pub fn get_for_internal_use(&self, kind: SecretKind) -> Result<Option<String>, SecretError> {
        if let Some(value) = self
            .session
            .lock()
            .map_err(|_| SecretError::StorageFailure)?
            .get(&kind)
            .cloned()
        {
            return Ok(Some(value));
        }
        match &self.vault {
            Some(vault) => vault.get(kind),
            None => Ok(None),
        }
    }

    pub fn delete(&self, kind: SecretKind) -> Result<(), SecretError> {
        self.session
            .lock()
            .map_err(|_| SecretError::StorageFailure)?
            .remove(&kind);
        if let Some(vault) = &self.vault {
            vault.delete(kind)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialVault, SecretError, SecretKind, SecretManager, SystemCredentialVault};
    use std::sync::{Arc, Mutex};

    struct BlindVault;

    impl CredentialVault for BlindVault {
        fn get(&self, _kind: SecretKind) -> Result<Option<String>, SecretError> {
            Ok(None)
        }

        fn set(&self, _kind: SecretKind, _value: &str) -> Result<(), SecretError> {
            Ok(())
        }

        fn delete(&self, _kind: SecretKind) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryVault(Mutex<Option<String>>);

    impl CredentialVault for MemoryVault {
        fn get(&self, _kind: SecretKind) -> Result<Option<String>, SecretError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn set(&self, _kind: SecretKind, value: &str) -> Result<(), SecretError> {
            *self.0.lock().unwrap() = Some(value.to_owned());
            Ok(())
        }

        fn delete(&self, _kind: SecretKind) -> Result<(), SecretError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn public_status_never_serializes_secret_value() {
        let manager = SecretManager::session_only();
        manager
            .set_session(SecretKind::Danbooru, "super-secret")
            .unwrap();

        let json = serde_json::to_value(manager.status(SecretKind::Danbooru).unwrap()).unwrap();

        assert_eq!(json, serde_json::json!({ "configured": true }));
        assert!(!json.to_string().contains("super-secret"));
    }

    #[test]
    fn persistent_secret_requires_successful_readback() {
        let manager = SecretManager::with_vault(Arc::new(BlindVault));

        let result = manager.set_persistent(SecretKind::Vllm, "secret");

        assert_eq!(result, Err(SecretError::VerificationFailed));
        assert!(!manager.status(SecretKind::Vllm).unwrap().configured);
    }

    #[test]
    fn verified_persistent_secret_replaces_an_older_session_secret() {
        let manager = SecretManager::with_vault(Arc::new(MemoryVault::default()));
        manager
            .set_session(SecretKind::Vllm, "older-session-value")
            .unwrap();

        manager
            .set_persistent(SecretKind::Vllm, "new-persistent-value")
            .unwrap();

        assert_eq!(
            manager.get_for_internal_use(SecretKind::Vllm).unwrap(),
            Some("new-persistent-value".to_string())
        );
    }

    #[test]
    fn internal_client_can_read_session_secret_without_public_exposure() {
        let manager = SecretManager::session_only();
        manager
            .set_session(SecretKind::Danbooru, "session-secret")
            .unwrap();

        assert_eq!(
            manager.get_for_internal_use(SecretKind::Danbooru).unwrap(),
            Some("session-secret".to_string())
        );
    }

    #[test]
    fn deleting_secret_clears_session_and_vault_state() {
        let manager = SecretManager::session_only();
        manager
            .set_session(SecretKind::Danbooru, "session-secret")
            .unwrap();

        manager.delete(SecretKind::Danbooru).unwrap();

        assert!(!manager.status(SecretKind::Danbooru).unwrap().configured);
    }

    #[test]
    fn system_vault_uses_separate_stable_slots() {
        assert_eq!(
            SystemCredentialVault::account(SecretKind::Danbooru),
            "danbooru-api-key"
        );
        assert_eq!(
            SystemCredentialVault::account(SecretKind::Vllm),
            "vllm-api-key"
        );
    }
}
