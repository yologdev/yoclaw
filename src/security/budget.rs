use crate::db::Db;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Tracks token usage with atomic counters for sync callback compatibility.
#[derive(Clone)]
pub struct BudgetTracker {
    max_tokens_per_day: Option<u64>,
    max_turns_per_session: Option<usize>,
    tokens_today: Arc<AtomicU64>,
    turns_this_session: Arc<AtomicU64>,
    db: Db,
}

impl BudgetTracker {
    pub fn new(
        max_tokens_per_day: Option<u64>,
        max_turns_per_session: Option<usize>,
        db: Db,
    ) -> Self {
        Self {
            max_tokens_per_day,
            max_turns_per_session,
            tokens_today: Arc::new(AtomicU64::new(0)),
            turns_this_session: Arc::new(AtomicU64::new(0)),
            db,
        }
    }

    /// Load today's token usage from the audit table.
    pub async fn load_from_db(&self) -> Result<(), crate::db::DbError> {
        let usage = self.db.audit_token_usage_today().await?;
        self.tokens_today.store(usage, Ordering::Relaxed);
        tracing::info!("Loaded today's token usage: {}", usage);
        Ok(())
    }

    /// Record token usage. Returns true if within budget.
    pub fn record_usage(&self, input: u64, output: u64) -> bool {
        let total = input + output;
        let prev = self.tokens_today.fetch_add(total, Ordering::Relaxed);
        if let Some(max) = self.max_tokens_per_day {
            if prev + total > max {
                tracing::warn!("Token budget exceeded: {} + {} > {}", prev, total, max);
                return false;
            }
        }
        true
    }

    /// Record a turn. Returns true if within budget.
    pub fn record_turn(&self) -> bool {
        let prev = self.turns_this_session.fetch_add(1, Ordering::Relaxed);
        if let Some(max) = self.max_turns_per_session {
            if prev + 1 > max as u64 {
                tracing::warn!("Turn limit exceeded: {} > {}", prev + 1, max);
                return false;
            }
        }
        true
    }

    /// Check if budget allows another turn (without recording).
    pub fn can_continue(&self) -> bool {
        if let Some(max) = self.max_tokens_per_day {
            if self.tokens_today.load(Ordering::Relaxed) >= max {
                return false;
            }
        }
        if let Some(max) = self.max_turns_per_session {
            if self.turns_this_session.load(Ordering::Relaxed) >= max as u64 {
                return false;
            }
        }
        true
    }

    /// Reset turn counter (for new sessions).
    pub fn reset_turns(&self) {
        self.turns_this_session.store(0, Ordering::Relaxed);
    }

    /// Get current token usage.
    pub fn tokens_used_today(&self) -> u64 {
        self.tokens_today.load(Ordering::Relaxed)
    }

    /// Get current turn count.
    pub fn turns_used(&self) -> u64 {
        self.turns_this_session.load(Ordering::Relaxed)
    }

    /// Update budget limits at runtime (for hot-reload).
    pub fn update_limits(&mut self, max_tokens: Option<u64>, max_turns: Option<usize>) {
        self.max_tokens_per_day = max_tokens;
        self.max_turns_per_session = max_turns;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_budget_within_limits() {
        let db = Db::open_memory().unwrap();
        let tracker = BudgetTracker::new(Some(10000), Some(5), db);

        assert!(tracker.can_continue());
        assert!(tracker.record_usage(100, 50));
        assert!(tracker.record_turn());
        assert_eq!(tracker.tokens_used_today(), 150);
        assert_eq!(tracker.turns_used(), 1);
    }

    #[tokio::test]
    async fn test_token_budget_exceeded() {
        let db = Db::open_memory().unwrap();
        let tracker = BudgetTracker::new(Some(100), None, db);

        assert!(tracker.record_usage(60, 30)); // 90, within budget
        assert!(!tracker.record_usage(20, 10)); // 120, exceeds 100
    }

    #[tokio::test]
    async fn test_turn_limit_exceeded() {
        let db = Db::open_memory().unwrap();
        let tracker = BudgetTracker::new(None, Some(2), db);

        assert!(tracker.record_turn()); // 1
        assert!(tracker.record_turn()); // 2
        assert!(!tracker.record_turn()); // 3 > 2
    }

    #[tokio::test]
    async fn test_no_limits() {
        let db = Db::open_memory().unwrap();
        let tracker = BudgetTracker::new(None, None, db);

        assert!(tracker.can_continue());
        assert!(tracker.record_usage(999999, 999999));
        assert!(tracker.record_turn());
        assert!(tracker.can_continue());
    }

    #[tokio::test]
    async fn test_reset_turns() {
        let db = Db::open_memory().unwrap();
        let tracker = BudgetTracker::new(None, Some(1), db);

        tracker.record_turn();
        assert!(!tracker.can_continue());
        tracker.reset_turns();
        assert!(tracker.can_continue());
    }
}
