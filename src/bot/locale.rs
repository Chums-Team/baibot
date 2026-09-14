//! The per-user locale of replies: the `cc.chums.set_user_locale` handler and the lookup the
//! controllers use. See `src/i18n`.

use mxlink::matrix_sdk::ruma::UserId;
use mxlink::matrix_sdk::ruma::events::OriginalSyncMessageLikeEvent;

use crate::i18n;
use crate::matrix::events::SetUserLocaleContent;

use super::Bot;

impl Bot {
    /// The locale for a reply addressed to `user_id`: the one their client declared, otherwise
    /// `i18n.fallback_locale`.
    pub(crate) async fn resolve_user_locale(&self, user_id: &UserId) -> String {
        match self.user_locale_manager().get(user_id).await {
            Some(locale) => locale,
            None => self.i18n_config().fallback_locale.clone(),
        }
    }

    /// Records the locale a client declares with a `cc.chums.set_user_locale` event. Locales the
    /// bot has no translations for are ignored (the user stays on the fallback locale).
    pub(super) fn attach_user_locale_event_handler(&self) {
        let bot = self.clone();

        self.matrix_link().client().add_event_handler(
            move |event: OriginalSyncMessageLikeEvent<SetUserLocaleContent>| {
                let bot = bot.clone();
                async move {
                    match i18n::normalize_locale(&event.content.locale) {
                        Some(locale) => {
                            tracing::debug!(sender = %event.sender, %locale, "User locale declared");
                            bot.user_locale_manager().set(event.sender, locale).await;
                        }
                        None => {
                            tracing::warn!(
                                sender = %event.sender,
                                locale = %event.content.locale,
                                "Ignoring a declared user locale without translations",
                            );
                        }
                    }
                }
            },
        );
    }
}
