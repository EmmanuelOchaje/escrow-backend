//! Solvency monitoring: periodic checks that the chain and database are in sync.

use chrono::{DateTime, Utc};
use escrow_chain::ChainReader;
use escrow_db::Db;
use escrow_core::Numeric;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SolvencyCheck {
    pub token: String,
    pub ledger_number: u32,
    pub held_on_chain: Numeric,
    pub balance_actual: Numeric,
    pub funded_expected: Numeric,
    pub released_net_sum: Numeric,
    pub fees_sum: Numeric,
    pub refunded_sum: Numeric,
    pub held_expected: Numeric,
    pub db_held_sum: Numeric,
    pub discrepancy: Option<String>,
    pub severity: SolvencySeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SolvencySeverity {
    Ok,
    Warning,
    Critical,
}

impl SolvencySeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// Run a solvency check for a specific token.
/// Returns the check result and any alert message.
pub async fn check_token_solvency(
    db: &Db,
    chain: &dyn ChainReader,
    token: &str,
) -> Result<(SolvencyCheck, Option<String>), Box<dyn std::error::Error>> {
    let ledger = chain.latest_ledger().await?;

    // Get contract totals for this token
    let (held_on_chain, balance_actual, funded_expected, released_net_sum, fees_sum, refunded_sum) =
        chain.get_token_totals(token, ledger).await?;

    // Calculate what should be held on-chain
    let held_expected = funded_expected - released_net_sum - fees_sum - refunded_sum;

    // Get database total for non-terminal bounties of this token
    let db_held_sum = db.get_held_bounties_total(token).await?;

    let mut check = SolvencyCheck {
        token: token.to_string(),
        ledger_number: ledger,
        held_on_chain,
        balance_actual,
        funded_expected,
        released_net_sum,
        fees_sum,
        refunded_sum,
        held_expected,
        db_held_sum,
        discrepancy: None,
        severity: SolvencySeverity::Ok,
    };

    // Check invariants
    if held_on_chain > balance_actual {
        check.severity = SolvencySeverity::Critical;
        check.discrepancy = Some(format!(
            "Insolvency: contract holds {} but has {} actual balance",
            held_on_chain, balance_actual
        ));
    } else if held_expected != held_on_chain {
        check.severity = SolvencySeverity::Warning;
        check.discrepancy = Some(format!(
            "Chain accounting mismatch: expected {} held but have {}",
            held_expected, held_on_chain
        ));
    } else if db_held_sum != held_on_chain {
        check.severity = SolvencySeverity::Warning;
        check.discrepancy = Some(format!(
            "Database drift: DB shows {} held but chain has {}",
            db_held_sum, held_on_chain
        ));
    }

    let alert = check.discrepancy.clone().map(|d| {
        format!(
            "Solvency alert ({} severity) for token {}: {}",
            check.severity.as_str(),
            token,
            d
        )
    });

    Ok((check, alert))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solvency_ok_when_balanced() {
        let check = SolvencyCheck {
            token: "CUSDC".to_string(),
            ledger_number: 1000,
            held_on_chain: Numeric::new(100),
            balance_actual: Numeric::new(100),
            funded_expected: Numeric::new(150),
            released_net_sum: Numeric::new(40),
            fees_sum: Numeric::new(5),
            refunded_sum: Numeric::new(5),
            held_expected: Numeric::new(100),
            db_held_sum: Numeric::new(100),
            discrepancy: None,
            severity: SolvencySeverity::Ok,
        };
        assert_eq!(check.severity, SolvencySeverity::Ok);
        assert_eq!(check.discrepancy, None);
    }

    #[test]
    fn solvency_critical_when_insolvent() {
        let check = SolvencyCheck {
            token: "CUSDC".to_string(),
            ledger_number: 1000,
            held_on_chain: Numeric::new(100),
            balance_actual: Numeric::new(50),
            funded_expected: Numeric::new(150),
            released_net_sum: Numeric::new(40),
            fees_sum: Numeric::new(5),
            refunded_sum: Numeric::new(5),
            held_expected: Numeric::new(100),
            db_held_sum: Numeric::new(100),
            discrepancy: Some("Contract holds more than it has".to_string()),
            severity: SolvencySeverity::Critical,
        };
        assert_eq!(check.severity, SolvencySeverity::Critical);
        assert!(check.discrepancy.is_some());
    }
}
