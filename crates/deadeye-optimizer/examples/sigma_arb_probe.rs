//! Probe: does the optimizer find a positive-EV σ-only trade when the
//! belief and market μ are close but the belief σ is tighter?
//!
//! Driven by the live CPI mainnet numbers (2026-05-14):
//!   belief : μ=4.3274 σ=0.2143 (cpi_decomposition_v3 against live FRED)
//!   market : μ=4.2900 σ=0.3500 (mainnet AMM via Cartridge RPC)
//!   k_eff  : 75.07 (live, via fly.dev indexer)

use deadeye_optimizer::{NormalOptimizationInput, optimize_normal_trade};

fn main() {
    let cases: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
        // (label, mu_b, sigma_b, mu_m, sigma_m, k_eff, budget)
        ("live-CPI-2026-05-14",        4.3274, 0.2143, 4.2900, 0.3500, 75.07, 50.0),
        ("μ-only arb HUGE budget",     4.5000, 0.3500, 4.2900, 0.3500, 75.07, 1e9),
        ("σ-only HUGE budget",         4.2900, 0.2143, 4.2900, 0.3500, 75.07, 1e9),
        ("classic README scenario",    2.4700, 0.1581, 2.1000, 0.2000, 50.00, 50.0),
        ("scenario w/ low k",          4.5000, 0.3500, 4.2900, 0.3500,  1.00, 50.0),
        ("CPI shape unequal-σ",        4.3274, 0.2143, 2.1000, 0.3500, 75.07, 50.0),
        ("CPI σ + low-μ market",       4.3274, 0.2143, 0.1000, 0.3500, 75.07, 50.0),
        ("README+CPI-μ μ-only",        2.4700, 0.2000, 2.1000, 0.2000, 50.00, 50.0),
    ];

    println!(
        "{:<26} {:>9} {:>9} {:>9} {:>9} {:>11} {:>10} {:>9} {:>9} {:>9} {:>9}",
        "case", "μ_b", "σ_b", "μ_m", "σ_m", "k_eff", "budget", "μ_g*", "σ_g*", "coll", "EV"
    );
    println!("{}", "─".repeat(132));

    for (label, mu_b, sigma_b, mu_m, sigma_m, k, budget) in cases.iter().copied() {
        let r = optimize_normal_trade(NormalOptimizationInput::new(
            budget, mu_b, sigma_b, mu_m, sigma_m, k,
        ));
        println!(
            "{label:<26} {mu_b:>9.4} {sigma_b:>9.4} {mu_m:>9.4} {sigma_m:>9.4} {k:>11.4} \
             {budget:>10.2} {:>9.4} {:>9.4} {:>9.4} {:>9.4}",
            r.optimized_mean,
            r.optimized_sigma,
            r.collateral_required,
            r.expected_value,
        );
    }
}
