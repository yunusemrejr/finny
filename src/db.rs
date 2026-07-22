use anyhow::Result;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: i64,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: Option<i64>,
    pub ts: DateTime<Utc>,
    pub role: String,
    pub text: String,
    pub session_id: i64,
    pub intent: Option<String>,
    pub query_plan: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: Option<i64>,
    pub url: String,
    pub retrieved_at: DateTime<Utc>,
    pub domain_tier: u8,
    pub purpose: String,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now())
}

#[derive(Clone)]
pub struct Db {
    path: std::path::PathBuf,
}

impl Db {
    pub fn open_default() -> Result<Self> {
        let base = std::env::var_os("FINNY_DATA_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("./data"));
        std::fs::create_dir_all(&base)?;
        let path = base.join("finny.db");
        Ok(Self { path })
    }

    pub fn init(&self) -> Result<()> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sessions(
                id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL,
                created_at TEXT NOT NULL, is_archived INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, role TEXT NOT NULL,
                text TEXT NOT NULL, session_id INTEGER NOT NULL, intent TEXT, query_plan TEXT,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
             );
             CREATE TABLE IF NOT EXISTS sources(
                id INTEGER PRIMARY KEY AUTOINCREMENT, url TEXT NOT NULL,
                retrieved_at TEXT NOT NULL, domain_tier INTEGER, purpose TEXT
             );",
        )?;
        c.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('db_version', '0.1.0')",
            [],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, include_archived: bool) -> Result<Vec<Session>> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let sql = if include_archived {
            "SELECT id, title, created_at, is_archived FROM sessions ORDER BY created_at DESC"
        } else {
            "SELECT id, title, created_at, is_archived FROM sessions WHERE is_archived = 0 ORDER BY created_at DESC"
        };
        let mut stmt = c.prepare(sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Session {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at: parse_dt(&row.get::<_, String>(2)?),
                    is_archived: row.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            let s = self.create_session("Default session")?;
            return Ok(vec![s]);
        }
        Ok(rows)
    }

    pub fn create_session(&self, title: &str) -> Result<Session> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        c.execute(
            "INSERT INTO sessions(title, created_at, is_archived) VALUES(?1, ?2, 0)",
            [title, &Utc::now().to_rfc3339()],
        )?;
        Ok(Session {
            id: c.last_insert_rowid(),
            title: title.to_string(),
            created_at: Utc::now(),
            is_archived: false,
        })
    }

    pub fn ensure_session(&self, id: Option<i64>) -> Result<i64> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        if let Some(id) = id {
            let n: i64 =
                c.query_row("SELECT COUNT(*) FROM sessions WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
            if n > 0 {
                return Ok(id);
            }
        }
        c.execute(
            "INSERT INTO sessions(title, created_at, is_archived) VALUES(?1, ?2, 0)",
            ["Default session", &Utc::now().to_rfc3339()],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn add_message(&self, msg: &Message) -> Result<i64> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let ts: String = msg.ts.to_rfc3339();
        let intent: String = msg.intent.clone().unwrap_or_default();
        let qp: String = msg.query_plan.clone().unwrap_or_default();
        let sid: i64 = msg.session_id;
        c.execute(
            "INSERT INTO messages(ts, role, text, session_id, intent, query_plan) VALUES(?1,?2,?3,?4,?5,?6)",
            rusqlite::params![&ts, &msg.role, &msg.text, sid, &intent, &qp],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn messages_for(&self, sid: i64) -> Result<Vec<Message>> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let mut stmt = c.prepare(
            "SELECT id, ts, role, text, session_id, intent, query_plan FROM messages WHERE session_id = ?1 ORDER BY ts, id",
        )?;
        let rows = stmt
            .query_map([sid], |row| {
                Ok(Message {
                    id: Some(row.get(0)?),
                    ts: parse_dt(&row.get::<_, String>(1)?),
                    role: row.get(2)?,
                    text: row.get(3)?,
                    session_id: row.get(4)?,
                    intent: row.get::<_, Option<String>>(5)?.filter(|s| !s.is_empty()),
                    query_plan: row.get::<_, Option<String>>(6)?.filter(|s| !s.is_empty()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn add_source(&self, src: &Source) -> Result<()> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let ts: String = src.retrieved_at.to_rfc3339();
        let dt: i32 = src.domain_tier as i32;
        c.execute(
            "INSERT INTO sources(url, retrieved_at, domain_tier, purpose) VALUES(?1,?2,?3,?4)",
            rusqlite::params![&src.url, &ts, dt, &src.purpose],
        )?;
        Ok(())
    }

    pub fn archive_session(&self, id: Option<i64>) -> Result<()> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        if let Some(id) = id {
            c.execute("UPDATE sessions SET is_archived = 1 WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    /// Wipe all user-generated data: every chat message, every cached live
    /// source, and every session. The schema (tables) is preserved and at least
    /// one fresh session is guaranteed to exist afterwards (via `list_sessions`).
    /// This is the "Clear data" action — it never deletes the database file.
    pub fn clear_all_data(&self) -> Result<()> {
        let c = rusqlite::Connection::open(&self.path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        c.execute_batch(
            "DELETE FROM messages; DELETE FROM sources; DELETE FROM sessions;",
        )?;
        Ok(())
    }
}
