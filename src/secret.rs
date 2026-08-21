
use crate::config::Config;

pub const SERVICE: &str = "coinfetch";

pub const ACCOUNT: &str = "coingecko-api-key";

pub trait KeyStore {

    fn load(&self) -> Result<Option<String>, String>;

    fn store(&self, key: &str) -> Result<(), String>;

    fn clear(&self) -> Result<(), String>;
}

pub struct Keyring;

impl Keyring {
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(describe)
    }
}

impl KeyStore for Keyring {
    fn load(&self) -> Result<Option<String>, String> {
        match self.entry()?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(describe(err)),
        }
    }

    fn store(&self, key: &str) -> Result<(), String> {
        self.entry()?.set_password(key).map_err(describe)
    }

    fn clear(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(describe(err)),
        }
    }
}

fn describe(err: keyring::Error) -> String {
    match err {
        keyring::Error::NoDefaultStore => "no credential store on this system \
             (on Linux that means no running Secret Service such as GNOME Keyring or KWallet)"
            .to_string(),
        other => other.to_string(),
    }
}

pub fn resolve_api_key(cfg: &mut Config, keys: &dyn KeyStore) -> Vec<String> {
    let mut warnings = Vec::new();

    let from_file = cfg.api_key().map(str::to_string);

    match keys.load() {
        Ok(Some(stored)) => {
            if from_file.is_some() {
                warnings.push(format!(
                    "`coingecko_api_key` is still in config.toml in plain text; the key from \
                     the {SERVICE} keyring is the one in use, and the next save drops the field"
                ));
            }
            cfg.coingecko_api_key = Some(stored);
            cfg.normalize_api_key();
        }

        Ok(None) => {
            if let Some(key) = &from_file {
                match keys.store(key) {
                    Ok(()) => warnings.push(
                        "moved `coingecko_api_key` from config.toml into the system keyring — \
                         the plaintext copy stays in the file until the next save removes it"
                            .to_string(),
                    ),

                    Err(err) => warnings.push(format!(
                        "could not move the API key into the system keyring ({err}); \
                         it stays in config.toml for now"
                    )),
                }
            }
        }

        Err(err) => {
            if from_file.is_some() {
                warnings.push(format!(
                    "system keyring unavailable ({err}); \
                     falling back to the `coingecko_api_key` still in config.toml"
                ));
            }
        }
    }

    warnings
}

#[cfg(test)]
pub struct MemoryStore {
    slot: std::cell::RefCell<Option<String>>,

    broken: Option<String>,
}

#[cfg(test)]
impl MemoryStore {
    pub fn empty() -> Self {
        MemoryStore {
            slot: std::cell::RefCell::new(None),
            broken: None,
        }
    }

    pub fn holding(key: &str) -> Self {
        MemoryStore {
            slot: std::cell::RefCell::new(Some(key.to_string())),
            broken: None,
        }
    }

    pub fn broken() -> Self {
        MemoryStore {
            slot: std::cell::RefCell::new(None),
            broken: Some("no credential store on this system".to_string()),
        }
    }

    pub fn peek(&self) -> Option<String> {
        self.slot.borrow().clone()
    }
}

#[cfg(test)]
impl KeyStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, String> {
        match &self.broken {
            Some(err) => Err(err.clone()),
            None => Ok(self.slot.borrow().clone()),
        }
    }

    fn store(&self, key: &str) -> Result<(), String> {
        match &self.broken {
            Some(err) => Err(err.clone()),
            None => {
                *self.slot.borrow_mut() = Some(key.to_string());
                Ok(())
            }
        }
    }

    fn clear(&self) -> Result<(), String> {
        match &self.broken {
            Some(err) => Err(err.clone()),
            None => {
                *self.slot.borrow_mut() = None;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_key(key: &str) -> Config {
        Config {
            coingecko_api_key: Some(key.to_string()),
            ..Config::default()
        }
    }

    #[test]
    fn a_key_written_to_the_store_reads_back_and_can_be_removed_again() {
        let keys = MemoryStore::empty();
        assert_eq!(
            keys.load(),
            Ok(None),
            "an empty store has no key, not an error"
        );

        keys.store("CG-abc123").expect("store");
        assert_eq!(keys.load(), Ok(Some("CG-abc123".to_string())));

        keys.store("CG-def456").expect("replace");
        assert_eq!(keys.load(), Ok(Some("CG-def456".to_string())));

        keys.clear().expect("clear");
        assert_eq!(keys.load(), Ok(None));
    }

    #[test]
    fn clearing_a_store_that_holds_nothing_is_not_a_failure() {

        let keys = MemoryStore::empty();
        assert_eq!(keys.clear(), Ok(()));
    }

    #[test]
    fn a_plaintext_key_in_the_config_moves_into_the_store() {
        let keys = MemoryStore::empty();
        let mut cfg = config_with_key("CG-abc123");

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(keys.peek(), Some("CG-abc123".to_string()), "not migrated");

        assert_eq!(cfg.api_key(), Some("CG-abc123"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("keyring"), "{warnings:?}");
    }

    #[test]
    fn the_migrated_key_leaves_the_file_on_the_next_save() {

        let keys = MemoryStore::empty();
        let mut cfg = config_with_key("CG-abc123");
        resolve_api_key(&mut cfg, &keys);

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(!text.contains("CG-abc123"), "{text}");
        assert!(!text.contains("coingecko_api_key"), "{text}");
    }

    #[test]
    fn a_key_already_in_the_store_is_never_migrated_over() {

        let keys = MemoryStore::holding("CG-current");
        let mut cfg = config_with_key("CG-stale");

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(cfg.api_key(), Some("CG-current"));
        assert_eq!(keys.peek(), Some("CG-current".to_string()));
        assert!(
            warnings.iter().any(|w| w.contains("plain text")),
            "the ignored plaintext copy has to be mentioned: {warnings:?}"
        );
    }

    #[test]
    fn a_stored_key_is_picked_up_without_the_config_mentioning_one() {

        let keys = MemoryStore::holding("CG-abc123");
        let mut cfg = Config::default();

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(cfg.api_key(), Some("CG-abc123"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn no_key_anywhere_is_the_quiet_free_tier_case() {
        let keys = MemoryStore::empty();
        let mut cfg = Config::default();

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(cfg.api_key(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_broken_store_leaves_the_tool_running_without_a_key() {

        let keys = MemoryStore::broken();
        let mut cfg = Config::default();

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(cfg.api_key(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_broken_store_keeps_using_the_key_still_in_the_config() {

        let keys = MemoryStore::broken();
        let mut cfg = config_with_key("CG-abc123");

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(cfg.api_key(), Some("CG-abc123"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("unavailable"), "{warnings:?}");
    }

    #[test]
    fn a_store_that_refuses_the_write_keeps_the_key_in_the_config() {

        struct WriteOnlyFails;
        impl KeyStore for WriteOnlyFails {
            fn load(&self) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn store(&self, _: &str) -> Result<(), String> {
                Err("the store is locked".to_string())
            }
            fn clear(&self) -> Result<(), String> {
                Ok(())
            }
        }

        let mut cfg = config_with_key("CG-abc123");
        let warnings = resolve_api_key(&mut cfg, &WriteOnlyFails);

        assert_eq!(cfg.api_key(), Some("CG-abc123"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("the store is locked"), "{warnings:?}");
    }

    #[test]
    fn a_blank_key_in_an_old_config_is_not_worth_migrating() {

        let keys = MemoryStore::empty();
        let mut cfg = config_with_key("   ");

        let warnings = resolve_api_key(&mut cfg, &keys);

        assert_eq!(keys.peek(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
