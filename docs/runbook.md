## 📓 Runbook: the bot with x402 top-ups

How to take the bot from source to a deployment that bills rooms and accepts USDT (TRC-20) top-ups on TRON, and how to check that it works. This is the Chums deployment; upstream-style installations without billing are covered by [🚀 Installation](./installation.md).

The stack is two containers on one Docker network:

- the **bot** (this repository, [`docker-compose.yml`](../docker-compose.yml)), which talks to the Matrix homeserver and the LLM provider, keeps the ledger and listens for settlement notifications on `http://bot:9000/internal/x402-settled`;
- the **payment sidecar** ([`x402-sidecar/`](../x402-sidecar/README.md), its own compose file), which fronts the x402 facilitator and is reached by the bot at `http://x402-sidecar:8402`.

The Chums client renders the bot's [📡 Matrix events](./matrix-events.md) as the payment widget, the cap banner and the top-up confirmation. Other clients see plain text messages.


### Prerequisites

1. A host with Docker and Docker Compose.
2. A Matrix homeserver and an account for the bot on it. On the Chums homeserver the account is logged into with a TRON wallet: create the account with the wallet from the Chums client (or bind the wallet to an existing account there) and keep the wallet's private key or seed phrase for the bot. Elsewhere, a password or an access token. See [🔐 Authentication](./configuration/authentication.md).
3. An API key for an LLM provider. [OpenRouter](./providers.md) is recommended: the bot charges the real cost of each call reported by OpenRouter; with other providers it charges by the [pricing table](./configuration/billing.md#configuration).
4. An x402 facilitator API key, issued to the TRON wallet that receives the payments. The sidecar's [`.env.example`](../x402-sidecar/.env.example) names the facilitator and the networks.
5. Two random secrets, e.g. from `openssl rand -hex 32`: one shared by the bot and the sidecar (`internal_secret`), one for the sidecar's legacy settlement webhook (`X402_FACILITATOR_WEBHOOK_SECRET`).
6. For the smoke test: a Chums client account and a wallet holding a little USDT and TRX on the chosen network (Nile is a testnet with faucets).

The Chums client must accept USD amounts as strings in the events (all current releases do; see [USD amounts are strings](./matrix-events.md#usd-amounts-are-strings)). When upgrading both, upgrade the client first.


### 1. Shared network

Once per host:

```sh
docker network create chums-shared
```


### 2. Sidecar

```sh
cd x402-sidecar
cp .env.example .env
```

Fill in `.env`: `X402_FACILITATOR_API_KEY`, `X402_NETWORK` (hex CAIP-2 id of Mainnet or Nile), `X402_AGENT_WALLET` (the receiving wallet), `X402_INTERNAL_SECRET`, `X402_FACILITATOR_WEBHOOK_SECRET`. Keep `BOT_X402_NOTIFY_URL=http://bot:9000/internal/x402-settled` and `X402_FACILITATOR_USE_STUB=false`.

```sh
docker compose up -d --build
docker compose logs -f
curl -s http://127.0.0.1:8402/health | jq
```

`/health` must show `"facilitator_mode": "live"` and, after the first probe (a few seconds), `"facilitator_watch": {"ok": true, ...}`. An `ok: false` there names the reason (facilitator unreachable, wrong network, relayer out of resources); the bot forwards that verdict to the client, which then hides the payment button, so fix it before going on.


### 3. Bot

From the repository root:

```sh
cp .env.example .env
cp etc/app/config.yml.dist config.yml
mkdir data
```

`.env` holds `UID`/`GID` (the owner of `./data`) and the secrets you prefer to keep out of `config.yml`; the comments in [`.env.example`](../.env.example) explain each variable.

In `config.yml`, besides the usual homeserver and user settings ([🛠️ Configuration](./configuration/README.md)):

- `user:` — on the Chums homeserver, remove `password` and put the wallet key in `.env` as `BAIBOT_USER_TRON_PRIVATE_KEY` or `BAIBOT_USER_TRON_SEED_PHRASE` ([🔐 Authentication](./configuration/authentication.md#tron-wallet-authentication)). The homeserver must already know the wallet: `curl "https://<homeserver>/_ever/tron-auth/lookup?address=<wallet address>"` answers `{"bound": true, "user_id": "@<mxid_localpart>:<server_name>"}`.
- `billing:` — uncomment the section; set `admin_mxids` to the users who may run the administration commands. The other keys have sensible defaults ([💰 Billing](./configuration/billing.md)).
- `x402:` — uncomment and set it up for the compose layout ([💸 x402 top-ups](./configuration/x402.md)):

  ```yaml
  x402:
    sidecar_url: http://x402-sidecar:8402
    internal_secret: "<the X402_INTERNAL_SECRET of the sidecar, or set BAIBOT_X402_INTERNAL_SECRET in .env>"
    internal_bind: 0.0.0.0
    internal_port: 9000
    allow_non_loopback_bind: true
  ```

  The bind on all interfaces is what lets the sidecar reach the bot across the Docker network; the endpoint is protected by the HMAC and the compose file does not publish it on the host.
- `i18n.fallback_locale`, `room.post_join_self_introduction_text` — the language of the replies to users without a declared locale and an optional introduction per language ([🌍 Localization](./configuration/i18n.md)).
- `access.commands_admin_only: true` — if users should only talk to the bot and manage their balance, without the configuration commands ([🔒 Access](./access.md#-administrator-only-commands)).
- `initial_global_config.handler` — or create the agent later from the chat ([🤖 Agents](./agents.md)). With billing, keep a `max_response_tokens` on the agent so the cost of a single reply stays below `reserve_amount_usd`.

Then either pull the image CI publishes from the `chums` branch, or build it on the host (a Rust release build, about ten minutes):

```sh
docker compose pull && docker compose up -d   # published image
docker compose up -d --build                  # or: local build
docker compose logs -f
```

Expected in the log at start-up: `x402 webhook server listening` with the bind address, `x402 top-ups enabled` with the sidecar URL, then the usual login and sync lines. With the wallet login, the first start logs `logging in through the TRON wallet` with the address and then `Logged in through the TRON wallet`; later starts log `Found an existing session` instead, since the saved session is reused. With `BAIBOT_USER_ENCRYPTION_RECOVERY_PASSPHRASE` set, every start also logs a `Recovery:` line (`secrets imported from secret storage`, or `secret storage created` on the very first start of the account); a `Recovery failed` line stops the bot and tells what to do. The homeserver's log shows the same login as `chums_tron_auth` events (`challenge_issued`, `auth_accepted`). A configuration error (an `x402` section without `billing`, a non-loopback bind without the flag, an unknown locale) stops the bot before it logs in, with the reason.

The sidecar's log should not show connection errors to the bot; it only calls the bot when a payment settles.


### 4. Smoke test

Do it from the Chums client with a test wallet, in the order below. `!bai` stands for your `command_prefix`.

| # | Step | Expected |
|---|---|---|
| 1 | Invite the bot to a direct chat. | It joins and sends the introduction, in the language of your client. |
| 2 | Send any message. | A reply that the room balance is too low, and a payment widget (a `cc.chums.x402_request` event with `facilitator_ok: true`). No LLM call, no rows in the ledger. |
| 3 | Pay through the widget (or after `!bai topup 0.10`). | Within a minute: the sidecar logs the settlement, the bot logs `x402 topup credited` with the `payment_id`, the room shows a confirmation with the transaction hash (a `cc.chums.x402_topup_confirmed` event and a text message). |
| 4 | Send a message; then `!bai balance`. | The LLM answers. The balance dropped by the cost of the call, and the last entries show `reserve`, `charge`, `release` with one `correlation_id`. |
| 5 | Add the bot to a group room. Send a message without mentioning it; then one that mentions it. | Silence, then a reply; the ledger rows belong to the group room's balance. |
| 6 | As a billing administrator: set `billing.daily_cap_usd` to a value below what was spent today, restart the bot, send a message. | A reply that the bot is paused until the next UTC day, a `cc.chums.cap_hit` event (the client shows a banner), no new rows. Restore the cap and restart. |
| 7 | Make the provider fail (an invalid API key on the agent, for instance) and send a message. | The bot's usual error reply for a failed agent call; the ledger has a `reserve` without `charge`/`release`. `!bai billing zombies 0` lists it; `!bai billing manual-release <event_id> "provider outage"` credits it back. Fix the agent. |
| 8 | Switch the client's language. | The replies to you (`balance`, the low-balance and cap messages) follow it; the administration commands stay English. |

Step 3 is the whole payment path: client → sidecar → facilitator → on-chain settlement → sidecar → bot. If the widget appears but the payment never confirms, check in this order: the sidecar log (the `/verify` and `/settle` answers of the facilitator, then `bot notify ok` or `bot notify failed`), then `BOT_X402_NOTIFY_URL` and the shared secret (the bot answers `401` to a bad signature and logs `x402 webhook rejected`), then the bot log.


### 5. Looking at the ledger

The ledger is `data/billing.db`, an SQLite file. The image ships the `sqlite3` client:

```sh
docker compose exec bot sqlite3 /data/billing.db \
  "SELECT type, COUNT(*), ROUND(SUM(amount_usd), 6) FROM billing_events GROUP BY type;"
```

Top-ups are positive, reserves and charges negative, releases positive. Every completed call leaves three rows sharing a `correlation_id`; a `correlation_id` with a single `reserve` row is a zombie:

```sh
docker compose exec bot sqlite3 /data/billing.db \
  "SELECT correlation_id, COUNT(*) FROM billing_events WHERE correlation_id IS NOT NULL GROUP BY correlation_id HAVING COUNT(*) NOT IN (1, 3);"
```

Expected: no rows. The ledger is append-only; corrections are new rows (`manual-release`, `manual-refund`) with the administrator's user id and reason.


### 6. Logs

- Bot: the `logging` key of `config.yml` (or `BAIBOT_LOGGING`), e.g. `warn,baibot=info,baibot::billing=debug,baibot::x402=debug`.
- Sidecar: `LOG_LEVEL=DEBUG` in its `.env`.
- Both log the `payment_id` of a top-up, so one `grep` over both logs follows a payment from the request to the credit; the sidecar also logs the transaction hash.


### 7. Day-to-day

- **Backup**: `data/` (the Matrix session and crypto store, the dynamic configuration, the ledger) and both `.env` files. Copy `billing.db` with `sqlite3 /data/billing.db ".backup /tmp/billing.db"` from inside the container, or stop the bot first; a plain copy of a live SQLite file can be inconsistent. The encryption keys are the one thing `data/` holds that has a copy elsewhere: with `BAIBOT_USER_ENCRYPTION_RECOVERY_PASSPHRASE` set, a bot started with an empty `data/` logs in again and imports them from the account's secret storage ([🔐 Authentication](./configuration/authentication.md#encryption-keys-and-recovery)); the old device stays listed on the account until removed from a client.
- **Upgrade**: `git pull`, then `docker compose pull && docker compose up -d` (or `docker compose up -d --build`) in the repository root and `docker compose up -d --build` in `x402-sidecar/`. The ledger schema is applied idempotently at start-up; older databases keep working. Read the top of the [changelog](../CHANGELOG.md) first.
- **Rotating the shared secret**: change it in both `.env` files (or `config.yml`) and restart both containers together. The sidecar does not retry a notification: a settlement that arrives while the secrets differ is answered `401`, logged as `bot notify failed` on the sidecar, and has to be credited by hand (last bullet).
- **Changing the facilitator or the network**: sidecar-side only (`x402-sidecar/.env`), then restart the sidecar. The bot forwards the network, the contracts and the receiving wallet from the sidecar's answers and has no copy of them.
- **Reconciling a top-up that was paid but never credited** (the bot was down or rejected the notification when the sidecar called it): find the payment in the sidecar's log (`bot notify failed`, with the `payment_id`) or its database, then credit the room with `!bai billing manual-refund <room_id> <amount_usd> "<tx hash>"`.


### 8. Several bots on one host

The compose files name everything for a single bot: the containers `chums-bot` and `chums-x402-sidecar`, the network `chums-shared`, the host port `8402`. A second bot on the same host needs its own names, and its own network: the sidecar finds the bot by the service name `bot`, which two bots on one network would both claim. Every clashing name is a variable with the single-bot value as its default, so an existing deployment keeps working untouched:

| Variable | Compose file | Default |
|---|---|---|
| `CHUMS_NETWORK` | both, the name of the shared network | `chums-shared` |
| `BOT_IMAGE` | `docker-compose.yml` | `ghcr.io/chums-team/baibot:chums` |
| `BOT_CONTAINER_NAME` | `docker-compose.yml` | `chums-bot` |
| `SIDECAR_IMAGE` | `x402-sidecar/docker-compose.yml` | `chums-x402-sidecar:0.2.0` |
| `SIDECAR_CONTAINER_NAME` | `x402-sidecar/docker-compose.yml` | `chums-x402-sidecar` |
| `SIDECAR_HOST_PORT` | `x402-sidecar/docker-compose.yml`, the loopback port of `/health` | `8402` |

They are read from the environment of `docker compose`, not from `.env` (which is the environment of the container). Keep them in a file next to the checkout and export it before every compose call, in both directories, with a compose project name per bot so `docker compose ps` and orphan detection stay separate:

```sh
# second/instance.env
CHUMS_NETWORK=second-net
BOT_CONTAINER_NAME=second-bot
SIDECAR_CONTAINER_NAME=second-x402-sidecar
SIDECAR_IMAGE=second-x402-sidecar:local
SIDECAR_HOST_PORT=8403
```

```sh
cd second && set -a && . ./instance.env && set +a
docker network create "$CHUMS_NETWORK"
(cd x402-sidecar && COMPOSE_PROJECT_NAME=second-x402 docker compose up -d --build)
COMPOSE_PROJECT_NAME=second docker compose up -d
curl -s "http://127.0.0.1:$SIDECAR_HOST_PORT/health" | jq
```

Each bot is its own checkout with its own `.env`, `config.yml` and `data/`; the `x402` section of `config.yml` stays the same, since the service names inside a network do not change. The sidecar runs as the `UID`/`GID` of its `.env` (the image's own user, 1001, by default) and must own `x402-sidecar/data/`.
