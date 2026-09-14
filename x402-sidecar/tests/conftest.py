"""Shared test setup.

`Settings` auto-loads `./.env` (pydantic-settings). The real sidecar
`.env` lives next to `pyproject.toml`, so running pytest from the package
root would leak operator values (facilitator URL, `USE_STUB=true`, ...)
into every test. Run each test from an empty temp dir instead.

The suite never talks to a facilitator, so stub mode is forced on by
default; the tests that pin the production default (live) drop the
variable explicitly.
"""
from __future__ import annotations

import pytest


@pytest.fixture(autouse=True)
def _isolate_from_dotenv(monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    # Belt and braces: an exported shell env must not steer defaults either.
    for var in (
        "X402_FACILITATOR_URL",
        "X402_NETWORK",
        "X402_SCHEME",
        "X402_FACILITATOR_USE_STUB",
        "X402_MAX_TIMEOUT_SECONDS",
        "SIDECAR_DB_PATH",
        "BOT_X402_NOTIFY_URL",
    ):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setenv("X402_FACILITATOR_USE_STUB", "true")
