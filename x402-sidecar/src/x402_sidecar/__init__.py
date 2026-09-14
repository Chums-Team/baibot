"""x402 sidecar — thin FastAPI service in front of an x402 v2 facilitator.

Bridges between the Rust bot (which speaks plain HTTP to this sidecar) and the
x402 facilitator (which speaks the x402 wire format on top of TIP-712 /
EIP-712 signed payloads).

Endpoints (see `app.py`):
- `GET  /health`                  — liveness + config echo.
- `POST /payment-request`         — create a payment requirement.
- `POST /payment-request/submit`  — verify + settle a signed permit.
- `GET  /status/{payment_id}`     — read-back payment state.
- `POST /webhook/settlement`      — legacy push path (HMAC), kept for reconciliation.
"""

__version__ = "0.2.0"
