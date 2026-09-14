## 🔒 Access

This bot employs access control to decide who can use its services and manage its configuration.


### 👋 Joining rooms

The bot automatically joins rooms only when invited by someone considered a bot [👥 user](#-users).


### 👥 Users

The bot can be used by users that match some [dynamically](./configuration/README.md#dynamic-configuration) configured [Matrix user id](https://spec.matrix.org/v1.11/#users) patterns.

Users:

- ✅ can **invite the bot to rooms**
- ✅ can **use all the bot's [features](./features.md)** ([💬 Text Generation](./features.md#-text-generation), [🦻 Speech-to-Text](./features.md#-speech-to-text), etc.) by sending room messages
- ✅ can **mention the bot** in threads and reply chains to provoke it to respond to non-user messages (see [🌟 Features / 💬 Text Generation / On-demand involvement](./features.md#on-demand-involvement))
- ✅ can **change the bot's configuration in a room** (e.g. `!bai config room ...` commands)
- ❌ cannot **change the bot's global configuration** (e.g. `!bai config global ...` commands)
- ❌ cannot **create new [🤖 Agents](./agents.md)** (neither in rooms, nor globally). See [💼 Room-local agent managers](#-room-local-agent-managers) for controlling which users can create agents.

The following commands are available:
- **Show** the currently allowed users: `!bai access users`
- **Set** the list of allowed users: `!bai access set-users SPACE_SEPARATED_PATTERNS`

Example patterns: `@*:example.com @*:another.com @someone:company.org`


### 👮‍♂️ Administrators

Administrators can **manage the bot's configuration and access control**.

Administrators are [👥 Users](#-users) and [💼 Room-local agent managers](#-room-local-agent-managers) implicitly, so they inherit all their permissions.

The bot can be administrated by users that match some [statically](./configuration/README.md#static-configuration) configured [Matrix user id](https://spec.matrix.org/v1.11/#users) patterns.

Administrators cannot be changed without adjusting the bot's configuration on the server.


### 🚧 Administrator-only commands

By default, every [👥 user](#-users) can use the bot's commands (`!bai help`, `!bai config room ...`, `!bai image create ...`, etc.).

Setting `access.commands_admin_only: true` in the [static configuration](./configuration/README.md#static-configuration) (or the `BAIBOT_ACCESS_COMMANDS_ADMIN_ONLY` environment variable) reserves the commands for [👮‍♂️ Administrators](#-administrators): a command sent by a non-administrator is ignored, without a reply. It is meant for deployments where users only talk to the bot (for example, through a client that shows a balance and a payment flow instead of commands).

Conversation is not affected: plain text messages, mentions of the bot and `!bai <free text>` (text generation via the command prefix) work for every user as before.

The commands that everyone may still use are listed in `access.commands_admin_exempt` (or the `BAIBOT_ACCESS_COMMANDS_ADMIN_EXEMPT` environment variable, space- or comma-separated), by their first word. The default is `balance`, `topup` and `image`: a room's [💰 billing](./configuration/billing.md) balance is shared, so anyone in the room can look at it and [pay into it](./configuration/x402.md), and anyone can generate images. The known commands are `help`, `access`, `provider`, `agent`, `config`, `image`, `sticker`, `usage`, `balance`, `topup`, `stats` and `billing`; an exemption applies to the whole command (`billing` covers `billing zombies`, etc.), and a command's own administrator checks (for example, on `billing zombies` or `config global`) still apply.

Example:

```yaml
access:
  admin_patterns:
    - "@admin:example.com"
  commands_admin_only: true
  commands_admin_exempt: [balance, topup]
```


### 💼 Room-local agent managers

Room-local agent managers are users privileged to **create their own [agents](./agents.md)** (see `!bai agent`) in rooms.

> [!WARNING]
> Letting regular users create agents which contact arbitrary network services **may be a security issue**.

The following commands are available:
- **Show** the currently allowed users: `!bai access room-local-agent-managers`
- **Set** the list of allowed users: `!bai access set-room-local-agent-managers SPACE_SEPARATED_PATTERNS`

Example patterns: `@*:example.com @*:another.com @someone:company.org`
