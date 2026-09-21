## 🔐 Authentication

baibot supports 3 authentication modes for the Matrix account (`user.*` keys in config).

Set **exactly one** mode. If several are set (or none is set), startup validation fails.

### Password authentication

- Config key: `user.password`
- Environment variable: `BAIBOT_USER_PASSWORD`

### Access token authentication

- Config keys: `user.access_token` + `user.device_id`
- Environment variables: `BAIBOT_USER_ACCESS_TOKEN` + `BAIBOT_USER_DEVICE_ID`

Access-token authentication is useful for OIDC-enabled homeservers (e.g. those using [Matrix Authentication Service](https://github.com/element-hq/matrix-authentication-service)).

Example token-generation command:

```sh
mas-cli manage issue-compatibility-token <username> [device_id]
```


### TRON wallet authentication

- Config keys: `user.tron.private_key` **or** `user.tron.seed_phrase`; optionally `user.tron.origin`
- Environment variables: `BAIBOT_USER_TRON_PRIVATE_KEY` / `BAIBOT_USER_TRON_SEED_PHRASE` / `BAIBOT_USER_TRON_ORIGIN`

For the Chums homeserver, where password login is switched off and accounts belong to TRON wallets. The bot signs a challenge with the wallet key and the homeserver logs it in as the user the wallet is bound to (login type `cc.chums.login.tron`, served by the `chums` module of the homeserver).

```yaml
user:
  mxid_localpart: baibot
  tron:
    # Either the private key (32 bytes in hex, optionally 0x-prefixed) …
    private_key: "…"
    # … or the seed phrase (12 to 24 English BIP-39 words). Not both.
    seed_phrase: "…"
    # Optional. Defaults to https://baibot.tron.mx.
    origin: null
```

| Key | Meaning |
|---|---|
| `private_key` | The wallet's private key. Exactly one of `private_key` and `seed_phrase`. |
| `seed_phrase` | The wallet's BIP-39 seed phrase. The key is taken at `m/44'/195'/0'/0/0`, the path the Chums wallet uses, so a seed phrase from the Chums client yields the same address. |
| `origin` | Reported to the homeserver when asking for a login challenge; the homeserver only issues challenges for the origins in its `tron_auth_allowed_origins` list. The default is the value the Chums homeserver lists for this bot. Set it only for a homeserver with a different list. |

Setting any `BAIBOT_USER_TRON_*` variable enables the mode even when the configuration file has no `user.tron` section. Keep the key out of `config.yml`; `.env.example` shows the variables.

**What the homeserver needs.** The bot's account must exist and the wallet must be bound to it (`GET /_ever/tron-auth/lookup?address=<the wallet address>` on the homeserver answers `{"bound": true, "user_id": "@<mxid_localpart>:<server_name>"}`). Bind it from the Chums client, logged in as the bot's account. The bot checks this before asking for a challenge and refuses to start when the wallet is unbound or bound to another user: the homeserver would otherwise log the bot in as that other user, or register a new account.

**Sessions.** The wallet is only used when the bot has no session yet (no `session.json` in the persistence directory). Afterwards the bot restores the saved session at start-up, as with the other modes. To make it log in again with a fresh device, stop the bot and delete the session file and the `db` directory; with `user.encryption.recovery_passphrase` set, the encryption keys are recovered (see below).

**Encryption keys.** With the wallet login the bot needs no `user.encryption.recovery_passphrase`: the passphrase of the account's secret storage is derived from the wallet key (the signature of `chums-ssss-v1:<user id>`, exactly what the Chums web client derives through TronLink), so the bot and the web client logged in with the same wallet open the same secret storage, and a bot whose `data/` was lost gets its keys back at the next start. A configured `recovery_passphrase` takes precedence over the derived one. See [Encryption keys and recovery](#encryption-keys-and-recovery).

**Errors at start-up.** `Origin is not allowed` means `user.tron.origin` is not in the homeserver's list. `Username is not available` means the wallet is not bound and the localpart is taken. Both stop the bot before it creates a session.

The mode is compiled in by default (cargo feature `tron-login`). A build made with `--no-default-features` refuses a `user.tron` section at start-up.


### Encryption keys and recovery

With `user.encryption.recovery_passphrase` (`BAIBOT_USER_ENCRYPTION_RECOVERY_PASSPHRASE`) set, the bot keeps its encryption keys (the cross-signing keys and the key of the server-side room key backup) in the account's secret storage under that passphrase, and imports them from there **at every start**, not only when it logs in. The account's secret storage is created at the first start if it does not exist. A bot whose `data/` directory was lost logs in again and reads the history of its encrypted rooms after the import. The log says what happened:

- `Recovery: secrets imported from secret storage` — the usual case;
- `Recovery: secret storage created` — the account had none;
- `Recovery: secret storage key reset` / `Recovery: secrets imported, key backup recreated` — only with `user.encryption.recovery_reset_allowed` (`BAIBOT_USER_ENCRYPTION_RECOVERY_RESET_ALLOWED`), see the comments in [`config.yml.dist`](../../etc/app/config.yml.dist).

A failure stops the bot with the reason and a hint (`Recovery failed: …`): the passphrase does not open the secret storage (it was created with another passphrase), the account has a key backup this device cannot use and no secret storage, or the backup on the server is not the one secret storage knows. Each of these is resolved by setting `recovery_reset_allowed` to `true` for one start, at the price of the history that this device has not received.

With the [TRON wallet login](#tron-wallet-authentication) and no configured passphrase, the passphrase is derived from the wallet key; the log then says `Recovery passphrase taken from the TRON wallet key`. A secret storage that the derived passphrase does not open was created with another passphrase or by another wallet.

Without a passphrase and without a wallet the keys stay on this device only, as in upstream baibot.
