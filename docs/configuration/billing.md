## 💰 Billing

Billing is **optional**. Without a `billing` section in the [static configuration](./README.md#static-configuration), the bot behaves exactly like upstream baibot and never touches money.

With the section present, every LLM text-generation call is accounted for in an append-only ledger (an SQLite file) per room:

1. Before the call, the room balance and the bot-wide spending caps are checked and a small amount (`reserve_amount_usd`) is held.
2. After the call, the actual cost (as reported by the provider, or estimated from token counts via the pricing table) multiplied by `markup_pct` is charged and the hold is released.

Only [OpenRouter](../providers.md#openrouter) reports the cost of a call: after each text generation, the bot asks OpenRouter for the cost of that completion. For every other provider (and when the lookup fails), the cost is estimated from the token counts via the pricing table.

Rooms are funded by top-ups. Users can pay into a room through the optional [💸 x402 top-ups](./x402.md) integration; administrators can credit a room by hand. The ledger only records them.


### Configuration

All keys are optional. An empty section (`billing: {}`) enables billing with the defaults shown below.

```yaml
billing:
  reserve_amount_usd: 0.03
  markup_pct: 2.0
  daily_cap_usd: 50.0
  monthly_cap_usd: 1000.0
  min_topup_usd: 0.10
  max_topup_usd: 1.0
  admin_mxids:
    - "@admin:example.com"
  db_path: null
  pricing:
    x-ai/grok-4.20:
      input_usd_per_million_tokens: 1.25
      output_usd_per_million_tokens: 2.5
```

| Key | Meaning |
|---|---|
| `reserve_amount_usd` | Amount (USD) held off the room balance for the duration of one LLM call. A room whose balance is below this value cannot trigger a call. |
| `markup_pct` | Multiplier applied to the provider's cost. `1.0` charges the cost as is, `2.0` charges double. |
| `daily_cap_usd`, `monthly_cap_usd` | Bot-wide (all rooms together) spending caps per UTC calendar day and month. When a cap would be exceeded, the call is refused. |
| `min_topup_usd`, `max_topup_usd` | Bounds for a single top-up request from a user. |
| `admin_mxids` | Full Matrix user ids allowed to run billing administration commands. This list is independent of `access.admin_patterns`. |
| `db_path` | Path to the ledger file. Defaults to `billing.db` inside `persistence.data_dir_path`. |
| `pricing` | Fallback prices per model id, in USD per million tokens (the way providers publish them). Used only when the provider does not report the cost of a call. Entries add to, or override, the built-in table. |

Every key can be overridden with an environment variable, following the usual naming rule (`billing.daily_cap_usd` → `BAIBOT_BILLING_DAILY_CAP_USD`). `BAIBOT_BILLING_ADMIN_MXIDS` takes a space-separated list. Setting any `BAIBOT_BILLING_*` variable enables billing even when the configuration file has no `billing` section.

> [!WARNING]
> If the model used for text generation is neither priced by the provider nor present in the pricing table, its calls are charged `$0` and the charge row is flagged with `unknown_model_audit: true` in its metadata, so the room is not blocked but the bot effectively pays for the call. Make sure every paid model you use has a pricing entry.


### Per-room markup

A room may override `markup_pct` through the `billing.markup_override_pct` key of its [room configuration](./README.md#dynamic-configuration). Values that are not positive finite numbers are ignored. There is no chat command for this setting yet.


### Chat commands

When billing is configured, the following commands are available (the prefix is your `command_prefix`, `!bai` by default). Without a `billing` section none of them exist, and such messages are handled like any other text.

Available to everyone in the room:

- `!bai balance` — the balance of the current room and its last 5 ledger entries.
- `!bai topup [<amount_usd>]` — pay into the room balance. Needs the [x402 integration](./x402.md).
- `!bai billing` (or `!bai billing help`) — a summary of the billing commands.

Available to the users listed in `billing.admin_mxids` only:

- `!bai stats day` / `!bai stats month` — bot-wide spending for the current UTC day or month against the corresponding cap.
- `!bai billing zombies [<minutes>]` — reserves older than the given number of minutes (default `10`) that were never charged or released. These appear when a provider call fails mid-way.
- `!bai billing manual-release <event_id> "<reason>"` — credits a zombie reserve back to its room. `<event_id>` is the ledger id shown by `billing zombies`; the reason is recorded in the ledger together with the administrator's user id.
- `!bai billing manual-refund <room_id> <amount_usd> "<reason>"` — credits an amount to a room, e.g. after returning a top-up off-chain. Also recorded with the reason and the administrator's user id.

The reason is mandatory and must be enclosed in double quotes.

The replies to everyone (`balance`, `topup`, the summary, the messages about a low balance or a reached cap) are sent in the user's language, see [🌍 Localization](./i18n.md). The administration commands answer in English.


### What happens around a call

- **Balance below `reserve_amount_usd`**: the bot replies with the current balance and the amount needed, and does not call the LLM. With the [x402 integration](./x402.md) configured, it also sends a [`cc.chums.x402_request` event](../matrix-events.md) so the Chums client offers to pay the missing amount right away.
- **A cap is reached**: the bot replies that the service is paused and when it resumes (the start of the next UTC day or month), and does not call the LLM. It also sends a [`cc.chums.cap_hit` event](../matrix-events.md) that the Chums client renders as a banner.
- **The ledger is unavailable** (e.g. the database cannot be read): the bot replies that billing is temporarily unavailable, and does not call the LLM.
- **The provider call fails**: the reserve stays in the ledger as a "zombie" for manual review, because a half-completed call may still have cost money. Zombies are surfaced by the billing administration commands.
- **The call succeeds but the ledger write fails**: the reply is still delivered and the failure is logged for manual reconciliation.
