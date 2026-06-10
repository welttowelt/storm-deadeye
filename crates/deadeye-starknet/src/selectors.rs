//! Pre-computed entry-point selectors for every AMM, Factory, and Oracle
//! function the SDK calls.
//!
//! Selectors are derived from `starknet_keccak(name)` truncated to 250 bits.
//! Each accessor memoises the result via [`std::sync::LazyLock`] so the
//! keccak is computed exactly once per process.

use starknet_core::{types::Felt, utils::get_selector_from_name};

fn compute(name: &'static str) -> Felt {
    get_selector_from_name(name).expect("entry-point name must be a valid Cairo identifier")
}

/// Selectors used by the AMM contracts.
pub mod amm {
    use std::sync::LazyLock;

    use starknet_core::types::Felt;

    /// Selector for the `get_params` entry-point.
    pub fn get_params() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_params"));
        *V
    }
    /// Selector for the `get_config` entry-point.
    pub fn get_config() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_config"));
        *V
    }
    /// Selector for the `get_distribution` entry-point.
    pub fn get_distribution() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_distribution"));
        *V
    }
    /// Selector for the `get_market_status` entry-point.
    pub fn get_market_status() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_market_status"));
        *V
    }
    /// Selector for the `get_position_summary` entry-point.
    pub fn get_position_summary() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_position_summary"));
        *V
    }
    /// Selector for the `get_position_compact` entry-point.
    pub fn get_position_compact() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_position_compact"));
        *V
    }
    /// Selector for the `get_trader_trade_lot_count` entry-point.
    pub fn get_trader_trade_lot_count() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_trader_trade_lot_count"));
        *V
    }
    /// Selector for the `get_trader_trade_lot_id` entry-point.
    pub fn get_trader_trade_lot_id() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_trader_trade_lot_id"));
        *V
    }
    /// Selector for the `get_trade_lot_settled` entry-point.
    pub fn get_trade_lot_settled() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_trade_lot_settled"));
        *V
    }
    /// Selector for the `get_trade_lot_cancelled` entry-point.
    pub fn get_trade_lot_cancelled() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_trade_lot_cancelled"));
        *V
    }
    /// Selector for the `get_trade_lot_value_at` entry-point.
    pub fn get_trade_lot_value_at() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_trade_lot_value_at"));
        *V
    }
    /// Selector for the `settle_trade_lots` entry-point (batch lot settlement).
    pub fn settle_trade_lots() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_trade_lots"));
        *V
    }
    /// Selector for the `batch_claim_for` entry-point.
    pub fn batch_claim_for() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("batch_claim_for"));
        *V
    }
    /// Selector for the `get_lp_info` entry-point.
    pub fn get_lp_info() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_lp_info"));
        *V
    }
    /// Selector for the `get_fee_config` entry-point.
    pub fn get_fee_config() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_fee_config"));
        *V
    }
    /// Selector for the `get_distribution_hints` entry-point.
    pub fn get_distribution_hints() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_distribution_hints"));
        *V
    }
    /// Selector for the `get_runtime_class_hash` entry-point.
    pub fn get_runtime_class_hash() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_runtime_class_hash"));
        *V
    }
    /// Selector for the `execute_trade` entry-point.
    pub fn execute_trade() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("execute_trade"));
        *V
    }
    /// Selector for the `sell_position_guarded` entry-point.
    pub fn sell_position_guarded() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("sell_position_guarded"));
        *V
    }
    /// Selector for the `check_sell_position` entry-point.
    pub fn check_sell_position() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("check_sell_position"));
        *V
    }
    /// Selector for the `claim` entry-point.
    pub fn claim() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("claim"));
        *V
    }
    /// Selector for the `claim_for` entry-point.
    pub fn claim_for() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("claim_for"));
        *V
    }
    /// Selector for the `add_liquidity` entry-point.
    pub fn add_liquidity() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("add_liquidity"));
        *V
    }
    /// Selector for the `remove_liquidity` entry-point.
    pub fn remove_liquidity() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("remove_liquidity"));
        *V
    }
}

/// Selectors used exclusively by multinoulli AMMs.
pub mod multinoulli {
    use std::sync::LazyLock;

    use starknet_core::types::Felt;

    /// Selector for `execute_trade_sparse`.
    pub fn execute_trade_sparse() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("execute_trade_sparse"));
        *V
    }
    /// Selector for `execute_trade_transfers`.
    pub fn execute_trade_transfers() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("execute_trade_transfers"));
        *V
    }
    /// Selector for `sell_position_guarded_sparse`.
    pub fn sell_position_guarded_sparse() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("sell_position_guarded_sparse"));
        *V
    }
    /// Selector for `settle_multi`.
    pub fn settle_multi() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_multi"));
        *V
    }
    /// Selector for `get_distribution_snapshot_count`.
    pub fn get_distribution_snapshot_count() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("get_distribution_snapshot_count"));
        *V
    }
    /// Selector for `get_settlement_outcomes`.
    pub fn get_settlement_outcomes() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_settlement_outcomes"));
        *V
    }
    /// Selector for `get_matrix_constraints`.
    pub fn get_matrix_constraints() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_matrix_constraints"));
        *V
    }
    /// Selector for `get_distribution_hint`.
    pub fn get_distribution_hint() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_distribution_hint"));
        *V
    }
}

/// Selectors used by the distribution factory.
pub mod factory {
    use std::sync::LazyLock;

    use starknet_core::types::Felt;

    /// Selector for `deploy_normal_market_from_profile`.
    pub fn deploy_normal_market_from_profile() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("deploy_normal_market_from_profile"));
        *V
    }
    /// Selector for `deploy_lognormal_market_from_profile`.
    pub fn deploy_lognormal_market_from_profile() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("deploy_lognormal_market_from_profile"));
        *V
    }
    /// Selector for `deploy_multinoulli_market_from_profile`.
    pub fn deploy_multinoulli_market_from_profile() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("deploy_multinoulli_market_from_profile"));
        *V
    }
    /// Selector for `deploy_bivariate_normal_market_from_profile`.
    pub fn deploy_bivariate_normal_market_from_profile() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("deploy_bivariate_normal_market_from_profile"));
        *V
    }
    /// Selector for `settle_normal_markets_best_effort`.
    pub fn settle_normal_markets_best_effort() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_normal_markets_best_effort"));
        *V
    }
    /// Selector for `settle_normal_markets_strict`.
    pub fn settle_normal_markets_strict() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_normal_markets_strict"));
        *V
    }
    /// Selector for `settle_lognormal_markets_best_effort`.
    pub fn settle_lognormal_markets_best_effort() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_lognormal_markets_best_effort"));
        *V
    }
    /// Selector for `settle_lognormal_markets_strict`.
    pub fn settle_lognormal_markets_strict() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_lognormal_markets_strict"));
        *V
    }
    /// Selector for `settle_bivariate_normal_markets_best_effort`.
    pub fn settle_bivariate_normal_markets_best_effort() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_bivariate_normal_markets_best_effort"));
        *V
    }
    /// Selector for `settle_bivariate_normal_markets_strict`.
    pub fn settle_bivariate_normal_markets_strict() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_bivariate_normal_markets_strict"));
        *V
    }
    /// Selector for `settle_multinoulli_markets_best_effort`.
    pub fn settle_multinoulli_markets_best_effort() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_multinoulli_markets_best_effort"));
        *V
    }
    /// Selector for `settle_multinoulli_markets_strict`.
    pub fn settle_multinoulli_markets_strict() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_multinoulli_markets_strict"));
        *V
    }
    /// Selector for the single-market `settle_normal_market(market, value)`.
    pub fn settle_normal_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_normal_market"));
        *V
    }
    /// Selector for the single-market `settle_lognormal_market(market, value)`.
    pub fn settle_lognormal_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_lognormal_market"));
        *V
    }
    /// Selector for the single-market `settle_bivariate_normal_market(market,
    /// point)`.
    pub fn settle_bivariate_normal_market() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("settle_bivariate_normal_market"));
        *V
    }
    /// Selector for the single-market `settle_multinoulli_market(market,
    /// outcome)`.
    pub fn settle_multinoulli_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("settle_multinoulli_market"));
        *V
    }
    /// Selector for `collect_protocol_fees(market)` (returns `u256`).
    pub fn collect_protocol_fees() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("collect_protocol_fees"));
        *V
    }
    /// Selector for `pause_market`.
    pub fn pause_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("pause_market"));
        *V
    }
    /// Selector for `unpause_market`.
    pub fn unpause_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("unpause_market"));
        *V
    }
    /// Selector for `get_market_count`.
    pub fn get_market_count() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_market_count"));
        *V
    }
    /// Selector for `get_market_at`.
    pub fn get_market_at() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_market_at"));
        *V
    }
    /// Selector for `get_owner`.
    pub fn get_owner() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_owner"));
        *V
    }
    /// Selector for `get_treasury`.
    pub fn get_treasury() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_treasury"));
        *V
    }
    /// Selector for `is_profile_enabled`.
    pub fn is_profile_enabled() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("is_profile_enabled"));
        *V
    }
    /// Selector for `get_deploy_profile`.
    pub fn get_deploy_profile() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_deploy_profile"));
        *V
    }
    /// Selector for `get_market_type_for_market`.
    pub fn get_market_type_for_market() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_market_type_for_market"));
        *V
    }
    /// Selector for `get_market_type_config`.
    pub fn get_market_type_config() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_market_type_config"));
        *V
    }
}

/// Selectors used by the oracle.
pub mod oracle {
    use std::sync::LazyLock;

    use starknet_core::types::Felt;

    /// Selector for `get_average_mean_over_period`.
    pub fn get_average_mean_over_period() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_average_mean_over_period"));
        *V
    }
    /// Selector for `get_average_mean_over_last`.
    pub fn get_average_mean_over_last() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_average_mean_over_last"));
        *V
    }
    /// Selector for `get_average_variance_over_period`.
    pub fn get_average_variance_over_period() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("get_average_variance_over_period"));
        *V
    }
    /// Selector for `get_average_variance_over_last`.
    pub fn get_average_variance_over_last() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("get_average_variance_over_last"));
        *V
    }
    /// Selector for `get_earliest_observation_time`.
    pub fn get_earliest_observation_time() -> Felt {
        static V: LazyLock<Felt> =
            LazyLock::new(|| super::compute("get_earliest_observation_time"));
        *V
    }
    /// Selector for `get_latest_observation_time`.
    pub fn get_latest_observation_time() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_latest_observation_time"));
        *V
    }
    /// Selector for `get_snapshot_count`.
    pub fn get_snapshot_count() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_snapshot_count"));
        *V
    }
    /// Selector for `get_snapshot`.
    pub fn get_snapshot() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_snapshot"));
        *V
    }
    /// Selector for `get_amm`.
    pub fn get_amm() -> Felt {
        static V: LazyLock<Felt> = LazyLock::new(|| super::compute("get_amm"));
        *V
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_are_stable() {
        // Selectors are deterministic; calling twice must yield identical values.
        assert_eq!(amm::get_distribution(), amm::get_distribution());
        assert_eq!(
            factory::deploy_normal_market_from_profile(),
            factory::deploy_normal_market_from_profile()
        );
    }

    #[test]
    fn distinct_entrypoints_have_distinct_selectors() {
        assert_ne!(amm::get_distribution(), amm::get_market_status());
        assert_ne!(amm::execute_trade(), amm::sell_position_guarded());
    }
}
