use std::net::IpAddr;

use serde::Deserialize;

/// Static configuration of the optional x402 top-up integration.
///
/// Requires the `billing` section: top-ups are credited to the billing ledger. With this
/// section present, the bot asks the payment sidecar for payment requests (when a room runs
/// out of balance, and on the `topup` chat command) and listens on an internal HTTP endpoint
/// for the sidecar's settlement notifications.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigX402 {
    /// Base URL of the payment sidecar, e.g. `http://127.0.0.1:8402`.
    #[serde(default)]
    pub sidecar_url: String,

    /// Shared secret the sidecar uses to sign settlement notifications (HMAC-SHA256).
    #[serde(default)]
    pub internal_secret: String,

    /// Address of the internal HTTP endpoint the sidecar posts settlements to.
    /// Loopback by default: the endpoint is protected by the HMAC and by network position.
    #[serde(default = "super::defaults::x402_internal_bind")]
    pub internal_bind: IpAddr,

    #[serde(default = "super::defaults::x402_internal_port")]
    pub internal_port: u16,

    /// Explicit opt-in for a non-loopback `internal_bind` (the sidecar runs in another
    /// container or host). The operator is then responsible for firewalling the endpoint.
    #[serde(default)]
    pub allow_non_loopback_bind: bool,
}

impl Default for ConfigX402 {
    fn default() -> Self {
        Self {
            sidecar_url: String::new(),
            internal_secret: String::new(),
            internal_bind: super::defaults::x402_internal_bind(),
            internal_port: super::defaults::x402_internal_port(),
            allow_non_loopback_bind: false,
        }
    }
}

impl ConfigX402 {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.sidecar_url.is_empty() {
            return Err(anyhow::anyhow!(
                "The x402.sidecar_url ({}) configuration must be set",
                super::env::BAIBOT_X402_SIDECAR_URL
            ));
        }

        match url::Url::parse(&self.sidecar_url) {
            Ok(url) if url.scheme() == "http" || url.scheme() == "https" => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "The x402.sidecar_url ({}) configuration must be an http(s) URL, got `{}`",
                    super::env::BAIBOT_X402_SIDECAR_URL,
                    self.sidecar_url,
                ));
            }
        }

        if self.internal_secret.is_empty() {
            return Err(anyhow::anyhow!(
                "The x402.internal_secret ({}) configuration must be set to the secret shared with the payment sidecar",
                super::env::BAIBOT_X402_INTERNAL_SECRET
            ));
        }

        if !self.internal_bind.is_loopback() && !self.allow_non_loopback_bind {
            return Err(anyhow::anyhow!(
                "The x402.internal_bind ({}) configuration is a non-loopback address ({}). Bind on 127.0.0.1, or set x402.allow_non_loopback_bind ({}) to true and firewall the endpoint yourself",
                super::env::BAIBOT_X402_INTERNAL_BIND,
                self.internal_bind,
                super::env::BAIBOT_X402_ALLOW_NON_LOOPBACK_BIND,
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn parse(yaml: &str) -> ConfigX402 {
        serde_yaml_ng::from_str(yaml).expect("valid yaml")
    }

    const MINIMAL: &str = "sidecar_url: http://127.0.0.1:8402\ninternal_secret: s3cret\n";

    #[test]
    fn minimal_section_uses_loopback_defaults() {
        let cfg = parse(MINIMAL);
        cfg.validate().unwrap();
        assert_eq!(cfg.internal_bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cfg.internal_port, 9000);
        assert!(!cfg.allow_non_loopback_bind);
    }

    #[test]
    fn empty_section_fails_validation_naming_the_url() {
        let err = parse("{}").validate().unwrap_err().to_string();
        assert!(err.contains("x402.sidecar_url"), "{err}");
    }

    #[test]
    fn non_http_sidecar_url_is_rejected() {
        let err = parse("sidecar_url: 127.0.0.1:8402\ninternal_secret: s\n")
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("http(s) URL"), "{err}");
    }

    #[test]
    fn missing_secret_is_rejected() {
        let err = parse("sidecar_url: http://127.0.0.1:8402\n")
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("x402.internal_secret"), "{err}");
    }

    #[test]
    fn non_loopback_bind_needs_explicit_opt_in() {
        let yaml = format!("{MINIMAL}internal_bind: 0.0.0.0\n");
        let err = parse(&yaml).validate().unwrap_err().to_string();
        assert!(err.contains("non-loopback"), "{err}");
        assert!(err.contains("allow_non_loopback_bind"), "{err}");

        let yaml = format!("{yaml}allow_non_loopback_bind: true\n");
        let cfg = parse(&yaml);
        cfg.validate().unwrap();
        assert_eq!(cfg.internal_bind, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn malformed_bind_fails_to_parse() {
        let yaml = format!("{MINIMAL}internal_bind: 999.1.1.1\n");
        assert!(serde_yaml_ng::from_str::<ConfigX402>(&yaml).is_err());
    }
}
