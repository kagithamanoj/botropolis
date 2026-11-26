//! Team accounts and API keys for hosted Botropolis.
//!
//! One SQLite database (`botropolis/data/platform.db`, gitignored)
//! holds the teams, their API keys, and their sessions:
//!
//! ```text
//! teams     id, name, email (unique), password_hash, plan, created_at
//! api_keys  id, team_id, key_hash, name, created_at, last_used_at, revoked_at
//! sessions  id, team_id, token_hash, created_at, expires_at
//! ```
//!
//! Passwords are hashed with argon2id; API keys and session tokens
//! are stored only as SHA-256 hashes, and the full key is shown
//! exactly once, at creation. No secret ever appears in a log, a
//! trace, or an error message, and no GET endpoint returns one.

use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tenancy::hash_secret;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    plan TEXT NOT NULL DEFAULT 'free',
    created_at REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    created_at REAL NOT NULL,
    last_used_at REAL,
    revoked_at REAL
);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys (key_hash);
CREATE INDEX IF NOT EXISTS idx_sessions_hash ON sessions (token_hash);
";

/// Session lifetime: 30 days.
pub const SESSION_TTL_SECS: f64 = 30.0 * 24.0 * 3600.0;

/// Minimum password length.
pub const MIN_PASSWORD_LEN: usize = 8;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", random_hex(8))
}

/// One team. Carries no password hash: nothing that leaves this
/// module can print one.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub email: String,
    pub plan: String,
    pub created_at: f64,
}

/// One API key's public metadata. Never carries the key itself.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub created_at: f64,
    pub last_used_at: Option<f64>,
    pub revoked_at: Option<f64>,
}

/// The platform database: teams, API keys, sessions.
#[derive(Debug, Clone)]
pub struct PlatformDb {
    conn: Arc<Mutex<Connection>>,
    path: String,
}

impl PlatformDb {
    /// Open (creating when needed) the platform database at a path.
    pub fn open(path: impl AsRef<Path>) -> Result<PlatformDb, String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(PlatformDb {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_string_lossy().to_string(),
        })
    }

    /// Open the default platform database.
    pub fn open_default() -> Result<PlatformDb, String> {
        Self::open(crate::paths::platform_db_path())
    }

    /// Where this database lives.
    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    }

    fn check_password(password: &str, hash: &str) -> bool {
        let parsed = match PasswordHash::new(hash) {
            Ok(parsed) => parsed,
            Err(_) => return false,
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    /// Register a team. Returns the team (without any secret).
    pub fn create_team(&self, name: &str, email: &str, password: &str) -> Result<Team, String> {
        let name = name.trim();
        let email = email.trim().to_lowercase();
        if name.is_empty() {
            return Err("team name must not be empty".to_string());
        }
        if !email.contains('@') || email.len() < 3 {
            return Err("email must be a valid email address".to_string());
        }
        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(format!(
                "password must be at least {MIN_PASSWORD_LEN} characters"
            ));
        }
        let team = Team {
            id: new_id("team"),
            name: name.to_string(),
            email: email.clone(),
            plan: "free".to_string(),
            created_at: now_secs(),
        };
        let hash = Self::hash_password(password)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let inserted = conn.execute(
            "INSERT INTO teams (id, name, email, password_hash, plan, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![team.id, team.name, team.email, hash, team.plan, team.created_at],
        );
        match inserted {
            Ok(_) => Ok(team),
            Err(e) => {
                let message = e.to_string();
                if message.contains("UNIQUE") {
                    Err("email already registered".to_string())
                } else {
                    Err(message)
                }
            }
        }
    }

    /// Look a team up by id.
    pub fn team_by_id(&self, team_id: &str) -> Option<Team> {
        let conn = self.conn.lock().ok()?;
        conn.query_row(
            "SELECT id, name, email, plan, created_at FROM teams WHERE id = ?1",
            params![team_id],
            |row| {
                Ok(Team {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    email: row.get(2)?,
                    plan: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .ok()
    }

    /// Check an email and password. The error message is the same
    /// for an unknown email and a wrong password.
    pub fn verify_signin(&self, email: &str, password: &str) -> Result<Team, String> {
        let email = email.trim().to_lowercase();
        let found: Option<(Team, String)> = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT id, name, email, plan, created_at, password_hash FROM teams WHERE email = ?1",
                params![email],
                |row| {
                    Ok((
                        Team {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            email: row.get(2)?,
                            plan: row.get(3)?,
                            created_at: row.get(4)?,
                        },
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .ok()
        };
        match found {
            Some((team, hash)) if Self::check_password(password, &hash) => Ok(team),
            _ => Err("invalid email or password".to_string()),
        }
    }

    /// Change a team's plan (for example `free` to another plan).
    /// Returns false when the team does not exist.
    pub fn set_plan(&self, team_id: &str, plan: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE teams SET plan = ?1 WHERE id = ?2",
                params![plan, team_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }

    /// Create a session for a team. Returns the token; only its
    /// hash is stored.
    pub fn create_session(&self, team_id: &str) -> Result<String, String> {
        let token = random_hex(32);
        let now = now_secs();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sessions (id, team_id, token_hash, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                new_id("sess"),
                team_id,
                hash_secret(&token),
                now,
                now + SESSION_TTL_SECS
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(token)
    }

    /// The team behind a session token, when the session exists and
    /// has not expired.
    pub fn team_from_session(&self, token: &str) -> Option<Team> {
        let team_id =
            crate::tenancy::lookup_team_by_session(Path::new(&self.path), &hash_secret(token))?;
        self.team_by_id(&team_id)
    }

    /// Create an API key for a team. Returns the metadata and the
    /// full key, which is shown once and never stored.
    pub fn create_api_key(
        &self,
        team_id: &str,
        name: &str,
    ) -> Result<(ApiKeyMeta, String), String> {
        let key = format!("btp_{}", random_hex(32));
        let meta = ApiKeyMeta {
            id: new_id("key"),
            name: name.trim().to_string(),
            created_at: now_secs(),
            last_used_at: None,
            revoked_at: None,
        };
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO api_keys (id, team_id, key_hash, name, created_at, last_used_at, revoked_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL)",
            params![meta.id, team_id, hash_secret(&key), meta.name, meta.created_at],
        )
        .map_err(|e| e.to_string())?;
        Ok((meta, key))
    }

    /// The team behind an API key, when the key exists and is not
    /// revoked. A successful lookup updates `last_used_at`.
    pub fn team_from_key(&self, key: &str) -> Option<Team> {
        let team_id = crate::tenancy::lookup_team_by_key(Path::new(&self.path), &hash_secret(key))?;
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE api_keys SET last_used_at = ?1 WHERE key_hash = ?2",
                params![now_secs(), hash_secret(key)],
            );
        }
        self.team_by_id(&team_id)
    }

    /// A team's keys, metadata only, newest first.
    pub fn list_api_keys(&self, team_id: &str) -> Vec<ApiKeyMeta> {
        let conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT id, name, created_at, last_used_at, revoked_at FROM api_keys WHERE team_id = ?1 ORDER BY created_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(params![team_id], |row| {
            Ok(ApiKeyMeta {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                last_used_at: row.get(3)?,
                revoked_at: row.get(4)?,
            })
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// Revoke one of a team's keys. Returns false when the key does
    /// not belong to the team or is already revoked.
    pub fn revoke_api_key(&self, team_id: &str, key_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE api_keys SET revoked_at = ?1 WHERE id = ?2 AND team_id = ?3 AND revoked_at IS NULL",
                params![now_secs(), key_id, team_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (tempfile::TempDir, PlatformDb) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("platform.db");
        (dir, PlatformDb::open(path).unwrap())
    }

    #[test]
    fn signup_and_signin_roundtrip() {
        let (_dir, db) = test_db();
        let team = db
            .create_team("Acme", "acme@example.com", "correct horse")
            .unwrap();
        assert_eq!(team.plan, "free");
        let signed_in = db
            .verify_signin("acme@example.com", "correct horse")
            .unwrap();
        assert_eq!(signed_in.id, team.id);
        assert!(db
            .verify_signin("acme@example.com", "wrong password")
            .is_err());
        assert!(db
            .verify_signin("nobody@example.com", "correct horse")
            .is_err());
        assert_eq!(
            db.verify_signin("nobody@example.com", "correct horse")
                .unwrap_err(),
            db.verify_signin("acme@example.com", "wrong password")
                .unwrap_err()
        );
    }

    #[test]
    fn duplicate_email_is_rejected() {
        let (_dir, db) = test_db();
        db.create_team("Acme", "acme@example.com", "correct horse")
            .unwrap();
        let err = db
            .create_team("Acme 2", "ACME@example.com", "another password")
            .unwrap_err();
        assert_eq!(err, "email already registered");
    }

    #[test]
    fn short_password_and_bad_email_are_rejected() {
        let (_dir, db) = test_db();
        assert!(db.create_team("Acme", "acme@example.com", "short").is_err());
        assert!(db
            .create_team("Acme", "not-an-email", "correct horse")
            .is_err());
        assert!(db
            .create_team("", "acme@example.com", "correct horse")
            .is_err());
    }

    #[test]
    fn key_create_use_and_revoke() {
        let (_dir, db) = test_db();
        let team = db
            .create_team("Acme", "acme@example.com", "correct horse")
            .unwrap();
        let (meta, key) = db.create_api_key(&team.id, "ci").unwrap();
        assert!(key.starts_with("btp_"));
        assert!(db.team_from_key(&key).is_some());
        let listed = db.list_api_keys(&team.id);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, meta.id);
        assert!(listed[0].last_used_at.is_some());
        assert!(db.revoke_api_key(&team.id, &meta.id).unwrap());
        assert!(db.team_from_key(&key).is_none());
        assert!(!db.revoke_api_key(&team.id, &meta.id).unwrap());
    }

    #[test]
    fn session_roundtrip_and_wrong_token() {
        let (_dir, db) = test_db();
        let team = db
            .create_team("Acme", "acme@example.com", "correct horse")
            .unwrap();
        let token = db.create_session(&team.id).unwrap();
        let found = db.team_from_session(&token).unwrap();
        assert_eq!(found.id, team.id);
        assert!(db.team_from_session("no-such-token").is_none());
    }

    #[test]
    fn secrets_never_appear_in_the_database_file() {
        let (_dir, db) = test_db();
        let team = db
            .create_team("Acme", "acme@example.com", "correct horse battery")
            .unwrap();
        let (_, key) = db.create_api_key(&team.id, "ci").unwrap();
        let token = db.create_session(&team.id).unwrap();
        let bytes = std::fs::read(db.path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("correct horse battery"));
        assert!(!text.contains(&key));
        assert!(!text.contains(&token));
        assert!(text.contains("$argon2"));
    }
}
