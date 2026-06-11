//! Step through the optimizer's inner loop manually.

#![allow(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::suboptimal_flops,
    reason = "dev-only example binary; stdout is the UX, panics on bad input are fine"
)]

use deadeye_collateral::{MinimizationPolicy, lambda, normal_collateral};
use deadeye_core::{NormalDistribution, Sq128};

fn gpi(mu1: f64, s1: f64, mu2: f64, s2: f64) -> f64 {
    let sv = (s1 * s1) + (s2 * s2);
    (1.0 / (2.0 * std::f64::consts::PI * sv).sqrt()) * (-(mu1 - mu2).powi(2) / (2.0 * sv)).exp()
}

fn main() {
    let mu_m = 4.29_f64;
    let s_m = 0.35_f64;
    let mu_b = 4.3274_f64;
    let s_b = 0.2143_f64;
    let k = 75.07_f64;

    println!("market: μ={mu_m}, σ={s_m}");
    println!("belief: μ={mu_b}, σ={s_b}");
    println!("k_eff:  {k}");
    println!();

    // Sweep σ candidates at μ_g = μ_market
    println!(
        "{:>8} {:>10} {:>10} {:>10} {:>10}",
        "σ_g", "λ_g", "EV", "cost", "net"
    );
    println!("{}", "─".repeat(54));

    let f = NormalDistribution::from_variance(
        Sq128::from_f64(mu_m).unwrap(),
        Sq128::from_f64(s_m * s_m).unwrap(),
    )
    .unwrap();
    let lam_f = lambda(s_m, k);

    for s_g_idx in 1..=15 {
        let s_g = 0.05 + 0.025 * f64::from(s_g_idx);
        let lam_g = lambda(s_g, k);
        let ev_g = lam_g * gpi(mu_m, s_g, mu_b, s_b);
        let ev_f = lam_f * gpi(mu_m, s_m, mu_b, s_b);
        let raw_ev = ev_g - ev_f;

        let g = NormalDistribution::from_variance(
            Sq128::from_f64(mu_m).unwrap(),
            Sq128::from_f64(s_g * s_g).unwrap(),
        )
        .unwrap();

        let cost = match normal_collateral(&f, &g, MinimizationPolicy::unrestricted()) {
            Ok(v) => v.collateral,
            Err(_) => f64::NAN,
        };

        let net = raw_ev - cost;
        let marker = if net > 0.0 { " ←" } else { "" };
        println!("{s_g:>8.4} {lam_g:>10.4} {raw_ev:>10.4} {cost:>10.4} {net:>10.4}{marker}");
    }
}
