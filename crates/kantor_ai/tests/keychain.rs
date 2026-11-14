//! The real system keychain (Secret Service on Linux). Ignored by default:
//! it needs an unlocked keychain on the session bus. Run it in a private
//! bus with a throwaway gnome-keyring, as `scripts/keychain-test.sh` does:
//!
//! ```text
//! KANTOR_TEST_KEYCHAIN=1 cargo test -p kantor_ai --test keychain -- --ignored
//! ```

use kantor_ai::keys::{CLOUD, KeyLocation};

#[test]
#[ignore]
fn cloud_vault_save_read_remove_in_the_keychain() {
    if std::env::var_os("KANTOR_TEST_KEYCHAIN").is_none() {
        eprintln!("skipped: KANTOR_TEST_KEYCHAIN not set");
        return;
    }
    // The script points XDG_CONFIG_HOME at a throwaway folder, so a
    // fallback file (if one were written) stays out of the user's config.
    let id = "keychain-test.example-budi";
    let secret = "app-pass-123 456";
    assert_eq!(CLOUD.save(id, secret), Ok(KeyLocation::Keychain));
    assert_eq!(CLOUD.load(id).as_deref(), Some(secret));
    // Not written to the fallback file.
    let fallback = CLOUD.fallback_path().unwrap();
    assert!(
        !fallback.exists(),
        "{} should not exist",
        fallback.display()
    );
    // Visible to other Secret Service clients under Kantor's service name.
    let out = std::process::Command::new("secret-tool")
        .args(["lookup", "service", "Kantor Cloud", "username", id])
        .output();
    match out {
        Ok(out) if !out.stdout.is_empty() => {
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), secret)
        }
        _ => eprintln!("secret-tool found nothing under service/username (other attribute names)"),
    }
    CLOUD.delete(id);
    assert_eq!(CLOUD.load(id), None);
}
