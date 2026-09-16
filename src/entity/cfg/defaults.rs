const CONFIG_FILE_PATH: &str = "config.yml";

const NAME: &str = "baibot";
const COMMAND_PREFIX: &str = "!bai";

const PERSISTENCE_SESSION_FILE_NAME: &str = "session.json";
const PERSISTENCE_DB_DIR_NAME: &str = "db";

const BILLING_DB_FILE_NAME: &str = "billing.db";

/// The `origin` the bot reports when asking the Chums homeserver for a TRON login challenge.
/// One value for every instance of the bot; the homeserver lists it in
/// `tron_auth_allowed_origins`. See docs/configuration/authentication.md.
const USER_TRON_ORIGIN: &str = "https://baibot.tron.mx";

pub(crate) fn name() -> String {
    NAME.to_owned()
}

pub(crate) fn config_file_path() -> String {
    CONFIG_FILE_PATH.to_owned()
}

pub(super) fn user_tron_origin() -> String {
    USER_TRON_ORIGIN.to_owned()
}

pub(super) fn command_prefix() -> String {
    COMMAND_PREFIX.to_owned()
}

pub(super) fn room_post_join_self_introduction_enabled() -> bool {
    true
}

pub(super) fn access_commands_admin_exempt() -> Vec<String> {
    ["balance", "topup", "image"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(super) fn i18n_fallback_locale() -> String {
    crate::i18n::DEFAULT_LOCALE.to_owned()
}

pub(super) fn persistence_data_dir_path() -> Option<String> {
    None
}

pub(super) fn persistence_session_file_name() -> String {
    PERSISTENCE_SESSION_FILE_NAME.to_owned()
}

pub(super) fn persistence_db_dir_name() -> String {
    PERSISTENCE_DB_DIR_NAME.to_owned()
}

pub(super) fn logging() -> String {
    "warn,mxlink=debug,baibot=debug".to_owned()
}

pub(super) fn billing_reserve_amount_usd() -> f64 {
    crate::billing::BillingWrapperConfig::DEFAULT_RESERVE
}

pub(super) fn billing_markup_pct() -> f64 {
    crate::billing::BillingWrapperConfig::DEFAULT_MARKUP
}

pub(super) fn billing_daily_cap_usd() -> f64 {
    crate::billing::BillingWrapperConfig::DEFAULT_DAILY_CAP
}

pub(super) fn billing_monthly_cap_usd() -> f64 {
    crate::billing::BillingWrapperConfig::DEFAULT_MONTHLY_CAP
}

pub(super) fn billing_min_topup_usd() -> f64 {
    0.10
}

pub(super) fn billing_max_topup_usd() -> f64 {
    1.0
}

pub(super) fn billing_db_file_name() -> String {
    BILLING_DB_FILE_NAME.to_owned()
}

pub(super) fn x402_internal_bind() -> std::net::IpAddr {
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

pub(super) fn x402_internal_port() -> u16 {
    9000
}
