use std::collections::BTreeMap;

use mxlink::MessageResponseType;
use mxlink::matrix_sdk::{Room, RoomMemberships};

use crate::entity::RoomConfigContext;
use crate::{Bot, strings};

pub async fn handle(
    bot: &Bot,
    room: &Room,
    room_config_context: &RoomConfigContext,
) -> anyhow::Result<()> {
    if !bot.post_join_self_introduction_enabled() {
        tracing::debug!(
            "Post-join self-introduction is disabled - not sending introduction message"
        );

        return Ok(());
    }

    let text = match configured_introduction(bot, room).await {
        Some(text) => text,
        None => {
            strings::introduction::create_on_join_introduction(
                bot.name(),
                bot.command_prefix(),
                bot.agent_manager(),
                room_config_context,
            )
            .await
        }
    };

    bot.messaging()
        .send_text_markdown_no_fail(room, text, MessageResponseType::InRoom)
        .await;

    Ok(())
}

/// The `room.post_join_self_introduction_text` entry for the room, in the locale of the user
/// who invited the bot (otherwise `i18n.fallback_locale`). `None` when the configuration has no
/// such texts: the built-in introduction is sent then.
async fn configured_introduction(bot: &Bot, room: &Room) -> Option<String> {
    let texts = bot.post_join_self_introduction_text();
    if texts.is_empty() {
        return None;
    }

    let fallback_locale = &bot.i18n_config().fallback_locale;
    let locale = inviter_locale(bot, room)
        .await
        .unwrap_or_else(|| fallback_locale.clone());

    let text = pick_text(texts, &locale, fallback_locale);
    if text.is_none() {
        // Config validation requires a text for the fallback locale, so this cannot happen.
        tracing::warn!(
            %locale,
            %fallback_locale,
            "No post-join introduction text for the locale - sending the built-in introduction"
        );
    }

    text.map(ToOwned::to_owned)
}

/// The text for `locale`, otherwise for `fallback_locale`.
fn pick_text<'a>(
    texts: &'a BTreeMap<String, String>,
    locale: &str,
    fallback_locale: &str,
) -> Option<&'a str> {
    texts
        .get(locale)
        .or_else(|| texts.get(fallback_locale))
        .map(String::as_str)
}

/// Best effort: the declared locale (see `cc.chums.set_user_locale`) of the first joined member
/// other than the bot who has one. In a direct chat that is the user who invited the bot; in a
/// group room it may be another member. `None` when nobody has declared a locale or the
/// members cannot be fetched.
async fn inviter_locale(bot: &Bot, room: &Room) -> Option<String> {
    let members = match room.members(RoomMemberships::JOIN).await {
        Ok(members) => members,
        Err(err) => {
            tracing::warn!(
                ?err,
                "Failed to fetch the room members - using the fallback locale for the introduction"
            );
            return None;
        }
    };

    // Keep the client in a local: `user_id()` borrows from it.
    let client = bot.matrix_link().client();
    let bot_user_id = client.user_id()?;

    for member in &members {
        if member.user_id() == bot_user_id {
            continue;
        }

        if let Some(locale) = bot.user_locale_manager().get(member.user_id()).await {
            tracing::debug!(
                user_id = %member.user_id(),
                %locale,
                "Using a member's declared locale for the introduction"
            );
            return Some(locale);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("en".to_owned(), "Hi!".to_owned()),
            ("ru".to_owned(), "Привет!".to_owned()),
        ])
    }

    #[test]
    fn picks_the_text_of_the_locale() {
        assert_eq!(pick_text(&texts(), "ru", "en"), Some("Привет!"));
    }

    #[test]
    fn falls_back_to_the_fallback_locale() {
        assert_eq!(pick_text(&texts(), "de", "en"), Some("Hi!"));
    }

    #[test]
    fn none_when_neither_locale_has_a_text() {
        assert_eq!(pick_text(&texts(), "de", "fr"), None);
        assert_eq!(pick_text(&BTreeMap::new(), "en", "en"), None);
    }
}
