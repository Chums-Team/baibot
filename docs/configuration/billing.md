## 💰 Billing

Billing is **optional**. Without a `billing` section in the [static configuration](./README.md#static-configuration), the bot behaves exactly like upstream baibot and never touches money.

With the section present, every LLM text-generation call is accounted for in an append-only ledger (an SQLite file) per room:

1. Before the call, the room balance and the bot-wide spending caps are checked and a small amount (`reserve_amount_usd`) is held.
2. After the call, the actual cost (as reported by the provider, or estimated from token counts via the pricing table) multiplied by `markup_pct` is charged and the hold is released.

Rooms are funded by top-ups. How top-ups arrive is a separate concern (see the x402 integration); the ledger only records them.


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


### What happens around a call

- **Balance below `reserve_amount_usd`**: the bot replies with the current balance and the amount needed, and does not call the LLM.
- **A cap is reached**: the bot replies that the service is paused and when it resumes (the start of the next UTC day or month), and does not call the LLM.
- **The ledger is unavailable** (e.g. the database cannot be read): the bot replies that billing is temporarily unavailable, and does not call the LLM.
- **The provider call fails**: the reserve stays in the ledger as a "zombie" for manual review, because a half-completed call may still have cost money. Zombies are surfaced by the billing administration commands.
- **The call succeeds but the ledger write fails**: the reply is still delivered and the failure is logged for manual reconciliation.
