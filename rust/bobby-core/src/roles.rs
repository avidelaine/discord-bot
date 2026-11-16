//! Who may ask Bobby what (port of `bot/roles.js`). Levels: other < prime < admin < super (`Role` order).
//! Ids and the Prime role name come from the environment (the same `.env` as Node, see `load_dotenv`);
//! changes made by voice live in the store's "permissions" document.
use crate::store::Store;
use crate::types::Role;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}
fn ids(v: &str) -> Vec<String> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub fn supers() -> Vec<String> {
    ids(&env("SUPER_ADMINS", "261531632589078528")) // WakandeX
}
fn admins() -> Vec<String> {
    ids(&env("ADMINS", "397478613559148545")) // iisal_ (super admins are admins too)
}
/// Discord role name or id.
pub fn prime_role() -> String {
    env("PRIME_ROLE", "Team Prime")
}

/// blacklist: falls to "other" whatever their Discord role; admins / notAdmins: admin granted or removed by voice.
/// Key order and unknown keys are kept as Node writes them (`{ blacklist, admins, notAdmins, ...saved }`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Perms {
    #[serde(default)]
    pub blacklist: Vec<String>,
    #[serde(default)]
    pub admins: Vec<String>,
    #[serde(default, rename = "notAdmins")]
    pub not_admins: Vec<String>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

pub fn perms(store: &Store) -> Perms {
    store.load("permissions").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn save_perms(store: &Store, p: &Perms) -> Result<(), String> {
    store.save("permissions", &serde_json::to_value(p).unwrap())
}

/// `roleOf(member)`: `discord_roles` holds the member's role names and ids.
pub fn role_of(id: &str, discord_roles: &[String], store: &Store) -> Role {
    if is_super(id) {
        return Role::Super;
    }
    let p = perms(store);
    let has = |l: &[String]| l.iter().any(|s| s == id);
    if has(&p.blacklist) {
        return Role::Other;
    }
    if (has(&admins()) || has(&p.admins)) && !has(&p.not_admins) {
        return Role::Admin;
    }
    let role = prime_role();
    if discord_roles.iter().any(|r| *r == role) {
        return Role::Prime;
    }
    Role::Other
}

pub fn allowed(role: Role, need: Role) -> bool {
    role >= need
}

pub fn is_super(id: &str) -> bool {
    supers().iter().any(|s| s == id)
}

/// People with each role right now, for "who can do what" (ids; the Prime role is a Discord role, not a list).
#[derive(Debug, Clone, PartialEq)]
pub struct People {
    pub supers: Vec<String>,
    pub admins: Vec<String>,
    pub prime_role: String,
    pub blacklist: Vec<String>,
}

pub fn people(store: &Store) -> People {
    let p = perms(store);
    let sup = supers();
    let mut all = admins();
    for a in &p.admins {
        if !all.contains(a) {
            all.push(a.clone());
        }
    }
    let admins = all.into_iter().filter(|id| !sup.contains(id) && !p.not_admins.contains(id) && !p.blacklist.contains(id)).collect();
    People { blacklist: p.blacklist.iter().filter(|id| !sup.contains(id)).cloned().collect(), supers: sup, admins, prime_role: prime_role() }
}

/// Loads `KEY=VALUE` lines of a `.env` file into the environment, never overriding a variable already set
/// (dotenv's rule). Call once at startup, before any thread starts.
pub fn load_dotenv(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim().trim_matches(|c| c == '"' || c == '\''));
            if !k.is_empty() && std::env::var_os(k).is_none() {
                std::env::set_var(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const WAKANDEX: &str = "261531632589078528";
    const IISAL: &str = "397478613559148545";

    #[test]
    fn roles_from_defaults_and_store() {
        let s = Store::temp();
        let r = |id: &str, roles: &[&str]| role_of(id, &roles.iter().map(|x| x.to_string()).collect::<Vec<_>>(), &s);
        assert_eq!(r(WAKANDEX, &[]), Role::Super);
        assert_eq!(r(IISAL, &[]), Role::Admin);
        assert_eq!(r("42", &["Team Prime"]), Role::Prime);
        assert_eq!(r("43", &["Membre"]), Role::Other);
        assert!(allowed(Role::Prime, Role::Prime) && !allowed(Role::Prime, Role::Admin) && !allowed(Role::Admin, Role::Super));
        save_perms(&s, &Perms { blacklist: vec![IISAL.into()], ..Default::default() }).unwrap();
        assert_eq!(r(IISAL, &["Team Prime"]), Role::Other);
        assert_eq!(std::fs::read_to_string(s.json_dir().join("permissions.json")).unwrap(), "{\n  \"blacklist\": [\n    \"397478613559148545\"\n  ],\n  \"admins\": [],\n  \"notAdmins\": []\n}");
        assert!(people(&s).admins.is_empty());
    }
}
