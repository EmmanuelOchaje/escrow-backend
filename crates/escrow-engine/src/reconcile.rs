//! Event indexer gap detection and state reconciliation from contract.

use chrono::{DateTime, Utc};
use escrow_chain::ChainReader;
use escrow_db::Db;
use serde::Serialize;

/// A gap in the event stream, usually due to RPC retention limits.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EventGap {
    pub gap_start: u32,
    pub gap_end: u32,
    pub detected_at: DateTime<Utc>,
}

impl EventGap {
    pub fn size(&self) -> u32 {
        self.gap_end.saturating_sub(self.gap_start)
    }
}

/// Detect if there's a gap between our cursor and the RPC's oldest retained ledger.
pub async fn detect_event_gap(
    db: &Db,
    chain: &dyn ChainReader,
) -> Result<Option<EventGap>, Box<dyn std::error::Error>> {
    // Get our current cursor
    let cursor_opt = db.get_indexer_cursor().await?;
    let Some(cursor) = cursor_opt else {
        return Ok(None);
    };

    // Get RPC retention info
    let health = chain.get_health().await?;
    let oldest_ledger = health.oldest_ledger;

    // If we're before the oldest retained, there's a gap
    if cursor.ledger < oldest_ledger {
        let gap = EventGap {
            gap_start: cursor.ledger,
            gap_end: oldest_ledger,
            detected_at: Utc::now(),
        };
        return Ok(Some(gap));
    }

    Ok(None)
}

/// A change to a bounty made during reconciliation
#[derive(Debug, Clone, Serialize)]
pub struct BountyReconciliation {
    pub bounty_id: i64,
    pub before_status: String,
    pub after_status: String,
    pub changes: Vec<String>,
}

/// Reconcile bounty state from the contract.
/// This reads get_bounty(id) for each non-terminal bounty and updates the database.
pub async fn reconcile_bounties(
    db: &Db,
    chain: &dyn ChainReader,
    dry_run: bool,
) -> Result<Vec<BountyReconciliation>, Box<dyn std::error::Error>> {
    let mut changes = vec![];

    // Get all non-terminal bounties
    let non_terminal = db.get_non_terminal_bounties().await?;

    for bounty in non_terminal {
        // Read from contract
        if let Ok(contract_state) = chain.get_bounty(bounty.id).await {
            let mut bounty_changes = vec![];

            // Compare and prepare updates
            if bounty.status != contract_state.status {
                bounty_changes.push(format!("status: {} -> {}", bounty.status, contract_state.status));
            }
            if bounty.winner_address != contract_state.winner_address {
                bounty_changes.push(format!(
                    "winner: {:?} -> {:?}",
                    bounty.winner_address, contract_state.winner_address
                ));
            }

            if !bounty_changes.is_empty() && !dry_run {
                db.update_bounty_from_chain(&bounty.id, &contract_state).await?;
            }

            if !bounty_changes.is_empty() {
                changes.push(BountyReconciliation {
                    bounty_id: bounty.id,
                    before_status: bounty.status.clone(),
                    after_status: contract_state.status.clone(),
                    changes: bounty_changes,
                });
            }
        }
    }

    Ok(changes)
}

/// Record a reconciliation audit entry
pub async fn record_reconciliation_audit(
    db: &Db,
    changes: &[BountyReconciliation],
    actor: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for change in changes {
        let data = serde_json::json!({
            "bounty_id": change.bounty_id,
            "before_status": change.before_status,
            "after_status": change.after_status,
            "changes": change.changes,
        });
        db.create_audit_entry(actor, "reconcile_bounty", &format!("bounty:{}", change.bounty_id), &data).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_gap_size_calculation() {
        let gap = EventGap {
            gap_start: 100,
            gap_end: 150,
            detected_at: Utc::now(),
        };
        assert_eq!(gap.size(), 50);
    }

    #[test]
    fn event_gap_zero_size_when_equal() {
        let gap = EventGap {
            gap_start: 100,
            gap_end: 100,
            detected_at: Utc::now(),
        };
        assert_eq!(gap.size(), 0);
    }
}
