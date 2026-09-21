//! Recovery of the bot's encryption secrets from the account's secret storage, run at every
//! start.
//!
//! The secrets (the private cross-signing keys and the decryption key of the server-side room
//! key backup) live in the account's secret storage (SSSS) under the recovery passphrase. mxlink
//! only recovers them when it logs in, that is when there is no saved session; a restart with a
//! saved session never looks at secret storage again. This module does it every time, so that
//! a device whose crypto store is gone, or whose secrets changed on the server, is brought back
//! in line with the account without operator intervention.
//!
//! See `docs/configuration/authentication.md` (Sessions).

use std::fmt;

use mxlink::matrix_sdk::Client;
use mxlink::matrix_sdk::encryption::recovery::RecoveryError as SdkRecoveryError;
use mxlink::matrix_sdk::encryption::secret_storage::SecretStorageError;

/// What the recovery step did. `Display` gives the line the bot logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The secrets were imported from the existing secret storage.
    Imported,
    /// The account had no secret storage; one was created and the secrets exported into it.
    Created,
    /// The passphrase did not open the secret storage; a new secret storage key was made from
    /// it and the secrets re-exported (only with `recovery_reset_allowed`).
    KeyReset,
    /// The secrets were imported, and the server-side key backup was replaced because its key
    /// was not the one in secret storage (only with `recovery_reset_allowed`).
    BackupRecreated,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Imported => "secrets imported from secret storage",
            Self::Created => "secret storage created",
            Self::KeyReset => "secret storage key reset",
            Self::BackupRecreated => "secrets imported, key backup recreated",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("the recovery passphrase does not open the account's secret storage")]
    SecretMismatch,

    #[error(
        "the account has a key backup but no secret storage, and the key of that backup is not on this device"
    )]
    BackupWithoutSecretStorage,

    #[error("the key backup on the server is not the one secret storage holds the key of")]
    InconsistentBackup,

    #[error("creating the secret storage failed: {0}")]
    Setup(SdkRecoveryError),

    #[error("resetting the secret storage key failed: {0}")]
    Reset(SdkRecoveryError),

    #[error("recreating the key backup failed: {0}")]
    FixBackup(SdkRecoveryError),

    #[error("{0}")]
    Other(SdkRecoveryError),
}

impl RecoveryError {
    /// What the operator can do about it, when there is a known way out.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::SecretMismatch => Some(
                "the secret storage was created with another passphrase. To replace it with one made from this passphrase, set user.encryption.recovery_reset_allowed (BAIBOT_USER_ENCRYPTION_RECOVERY_RESET_ALLOWED) to true; the history of encrypted rooms that this device has not received stays inaccessible",
            ),
            Self::BackupWithoutSecretStorage => Some(
                "nobody can read that backup. To replace it with a new one, set user.encryption.recovery_reset_allowed (BAIBOT_USER_ENCRYPTION_RECOVERY_RESET_ALLOWED) to true",
            ),
            Self::InconsistentBackup => Some(
                "another device replaced the backup. To replace it again with one this device can use, set user.encryption.recovery_reset_allowed (BAIBOT_USER_ENCRYPTION_RECOVERY_RESET_ALLOWED) to true; the room keys in the current backup are then only kept by the devices that have them",
            ),
            Self::Setup(_) | Self::Reset(_) | Self::FixBackup(_) | Self::Other(_) => None,
        }
    }
}

/// Why the plain recovery did not go through, from the SDK's error.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The account has no secret storage (no default key in account data).
    NoSecretStorage,
    /// The passphrase does not match the secret storage key.
    WrongKey,
    /// The secrets were readable, but the backup key in them does not fit the server's backup.
    InconsistentBackup,
    Other,
}

fn classify(err: &SdkRecoveryError) -> Failure {
    match err {
        SdkRecoveryError::SecretStorage(SecretStorageError::MissingKeyInfo { .. }) => {
            Failure::NoSecretStorage
        }
        SdkRecoveryError::SecretStorage(SecretStorageError::SecretStorageKey(_)) => {
            Failure::WrongKey
        }
        SdkRecoveryError::SecretStorage(
            SecretStorageError::InconsistentBackupDecryptionKey
            | SecretStorageError::MissingOrInvalidBackupDecryptionKey,
        ) => Failure::InconsistentBackup,
        _ => Failure::Other,
    }
}

/// Brings the device in line with the account's secret storage, opening it with `passphrase`.
///
/// - Secret storage exists and opens: the secrets are imported.
/// - No secret storage yet: one is created with `passphrase` and the secrets exported. If the
///   account already has a key backup this device cannot use, it is replaced only with
///   `reset_allowed`.
/// - The passphrase does not open it: with `reset_allowed` the secret storage key is reset from
///   the passphrase, otherwise this is an error.
/// - The backup key in secret storage does not fit the server's backup: with `reset_allowed`
///   the backup is recreated, otherwise this is an error.
///
/// Waits for the SDK's own encryption set-up first, so that the two do not race.
pub async fn ensure(
    client: &Client,
    passphrase: &str,
    reset_allowed: bool,
) -> Result<Outcome, RecoveryError> {
    let encryption = client.encryption();
    encryption.wait_for_e2ee_initialization_tasks().await;

    let recovery = encryption.recovery();

    let Err(err) = recovery.recover(passphrase).await else {
        return Ok(Outcome::Imported);
    };

    match classify(&err) {
        Failure::NoSecretStorage => {
            tracing::warn!("The account has no secret storage; creating one");

            create(client, passphrase, reset_allowed).await
        }
        Failure::WrongKey => {
            if !reset_allowed {
                return Err(RecoveryError::SecretMismatch);
            }

            tracing::warn!(
                "The recovery passphrase does not open the secret storage; resetting its key"
            );

            recovery
                .reset_key()
                .with_passphrase(passphrase)
                .await
                .map_err(RecoveryError::Reset)?;

            Ok(Outcome::KeyReset)
        }
        Failure::InconsistentBackup => {
            if !reset_allowed {
                return Err(RecoveryError::InconsistentBackup);
            }

            tracing::warn!(
                "The key backup on the server does not match secret storage; recreating it"
            );

            recovery
                .recover_and_fix_backup(passphrase)
                .await
                .map_err(RecoveryError::FixBackup)?;

            Ok(Outcome::BackupRecreated)
        }
        Failure::Other => Err(RecoveryError::Other(err)),
    }
}

async fn create(
    client: &Client,
    passphrase: &str,
    reset_allowed: bool,
) -> Result<Outcome, RecoveryError> {
    let recovery = client.encryption().recovery();

    // The recovery key the SDK returns is not needed: the passphrase opens the store.
    let result = recovery
        .enable()
        .wait_for_backups_to_upload()
        .with_passphrase(passphrase)
        .await
        .map(|_recovery_key| Outcome::Created);

    match result {
        Err(SdkRecoveryError::BackupExistsOnServer) => {
            if !reset_allowed {
                return Err(RecoveryError::BackupWithoutSecretStorage);
            }

            tracing::warn!(
                "The account has a key backup this device cannot use and no secret storage; replacing the backup"
            );

            client
                .encryption()
                .backups()
                .disable_and_delete()
                .await
                .map_err(|err| RecoveryError::Setup(err.into()))?;

            recovery
                .enable()
                .wait_for_backups_to_upload()
                .with_passphrase(passphrase)
                .await
                .map(|_recovery_key| Outcome::Created)
                .map_err(RecoveryError::Setup)
        }
        other => other.map_err(RecoveryError::Setup),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_default_key_means_no_secret_storage() {
        let err =
            SdkRecoveryError::SecretStorage(SecretStorageError::MissingKeyInfo { key_id: None });
        assert_eq!(classify(&err), Failure::NoSecretStorage);

        let err = SdkRecoveryError::SecretStorage(SecretStorageError::MissingKeyInfo {
            key_id: Some("abc".to_owned()),
        });
        assert_eq!(classify(&err), Failure::NoSecretStorage);
    }

    #[test]
    fn backup_key_problems_are_inconsistent_backups() {
        for err in [
            SecretStorageError::InconsistentBackupDecryptionKey,
            SecretStorageError::MissingOrInvalidBackupDecryptionKey,
        ] {
            assert_eq!(
                classify(&SdkRecoveryError::SecretStorage(err)),
                Failure::InconsistentBackup
            );
        }
    }

    #[test]
    fn anything_else_is_other() {
        assert_eq!(
            classify(&SdkRecoveryError::BackupExistsOnServer),
            Failure::Other
        );
    }

    #[test]
    fn outcomes_read_as_log_lines() {
        assert_eq!(
            Outcome::Imported.to_string(),
            "secrets imported from secret storage"
        );
        assert_eq!(Outcome::Created.to_string(), "secret storage created");
    }

    #[test]
    fn operator_errors_have_hints_and_sdk_errors_do_not() {
        assert!(RecoveryError::SecretMismatch.hint().is_some());
        assert!(RecoveryError::BackupWithoutSecretStorage.hint().is_some());
        assert!(RecoveryError::InconsistentBackup.hint().is_some());
        assert!(
            RecoveryError::Other(SdkRecoveryError::BackupExistsOnServer)
                .hint()
                .is_none()
        );
    }
}
