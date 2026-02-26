//! Cron job execution: check due jobs, parse cron expressions, record runs.

use super::AgentRunConfig;
use crate::channels::OutgoingMessage;
use crate::db::{now_ms, Db, DbError};
use chrono::{TimeZone, Utc};
use cron::Schedule;
use std::str::FromStr;
use tokio::sync::mpsc;

/// Normalize a cron expression to the 6/7-field format the `cron` crate expects.
/// Standard 5-field (min hour dom month dow) gets "0 " prepended for seconds.
fn normalize_cron(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() == 5 {
        format!("0 {}", expr)
    } else {
        expr.to_string()
    }
}

/// Check all enabled cron jobs and run those that are due. Returns number of jobs executed.
pub async fn check_and_run_due_jobs(
    db: &Db,
    agent_config: &AgentRunConfig,
    delivery_tx: Option<&mpsc::UnboundedSender<OutgoingMessage>>,
) -> Result<usize, DbError> {
    let jobs = list_due_jobs(db).await?;
    let mut ran = 0;

    for job in jobs {
        tracing::info!(
            "Cron job '{}' is due, executing... (mode: {})",
            job.name,
            job.session_mode
        );

        let started_at = now_ms() as i64;
        let job_id = job.id;

        // Record the run as started
        let run_id = db
            .exec(move |conn| {
                conn.execute(
                    "INSERT INTO cron_runs (job_id, status, started_at) VALUES (?1, 'running', ?2)",
                    rusqlite::params![job_id, started_at],
                )?;
                let id = conn.last_insert_rowid();
                Ok(id)
            })
            .await?;

        // Execute based on session mode
        let session_id = format!("cron-{}", job.name);
        let system_prompt = "You are a scheduled task agent. Execute the following task concisely.";

        let result = match job.session_mode.as_str() {
            "persistent" => {
                super::run_persistent_prompt(
                    db,
                    agent_config,
                    &session_id,
                    system_prompt,
                    &job.prompt,
                )
                .await
            }
            _ => {
                if job.session_mode != "isolated" {
                    tracing::warn!(
                        "Cron job '{}' has unknown session_mode '{}'; using isolated",
                        job.name,
                        job.session_mode
                    );
                }
                super::run_ephemeral_prompt(agent_config, system_prompt, &job.prompt).await
            }
        };

        match result {
            Ok(response) => {
                tracing::info!(
                    "Cron job '{}' completed ({} chars)",
                    job.name,
                    response.len()
                );

                // Record successful run
                let finished_at = now_ms() as i64;
                let result_text = response.clone();
                db.exec(move |conn| {
                    conn.execute(
                        "UPDATE cron_runs SET status = 'ok', result = ?1, finished_at = ?2 WHERE id = ?3",
                        rusqlite::params![result_text, finished_at, run_id],
                    )?;
                    Ok(())
                })
                .await?;

                // Deliver to target channel if configured
                if let (Some(target), Some(tx)) = (&job.target_channel, delivery_tx) {
                    // target is a session_id like "tg-514133400" or "dc-guild-channel"
                    // Derive the adapter name from the prefix
                    let adapter_name = channel_from_session_id(target);
                    let _ = tx.send(OutgoingMessage {
                        channel: adapter_name.to_string(),
                        session_id: target.clone(),
                        content: response,
                        reply_to: None,
                    });
                }
            }
            Err(e) => {
                tracing::error!("Cron job '{}' failed: {}", job.name, e);

                // Record failed run
                let finished_at = now_ms() as i64;
                let err_msg = e.to_string();
                db.exec(move |conn| {
                    conn.execute(
                        "UPDATE cron_runs SET status = 'error', result = ?1, finished_at = ?2 WHERE id = ?3",
                        rusqlite::params![err_msg, finished_at, run_id],
                    )?;
                    Ok(())
                })
                .await?;
            }
        }

        // Update the job's updated_at to prevent re-running within the same tick
        let now = now_ms() as i64;
        let jid = job.id;
        db.exec(move |conn| {
            conn.execute(
                "UPDATE cron_jobs SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![now, jid],
            )?;
            Ok(())
        })
        .await?;

        ran += 1;
    }

    Ok(ran)
}

/// Derive the adapter/channel name from a session_id prefix.
/// e.g. "tg-514133400" → "telegram", "dc-guild-chan" → "discord", "slack-chan" → "slack"
fn channel_from_session_id(session_id: &str) -> &str {
    if session_id.starts_with("tg-") {
        "telegram"
    } else if session_id.starts_with("dc-") {
        "discord"
    } else if session_id.starts_with("slack-") {
        "slack"
    } else {
        // Fallback: use the session_id as-is (legacy behavior)
        session_id
    }
}

/// A loaded cron job from the database.
#[derive(Debug, Clone)]
pub struct CronJob {
    pub id: i64,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    pub target_channel: Option<String>,
    pub session_mode: String,
    pub enabled: bool,
}

/// List all enabled cron jobs that are due to run based on their schedule.
async fn list_due_jobs(db: &Db) -> Result<Vec<CronJob>, DbError> {
    db.exec(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, schedule, prompt, target_channel, session_mode, enabled, updated_at
             FROM cron_jobs WHERE enabled = 1",
        )?;

        let now = Utc::now();
        let mut due = Vec::new();

        let rows = stmt.query_map([], |row| {
            Ok((
                CronJob {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    schedule: row.get(2)?,
                    prompt: row.get(3)?,
                    target_channel: row.get(4)?,
                    session_mode: row
                        .get::<_, Option<String>>(5)?
                        .unwrap_or_else(|| "isolated".to_string()),
                    enabled: row.get::<_, i64>(6)? == 1,
                },
                row.get::<_, i64>(7)?, // updated_at
            ))
        })?;

        for row in rows {
            let (job, updated_at) = row?;

            // Parse cron expression (normalize 5-field to 6-field)
            let normalized = normalize_cron(&job.schedule);
            let schedule = match Schedule::from_str(&normalized) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "Invalid cron expression '{}' for job '{}': {}",
                        job.schedule,
                        job.name,
                        e
                    );
                    continue;
                }
            };

            // Find the last time this job should have run
            let last_update = Utc.timestamp_millis_opt(updated_at).single();
            let since = last_update.unwrap_or(now - chrono::Duration::hours(24));

            // Check if there's a scheduled time between last update and now
            if let Some(next) = schedule.after(&since).next() {
                if next <= now {
                    due.push(job);
                }
            }
        }

        Ok(due)
    })
    .await
}

/// Create a new cron job in the database. Returns the job ID.
pub async fn create_job(
    db: &Db,
    name: &str,
    schedule: &str,
    prompt: &str,
    target: Option<&str>,
    session: &str,
) -> Result<i64, DbError> {
    // Validate cron expression first (normalize 5-field to 6-field)
    let normalized = normalize_cron(schedule);
    Schedule::from_str(&normalized).map_err(|e| {
        DbError::Sqlite(rusqlite::Error::InvalidParameterName(format!(
            "Invalid cron expression: {}",
            e
        )))
    })?;

    let name = name.to_string();
    let schedule = schedule.to_string();
    let prompt = prompt.to_string();
    let target = target.map(|s| s.to_string());
    let session = session.to_string();

    db.exec(move |conn| {
        let ts = now_ms() as i64;
        conn.execute(
            "INSERT INTO cron_jobs (name, schedule, prompt, target_channel, session_mode, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(name) DO UPDATE SET
                schedule = excluded.schedule,
                prompt = excluded.prompt,
                target_channel = excluded.target_channel,
                session_mode = excluded.session_mode,
                updated_at = excluded.updated_at",
            rusqlite::params![name, schedule, prompt, target, session, ts],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    })
    .await
}

/// List all cron jobs (for display).
pub async fn list_jobs(db: &Db) -> Result<Vec<CronJob>, DbError> {
    db.exec(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, schedule, prompt, target_channel, session_mode, enabled FROM cron_jobs ORDER BY name",
        )?;

        let jobs = stmt
            .query_map([], |row| {
                Ok(CronJob {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    schedule: row.get(2)?,
                    prompt: row.get(3)?,
                    target_channel: row.get(4)?,
                    session_mode: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "isolated".to_string()),
                    enabled: row.get::<_, i64>(6)? == 1,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    })
    .await
}

/// Delete a cron job by name. Returns true if a job was deleted.
pub async fn delete_job(db: &Db, name: &str) -> Result<bool, DbError> {
    let name = name.to_string();
    db.exec(move |conn| {
        let deleted = conn.execute(
            "DELETE FROM cron_jobs WHERE name = ?1",
            rusqlite::params![name],
        )?;
        Ok(deleted > 0)
    })
    .await
}

/// Toggle a cron job's enabled state by name. Returns the new enabled state, or None if not found.
pub async fn toggle_job(db: &Db, name: &str, enabled: bool) -> Result<Option<bool>, DbError> {
    let name = name.to_string();
    let enabled_int: i64 = if enabled { 1 } else { 0 };
    db.exec(move |conn| {
        let updated = conn.execute(
            "UPDATE cron_jobs SET enabled = ?1, updated_at = ?2 WHERE name = ?3",
            rusqlite::params![enabled_int, now_ms() as i64, name],
        )?;
        if updated > 0 {
            Ok(Some(enabled))
        } else {
            Ok(None)
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test agent config that won't actually call any real provider.
    /// The check_and_run_due_jobs tests below will invoke the ephemeral agent,
    /// which will fail (no real API key), but we test the DB recording logic separately.
    fn test_agent_config() -> AgentRunConfig {
        AgentRunConfig {
            provider: "anthropic".to_string(),
            model: "mock".to_string(),
            api_key: "test-key".to_string(),
            context: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_create_and_list_jobs() {
        let db = Db::open_memory().unwrap();

        create_job(
            &db,
            "test-job",
            "0 9 * * *",
            "Do something",
            Some("telegram"),
            "isolated",
        )
        .await
        .unwrap();
        create_job(
            &db,
            "another-job",
            "0 18 * * 1-5",
            "Evening summary",
            None,
            "main",
        )
        .await
        .unwrap();

        let jobs = list_jobs(&db).await.unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "another-job"); // sorted by name
        assert_eq!(jobs[1].name, "test-job");
    }

    #[tokio::test]
    async fn test_create_job_invalid_cron() {
        let db = Db::open_memory().unwrap();
        let result = create_job(&db, "bad", "not a cron", "test", None, "isolated").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_job() {
        let db = Db::open_memory().unwrap();
        create_job(&db, "to-delete", "0 9 * * *", "test", None, "isolated")
            .await
            .unwrap();

        let deleted = delete_job(&db, "to-delete").await.unwrap();
        assert!(deleted);

        let deleted_again = delete_job(&db, "to-delete").await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_toggle_job() {
        let db = Db::open_memory().unwrap();
        create_job(&db, "toggleable", "0 9 * * *", "test", None, "isolated")
            .await
            .unwrap();

        let state = toggle_job(&db, "toggleable", false).await.unwrap();
        assert_eq!(state, Some(false));

        let state = toggle_job(&db, "toggleable", true).await.unwrap();
        assert_eq!(state, Some(true));

        let state = toggle_job(&db, "nonexistent", false).await.unwrap();
        assert_eq!(state, None);
    }

    #[tokio::test]
    async fn test_check_due_jobs_none_due() {
        let db = Db::open_memory().unwrap();
        let agent = test_agent_config();

        // Create a job that just ran (updated_at = now)
        create_job(&db, "recent", "0 9 * * *", "test", None, "isolated")
            .await
            .unwrap();

        // No jobs should be due since the job was just created (updated_at = now)
        let ran = check_and_run_due_jobs(&db, &agent, None).await.unwrap();
        assert_eq!(ran, 0);
    }

    #[tokio::test]
    async fn test_check_due_jobs_with_old_job() {
        let db = Db::open_memory().unwrap();
        let agent = test_agent_config();

        // Create a job, then backdate its updated_at to 25 hours ago
        create_job(
            &db,
            "overdue",
            "* * * * *",
            "every minute",
            None,
            "isolated",
        )
        .await
        .unwrap();

        let old_ts = (now_ms() - 25 * 60 * 60 * 1000) as i64;
        db.exec(move |conn| {
            conn.execute(
                "UPDATE cron_jobs SET updated_at = ?1 WHERE name = 'overdue'",
                rusqlite::params![old_ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // This will try to run the ephemeral agent with a fake API key,
        // so the agent call will fail. But the run should still be recorded as error.
        let ran = check_and_run_due_jobs(&db, &agent, None).await.unwrap();
        assert_eq!(ran, 1);

        // Verify a run was recorded (either ok or error)
        let run_count = db
            .exec(|conn| {
                let c: i64 = conn.query_row("SELECT COUNT(*) FROM cron_runs", [], |r| r.get(0))?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(run_count, 1);
    }

    #[tokio::test]
    async fn test_persistent_mode_dispatch() {
        let db = Db::open_memory().unwrap();
        let agent = test_agent_config();

        // Create a persistent-mode job
        create_job(
            &db,
            "persistent-job",
            "* * * * *",
            "check status",
            None,
            "persistent",
        )
        .await
        .unwrap();

        // Backdate so it's due
        let old_ts = (now_ms() - 25 * 60 * 60 * 1000) as i64;
        db.exec(move |conn| {
            conn.execute(
                "UPDATE cron_jobs SET updated_at = ?1 WHERE name = 'persistent-job'",
                rusqlite::params![old_ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Will fail at provider level (fake API key), but should record run attempt
        let ran = check_and_run_due_jobs(&db, &agent, None).await.unwrap();
        assert_eq!(ran, 1);

        // Verify run was recorded
        let run_count = db
            .exec(|conn| {
                let c: i64 = conn.query_row("SELECT COUNT(*) FROM cron_runs", [], |r| r.get(0))?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(run_count, 1);
    }

    #[tokio::test]
    async fn test_unknown_session_mode_falls_back() {
        let db = Db::open_memory().unwrap();
        let agent = test_agent_config();

        // Create a job with unknown session mode
        create_job(&db, "weird-mode", "* * * * *", "test", None, "unknown_mode")
            .await
            .unwrap();

        // Backdate so it's due
        let old_ts = (now_ms() - 25 * 60 * 60 * 1000) as i64;
        db.exec(move |conn| {
            conn.execute(
                "UPDATE cron_jobs SET updated_at = ?1 WHERE name = 'weird-mode'",
                rusqlite::params![old_ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Should run (falls back to isolated) without panic
        let ran = check_and_run_due_jobs(&db, &agent, None).await.unwrap();
        assert_eq!(ran, 1);
    }

    #[test]
    fn test_channel_from_session_id() {
        assert_eq!(channel_from_session_id("tg-514133400"), "telegram");
        assert_eq!(channel_from_session_id("dc-guild-channel"), "discord");
        assert_eq!(channel_from_session_id("slack-general"), "slack");
        assert_eq!(channel_from_session_id("unknown-id"), "unknown-id");
    }
}
