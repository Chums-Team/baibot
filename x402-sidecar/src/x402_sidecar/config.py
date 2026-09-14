"""Sidecar configuration — env-vars only, no .env auto-loading in library code.

Loaded at FastAPI startup; missing required values fail fast with a clear error
naming the variable. Defaults: TRON Nile testnet, hosted BofAI facilitator v2,
live mode (the facilitator is really called on submit).
"""
from __future__ import annotations

from pydantic import Field, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

from .permit2_builder import DEFAULT_MAX_TIMEOUT_SECONDS, SCHEME_EXACT
from .utils import TronAddressError, canonical_network


class Settings(BaseSettings):
    """Runtime configuration. See `.env.example` for a full template."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        case_sensitive=False,
        extra="ignore",
    )

    # ===== facilitator (x402 v2: /supported, /verify, /settle) =====
    facilitator_api_key: str = Field(
        ...,
        alias="X402_FACILITATOR_API_KEY",
        description="Seller API key (X-API-KEY). Issued by the facilitator operator.",
    )
    facilitator_url: str = Field(
        default="https://facilitator.bankofai.io",
        alias="X402_FACILITATOR_URL",
        description=(
            "Base URL of an x402 v2 facilitator. Hosted BofAI by default; point "
            "at the self-host TS facilitator (http://<container>:8001) as fallback."
        ),
    )

    # ===== Network + token =====
    network: str = Field(
        default="tron:0xcd8690dc",
        alias="X402_NETWORK",
        description=(
            "CAIP-2 id with hex reference: tron:0x2b6653dc (mainnet) | "
            "tron:0xcd8690dc (nile) | tron:0x94a9059e (shasta). Legacy names "
            "tron:mainnet / tron:nile / tron:shasta are accepted and canonicalised."
        ),
    )
    token: str = Field(
        default="USDT",
        alias="X402_TOKEN",
        description="Token symbol expected on the chosen network (display only).",
    )
    payment_scheme: str = Field(
        default=SCHEME_EXACT,
        alias="X402_SCHEME",
        description=(
            "x402 scheme. Only `exact` (Permit2 asset transfer) is implemented; "
            "the legacy `exact_permit` no longer exists on the facilitator."
        ),
    )
    max_timeout_seconds: int = Field(
        default=DEFAULT_MAX_TIMEOUT_SECONDS,
        alias="X402_MAX_TIMEOUT_SECONDS",
        ge=60,
        le=3595,
        description="Permit deadline window (= paymentRequirements.maxTimeoutSeconds).",
    )

    # ===== Pay-to address (the agent wallet that receives top-ups) =====
    agent_wallet: str = Field(
        ...,
        alias="X402_AGENT_WALLET",
        description="TRON Base58 address — the receiver of all top-ups.",
    )

    # ===== Internal HMAC secret shared with the Rust bot =====
    internal_secret: str = Field(
        ...,
        alias="X402_INTERNAL_SECRET",
        description="HMAC secret for POST /internal/x402-settled bot endpoint.",
    )

    # ===== HMAC secret for the (optional) inbound settlement webhook =====
    facilitator_webhook_secret: str = Field(
        ...,
        alias="X402_FACILITATOR_WEBHOOK_SECRET",
        description=(
            "HMAC secret for POST /webhook/settlement. The v2 facilitator does not "
            "push webhooks; the endpoint stays for a future reconciliation job."
        ),
    )

    # ===== Bot notify URL =====
    bot_notify_url: str = Field(
        default="http://localhost:9000/internal/x402-settled",
        alias="BOT_X402_NOTIFY_URL",
    )

    # ===== Local sidecar bind =====
    bind_host: str = Field(default="0.0.0.0", alias="SIDECAR_BIND_HOST")
    bind_port: int = Field(default=8402, alias="SIDECAR_BIND_PORT")

    # ===== Local SQLite for payment-id mapping =====
    db_path: str = Field(default="./data/sidecar.db", alias="SIDECAR_DB_PATH")

    # ===== Facilitator readiness watch (background, off the payment path) =====
    facilitator_probe_interval_sec: int = Field(
        default=300,
        alias="X402_FACILITATOR_PROBE_INTERVAL_SEC",
        ge=30,
        description="How often the sidecar re-checks /supported + relayer resources.",
    )
    facilitator_min_settles_headroom: int = Field(
        default=5,
        alias="X402_FACILITATOR_MIN_SETTLES",
        ge=1,
        description="Relayer must afford at least this many settles (energy or TRX).",
    )
    facilitator_failure_threshold: int = Field(
        default=3,
        alias="X402_FACILITATOR_FAILURE_THRESHOLD",
        ge=1,
        description="Consecutive /settle failures that flip the readiness flag to false.",
    )
    tron_rpc_url: str | None = Field(
        default=None,
        alias="X402_TRON_RPC_URL",
        description="TronGrid-compatible RPC for relayer resource reads; per-network default when unset.",
    )

    # ===== Logging =====
    log_level: str = Field(default="INFO", alias="LOG_LEVEL")

    # ===== Stub vs. live facilitator =====
    # False (default): /submit runs facilitator /verify + /settle for real.
    # True (dev only): /payment-request emits a real-shaped envelope marked
    # `_stub: true`, and /payment-request/submit acks without calling the
    # facilitator, so nothing is ever settled and the bot is never notified.
    facilitator_use_stub: bool = Field(
        default=False,
        alias="X402_FACILITATOR_USE_STUB",
        description="Dev only. When True, never call the facilitator on submit.",
    )

    @field_validator("network")
    @classmethod
    def _canonical_network(cls, v: str) -> str:
        try:
            return canonical_network(v)
        except TronAddressError as e:
            raise ValueError(str(e)) from None

    @field_validator("payment_scheme")
    @classmethod
    def _only_exact(cls, v: str) -> str:
        if v != SCHEME_EXACT:
            raise ValueError(
                f"X402_SCHEME={v!r} is not supported; only {SCHEME_EXACT!r} "
                "(Permit2) is implemented against the x402 v2 facilitator"
            )
        return v


_settings: Settings | None = None


def get_settings() -> Settings:
    """FastAPI dependency — cached singleton."""
    global _settings
    if _settings is None:
        _settings = Settings()  # type: ignore[call-arg]
    return _settings
