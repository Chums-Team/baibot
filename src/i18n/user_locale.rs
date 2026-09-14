//! The per-user locale map fed by `cc.chums.set_user_locale` events.

use std::collections::HashMap;
use std::sync::Arc;

use mxlink::matrix_sdk::ruma::{OwnedUserId, UserId};
use tokio::sync::RwLock;

/// In-memory `user id → locale` map. Cheap to clone (shared state).
///
/// Reads (one per message) far outnumber writes (one per language switch), hence the `RwLock`.
/// Nothing is persisted: after a restart every user is back on the fallback locale until their
/// client declares the locale again.
#[derive(Debug, Clone, Default)]
pub struct UserLocaleManager {
    inner: Arc<RwLock<HashMap<OwnedUserId, String>>>,
}

impl UserLocaleManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The locale `user_id` declared, if any.
    pub async fn get(&self, user_id: &UserId) -> Option<String> {
        self.inner.read().await.get(user_id).cloned()
    }

    /// Records the locale of `user_id`, replacing an earlier one.
    pub async fn set(&self, user_id: OwnedUserId, locale: String) {
        self.inner.write().await.insert(user_id, locale);
    }
}

#[cfg(test)]
mod tests {
    use mxlink::matrix_sdk::ruma::user_id;

    use super::*;

    #[tokio::test]
    async fn unknown_user_has_no_locale() {
        let manager = UserLocaleManager::new();
        assert_eq!(manager.get(user_id!("@bob:example.com")).await, None);
    }

    #[tokio::test]
    async fn set_then_get_returns_the_locale() {
        let manager = UserLocaleManager::new();
        let alice = user_id!("@alice:example.com").to_owned();
        manager.set(alice.clone(), "ru".to_owned()).await;
        assert_eq!(manager.get(&alice).await.as_deref(), Some("ru"));
    }

    #[tokio::test]
    async fn set_replaces_the_previous_locale() {
        let manager = UserLocaleManager::new();
        let carol = user_id!("@carol:example.com").to_owned();
        manager.set(carol.clone(), "en".to_owned()).await;
        manager.set(carol.clone(), "de".to_owned()).await;
        assert_eq!(manager.get(&carol).await.as_deref(), Some("de"));
    }

    #[tokio::test]
    async fn clones_share_state() {
        let manager = UserLocaleManager::new();
        let clone = manager.clone();
        let dave = user_id!("@dave:example.com").to_owned();
        clone.set(dave.clone(), "fr".to_owned()).await;
        assert_eq!(manager.get(&dave).await.as_deref(), Some("fr"));
    }
}
