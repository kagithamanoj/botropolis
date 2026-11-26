//! Tenancy: who a request belongs to, and where their state lives.
//!
//! A tenant is a team. Every tenant gets isolated state under
//! `botropolis/data/tenants/<tenant_id>/`: its own memory database,
//! ledger, run state, notebook, and usage log. The default tenant,
//! `"default"`, keeps the old single-tenant paths, so local dev
//! behaves exactly as before.
//!
//! A request names its tenant with an `X-Botropolis-Key` header
//! carrying a team API key, or a `botropolis_session` cookie from
//! signin. Keys and session tokens are matched by SHA-256 hash
//! against the platform database; the full secret never lives in
//! the database and never appears in a log or error message.
//!
//! In hosted mode (`BOTROPOLIS_HOSTED=1`) a request with no tenant
//! credential is rejected with 401. In single-tenant mode (the
//! default, for local dev) it falls back to the default tenant.

use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// The tenant id used when the server runs in single-tenant mode.
pub const DEFAULT_TENANT: &str = "default";

/// Header carrying a team API key.
pub const TENANT_KEY_HEADER: &str = "x-botropolis-key";

/// Cookie carrying a session token from signin.
pub const SESSION_COOKIE: &str = "botropolis_session";

/// Hosted mode is on when `BOTROPOLIS_HOSTED` is `1` or `true`.
pub fn hosted_mode() -> bool {
    match std::env::var("BOTROPOLIS_HOSTED") {
        Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        Err(_) => false,
    }
}

/// Keep a tenant id safe to use as one path segment: letters,
/// digits, `-` and `_` survive, everything else becomes `_`.
/// An empty result falls back to the default tenant.
pub fn sanitize_tenant_id(tenant_id: &str) -> String {
    let clean: String = tenant_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if clean.is_empty() {
        DEFAULT_TENANT.to_string()
    } else {
        clean
    }
}

/// SHA-256 hex of a secret. Only hashes are stored or compared.
pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

thread_local! {
    static CURRENT_TENANT: RefCell<String> = RefCell::new(DEFAULT_TENANT.to_string());
}

/// The tenant id for the work running on this thread.
pub fn current_tenant() -> String {
    CURRENT_TENANT.with(|cell| cell.borrow().clone())
}

/// Run `f` with this thread's tenant set, restoring it after.
pub fn with_tenant<R>(tenant_id: &str, f: impl FnOnce() -> R) -> R {
    let previous = CURRENT_TENANT.with(|cell| cell.replace(tenant_id.to_string()));
    let out = f();
    CURRENT_TENANT.with(|cell| cell.replace(previous));
    out
}

/// Who a request belongs to, and how it proved it.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantContext {
    pub tenant_id: String,
    /// True when the request carried a valid key or session.
    pub authenticated: bool,
}

impl TenantContext {
    pub fn default_tenant() -> Self {
        TenantContext {
            tenant_id: DEFAULT_TENANT.to_string(),
            authenticated: false,
        }
    }

    pub fn team(team_id: &str) -> Self {
        TenantContext {
            tenant_id: team_id.to_string(),
            authenticated: true,
        }
    }
}

/// Look up the team that owns an API key hash in the platform
/// database. None when the database or the key is missing, so a
/// fresh install without teams simply has no tenants yet.
pub fn lookup_team_by_key(db_path: &Path, key_hash: &str) -> Option<String> {
    if !db_path.is_file() {
        return None;
    }
    let conn = rusqlite::Connection::open(db_path).ok()?;
    let team_id: String = conn
        .query_row(
            "SELECT team_id FROM api_keys WHERE key_hash = ?1 AND revoked_at IS NULL",
            rusqlite::params![key_hash],
            |row| row.get(0),
        )
        .ok()?;
    Some(team_id)
}

/// Look up the team that owns a session token hash. None when the
/// database, the session, or the team is missing, or the session
/// has expired.
pub fn lookup_team_by_session(db_path: &Path, token_hash: &str) -> Option<String> {
    if !db_path.is_file() {
        return None;
    }
    let conn = rusqlite::Connection::open(db_path).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let team_id: String = conn
        .query_row(
            "SELECT team_id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
            rusqlite::params![token_hash, now],
            |row| row.get(0),
        )
        .ok()?;
    Some(team_id)
}

/// Pull one cookie value out of a raw `Cookie` header.
pub fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            if key.trim() == name {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Resolve a request to a tenant.
///
/// `api_key` is the `X-Botropolis-Key` header value, if any;
/// `cookie_header` is the raw `Cookie` header, if any. A key that
/// is present but unknown is rejected in both modes: a wrong
/// credential fails closed instead of silently falling back.
/// With no credential at all, hosted mode rejects the request and
/// single-tenant mode uses the default tenant.
pub fn resolve_tenant(
    api_key: Option<&str>,
    cookie_header: Option<&str>,
    platform_db: &Path,
    hosted: bool,
) -> Result<TenantContext, String> {
    if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
        let hash = hash_secret(key);
        return match lookup_team_by_key(platform_db, &hash) {
            Some(team_id) => Ok(TenantContext::team(&team_id)),
            None => Err("unknown or revoked team key".to_string()),
        };
    }
    if let Some(header) = cookie_header {
        if let Some(token) = cookie_value(header, SESSION_COOKIE) {
            if !token.is_empty() {
                let hash = hash_secret(&token);
                return match lookup_team_by_session(platform_db, &hash) {
                    Some(team_id) => Ok(TenantContext::team(&team_id)),
                    None => Err("unknown or expired session".to_string()),
                };
            }
        }
    }
    if hosted {
        return Err("a team key or session is required".to_string());
    }
    Ok(TenantContext::default_tenant())
}

/// Where the platform database lives. `BOTROPOLIS_PLATFORM_DB`
/// moves it; otherwise `botropolis/data/platform.db`.
pub fn platform_db_path() -> PathBuf {
    crate::paths::platform_db_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_env;

    fn make_platform_db(dir: &Path) -> PathBuf {
        let path = dir.join("platform.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE api_keys (id TEXT PRIMARY KEY, team_id TEXT NOT NULL, key_hash TEXT NOT NULL, created_at REAL NOT NULL, revoked_at REAL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY, team_id TEXT NOT NULL, token_hash TEXT NOT NULL, created_at REAL NOT NULL, expires_at REAL NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO api_keys (id, team_id, key_hash, created_at, revoked_at) VALUES (?1, ?2, ?3, 0, NULL)",
            rusqlite::params!["key-1", "team-a", hash_secret("secret-key-a")],
        )
        .unwrap();
        let far_future = 9_999_999_999.0;
        conn.execute(
            "INSERT INTO sessions (id, team_id, token_hash, created_at, expires_at) VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params!["sess-1", "team-b", hash_secret("token-b"), far_future],
        )
        .unwrap();
        path
    }

    #[test]
    fn sanitize_keeps_ids_to_one_path_segment() {
        assert_eq!(sanitize_tenant_id("team-a_1"), "team-a_1");
        assert_eq!(sanitize_tenant_id("../../etc"), "______etc");
        assert_eq!(sanitize_tenant_id(""), DEFAULT_TENANT);
        assert_eq!(sanitize_tenant_id("a/b"), "a_b");
    }

    #[test]
    fn resolve_key_header_to_team() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_platform_db(dir.path());
        let ctx = resolve_tenant(Some("secret-key-a"), None, &db, false).unwrap();
        assert_eq!(ctx.tenant_id, "team-a");
        assert!(ctx.authenticated);
    }

    #[test]
    fn unknown_key_fails_closed_in_both_modes() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_platform_db(dir.path());
        assert!(resolve_tenant(Some("nope"), None, &db, false).is_err());
        assert!(resolve_tenant(Some("nope"), None, &db, true).is_err());
    }

    #[test]
    fn session_cookie_resolves_to_team() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_platform_db(dir.path());
        let ctx =
            resolve_tenant(None, Some("other=1; botropolis_session=token-b"), &db, true).unwrap();
        assert_eq!(ctx.tenant_id, "team-b");
    }

    #[test]
    fn keyless_request_depends_on_mode() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_platform_db(dir.path());
        let single = resolve_tenant(None, None, &db, false).unwrap();
        assert_eq!(single.tenant_id, DEFAULT_TENANT);
        assert!(!single.authenticated);
        assert!(resolve_tenant(None, None, &db, true).is_err());
    }

    #[test]
    fn missing_platform_db_means_no_tenant_for_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("no-such.db");
        assert!(resolve_tenant(Some("secret-key-a"), None, &db, false).is_err());
        let ctx = resolve_tenant(None, None, &db, false).unwrap();
        assert_eq!(ctx.tenant_id, DEFAULT_TENANT);
    }

    #[test]
    fn with_tenant_scopes_and_restores() {
        let _guard = lock_env();
        assert_eq!(current_tenant(), DEFAULT_TENANT);
        with_tenant("team-a", || {
            assert_eq!(current_tenant(), "team-a");
        });
        assert_eq!(current_tenant(), DEFAULT_TENANT);
    }
}
