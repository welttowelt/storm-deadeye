//! Serializable view types + `Render` impls for each command.
//!
//! Every command builds one of these from its raw SDK reads, then hands
//! it to the [`Renderer`](crate::output::Renderer). Keeping the view
//! types in one place means the JSON wire format is one grep away from
//! the table layout — the two never drift.

use std::io::{self, Write};

use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use deadeye_indexer::{MarketSummary, Position as IndexerPosition};
use serde::Serialize;

use crate::output::{Render, Renderer};

/// Output of `deadeye account show`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AccountView {
    pub(crate) profile: String,
    pub(crate) address: Option<String>,
    pub(crate) chain_id: String,
    pub(crate) rpc_url: String,
    pub(crate) indexer_url: String,
    /// STRK balance in base units (10^18 = 1 STRK). `None` if address unset.
    pub(crate) strk_balance_base: Option<u128>,
    /// STRK balance as whole STRK (`base / 1e18`).
    pub(crate) strk_balance_strk: Option<f64>,
}

impl Render for AccountView {
    fn render_pretty(&self, r: &Renderer) {
        r.header("Account");
        r.kv("profile", &r.highlight(&self.profile));
        r.kv(
            "address",
            self.address.as_deref().unwrap_or(&r.dim("(unset)")),
        );
        r.kv("chain id", &self.chain_id);
        r.kv("rpc", &r.dim(&self.rpc_url));
        r.kv("indexer", &r.dim(&self.indexer_url));
        match (self.strk_balance_base, self.strk_balance_strk) {
            (Some(base), Some(strk)) => {
                r.kv("STRK balance", &format!("{strk:.6} STRK  ({base} base units)"));
            },
            _ => {
                r.kv("STRK balance", &r.dim("(unknown — address required)"));
            },
        }
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "profile: {}", self.profile)?;
        writeln!(w, "address: {}", self.address.as_deref().unwrap_or("-"))?;
        writeln!(w, "chain_id: {}", self.chain_id)?;
        writeln!(w, "rpc_url: {}", self.rpc_url)?;
        writeln!(w, "indexer_url: {}", self.indexer_url)?;
        if let (Some(base), Some(strk)) = (self.strk_balance_base, self.strk_balance_strk) {
            writeln!(w, "strk_balance_base: {base}")?;
            writeln!(w, "strk_balance_strk: {strk:.6}")?;
        }
        Ok(())
    }
}

/// One row of `deadeye markets list`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MarketRow {
    pub(crate) address: String,
    pub(crate) family: String,
    pub(crate) title: String,
    pub(crate) mu: Option<f64>,
    pub(crate) sigma: Option<f64>,
    pub(crate) k: Option<f64>,
    pub(crate) backing: Option<String>,
    pub(crate) settled: bool,
    pub(crate) is_active: bool,
}

impl MarketRow {
    pub(crate) fn from_summary(s: &MarketSummary) -> Self {
        let (mu, sigma, k, backing, settled) = match (&s.state, &s.multinoulli_state) {
            (Some(n), _) => (n.mean, n.sigma, n.k, n.total_backing.clone(), n.is_settled),
            (_, Some(m)) => (None, None, m.k, m.total_backing.clone(), m.is_settled),
            _ => (None, None, None, None, false),
        };
        Self {
            address: s.address.clone(),
            family: s.market_type.clone(),
            title: s.title.clone(),
            mu,
            sigma,
            k,
            backing,
            settled,
            is_active: s.is_active,
        }
    }
}

impl Render for MarketRow {
    fn render_pretty(&self, r: &Renderer) {
        // Single-row fallback when used outside a table. Reuse the row
        // renderer.
        Self::render_pretty_table_header(self, r);
        self.render_pretty_table_row(r);
        Self::render_pretty_table_footer(self, r);
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "address: {}", self.address)?;
        writeln!(w, "family: {}", self.family)?;
        writeln!(w, "title: {}", self.title)?;
        writeln!(w, "mu: {}", opt_f(self.mu))?;
        writeln!(w, "sigma: {}", opt_f(self.sigma))?;
        writeln!(w, "k: {}", opt_f(self.k))?;
        writeln!(w, "backing: {}", self.backing.as_deref().unwrap_or("-"))?;
        writeln!(w, "settled: {}", self.settled)?;
        writeln!(w, "active: {}", self.is_active)?;
        Ok(())
    }

    fn render_pretty_table_header(&self, _r: &Renderer) {
        // No-op — we render the whole table in the footer to leverage
        // comfy-table's buffering. This trait method is called once per
        // call, so we use a thread-local Table.
        TABLE_BUFFER.with_borrow_mut(|t| {
            *t = make_market_table();
        });
    }

    fn render_pretty_table_row(&self, _r: &Renderer) {
        TABLE_BUFFER.with_borrow_mut(|t| {
            t.add_row(vec![
                short_addr(&self.address),
                self.family.clone(),
                ellipsis(&self.title, 32),
                opt_f(self.mu),
                opt_f(self.sigma),
                opt_f(self.k),
                self.backing.clone().unwrap_or_else(|| "-".into()),
                if self.settled { "yes".into() } else { "no".into() },
            ]);
        });
    }

    fn render_pretty_table_footer(&self, _r: &Renderer) {
        TABLE_BUFFER.with_borrow(|t| {
            println!("{t}");
        });
    }
}

thread_local! {
    static TABLE_BUFFER: core::cell::RefCell<Table> = core::cell::RefCell::new(Table::new());
}

fn make_market_table() -> Table {
    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["addr", "family", "title", "μ", "σ", "k", "backing", "settled"]);
    t
}

fn make_position_table() -> Table {
    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["market", "family", "collateral", "share_pct", "claimed"]);
    t
}

/// Full on-chain view of one market.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MarketShowView {
    pub(crate) address: String,
    pub(crate) family: String,
    pub(crate) distribution: serde_json::Value,
    pub(crate) params: MarketParamsView,
    pub(crate) lp_info: MarketLpInfoView,
    pub(crate) fee_config: MarketFeeConfigView,
    pub(crate) status: Option<MarketStatusView>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MarketParamsView {
    pub(crate) k: f64,
    pub(crate) backing: f64,
    pub(crate) tolerance: f64,
    pub(crate) min_trade_collateral: f64,
    pub(crate) payout_amplifier: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MarketLpInfoView {
    pub(crate) total_shares: f64,
    pub(crate) total_backing_deposited: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct MarketFeeConfigView {
    pub(crate) lp_fee_bps: u16,
    pub(crate) protocol_fee_bps: u16,
    pub(crate) settlement_fee_bps: u16,
    pub(crate) total_bps: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct MarketStatusView {
    pub(crate) is_initialised: bool,
    pub(crate) is_paused: bool,
    pub(crate) is_settled: bool,
    pub(crate) settlement_value: f64,
}

impl Render for MarketShowView {
    fn render_pretty(&self, r: &Renderer) {
        r.header(&format!("Market {} ({})", self.address, self.family));
        r.kv("family", &r.highlight(&self.family));
        if let Some(status) = self.status {
            r.kv(
                "status",
                &format!(
                    "init={} paused={} settled={}",
                    yn(status.is_initialised),
                    yn(status.is_paused),
                    yn(status.is_settled),
                ),
            );
            if status.is_settled {
                r.kv("settlement_value", &format!("{:.6}", status.settlement_value));
            }
        }
        // Distribution: render the underlying JSON as a series of kv pairs.
        if let serde_json::Value::Object(map) = &self.distribution {
            for (key, val) in map {
                r.kv(&format!("dist.{key}"), &fmt_json_scalar(val));
            }
        }
        r.kv("params.k", &format!("{:.6}", self.params.k));
        r.kv("params.backing", &format!("{:.6}", self.params.backing));
        r.kv("params.tolerance", &format!("{:.3e}", self.params.tolerance));
        r.kv(
            "params.min_trade_collateral",
            &format!("{:.6}", self.params.min_trade_collateral),
        );
        r.kv(
            "params.payout_amplifier",
            &format!("{:.6}", self.params.payout_amplifier),
        );
        r.kv(
            "lp.total_shares",
            &format!("{:.6}", self.lp_info.total_shares),
        );
        r.kv(
            "lp.total_backing_deposited",
            &format!("{:.6}", self.lp_info.total_backing_deposited),
        );
        r.kv(
            "fees",
            &format!(
                "lp={}bps, protocol={}bps, settlement={}bps (total {}bps)",
                self.fee_config.lp_fee_bps,
                self.fee_config.protocol_fee_bps,
                self.fee_config.settlement_fee_bps,
                self.fee_config.total_bps,
            ),
        );
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "address: {}", self.address)?;
        writeln!(w, "family: {}", self.family)?;
        if let Some(s) = self.status {
            writeln!(w, "is_initialised: {}", s.is_initialised)?;
            writeln!(w, "is_paused: {}", s.is_paused)?;
            writeln!(w, "is_settled: {}", s.is_settled)?;
            writeln!(w, "settlement_value: {:.6}", s.settlement_value)?;
        }
        if let serde_json::Value::Object(map) = &self.distribution {
            for (key, val) in map {
                writeln!(w, "dist.{key}: {}", fmt_json_scalar(val))?;
            }
        }
        writeln!(w, "params.k: {:.6}", self.params.k)?;
        writeln!(w, "params.backing: {:.6}", self.params.backing)?;
        writeln!(w, "params.tolerance: {:.3e}", self.params.tolerance)?;
        writeln!(
            w,
            "params.min_trade_collateral: {:.6}",
            self.params.min_trade_collateral
        )?;
        writeln!(
            w,
            "params.payout_amplifier: {:.6}",
            self.params.payout_amplifier
        )?;
        writeln!(w, "lp.total_shares: {:.6}", self.lp_info.total_shares)?;
        writeln!(
            w,
            "lp.total_backing_deposited: {:.6}",
            self.lp_info.total_backing_deposited
        )?;
        writeln!(w, "fees.lp_bps: {}", self.fee_config.lp_fee_bps)?;
        writeln!(w, "fees.protocol_bps: {}", self.fee_config.protocol_fee_bps)?;
        writeln!(w, "fees.settlement_bps: {}", self.fee_config.settlement_fee_bps)?;
        writeln!(w, "fees.total_bps: {}", self.fee_config.total_bps)?;
        Ok(())
    }
}

/// Indexer-side metadata for one market (`markets info`).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MarketInfoView {
    pub(crate) summary: MarketSummary,
}

impl Render for MarketInfoView {
    fn render_pretty(&self, r: &Renderer) {
        let s = &self.summary;
        r.header(&format!("{} ({})", s.title, s.market_type));
        r.kv("address", &s.address);
        r.kv("active", &yn(s.is_active));
        r.kv(
            "category",
            s.category.as_deref().unwrap_or(&r.dim("(none)")),
        );
        if !s.topics.is_empty() {
            r.kv("topics", &s.topics.join(", "));
        }
        if !s.description.is_empty() {
            r.kv("description", &s.description);
        }
        r.kv("created_at", &s.created_at.to_string());
        r.kv("updated_at", &s.updated_at.to_string());
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        let s = &self.summary;
        writeln!(w, "address: {}", s.address)?;
        writeln!(w, "family: {}", s.market_type)?;
        writeln!(w, "title: {}", s.title)?;
        writeln!(w, "active: {}", s.is_active)?;
        writeln!(w, "category: {}", s.category.as_deref().unwrap_or("-"))?;
        if !s.topics.is_empty() {
            writeln!(w, "topics: {}", s.topics.join(","))?;
        }
        if !s.description.is_empty() {
            writeln!(w, "description: {}", s.description)?;
        }
        writeln!(w, "created_at: {}", s.created_at)?;
        writeln!(w, "updated_at: {}", s.updated_at)?;
        Ok(())
    }
}

/// One row of `deadeye position list`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PositionRow {
    pub(crate) market_address: String,
    pub(crate) family: String,
    pub(crate) collateral_locked: Option<String>,
    pub(crate) collateral_f64: Option<f64>,
    pub(crate) settlement_state: Option<String>,
    pub(crate) claimed: bool,
}

impl PositionRow {
    pub(crate) fn from_indexer(p: &IndexerPosition) -> Self {
        let cf = p
            .collateral_locked
            .as_ref()
            .and_then(|s| s.parse::<u128>().ok())
            .map(|n| n as f64 / 1e18_f64);
        Self {
            market_address: p.market_address.clone(),
            family: String::new(),
            collateral_locked: p.collateral_locked.clone(),
            collateral_f64: cf,
            settlement_state: p.settlement_state.clone(),
            claimed: p.claimed,
        }
    }
}

impl Render for PositionRow {
    fn render_pretty(&self, r: &Renderer) {
        Self::render_pretty_table_header(self, r);
        self.render_pretty_table_row(r);
        Self::render_pretty_table_footer(self, r);
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "market_address: {}", self.market_address)?;
        writeln!(w, "family: {}", self.family)?;
        writeln!(
            w,
            "collateral_locked: {}",
            self.collateral_locked.as_deref().unwrap_or("-")
        )?;
        writeln!(w, "collateral_strk: {}", opt_f(self.collateral_f64))?;
        writeln!(
            w,
            "settlement_state: {}",
            self.settlement_state.as_deref().unwrap_or("-")
        )?;
        writeln!(w, "claimed: {}", self.claimed)?;
        Ok(())
    }

    fn render_pretty_table_header(&self, _r: &Renderer) {
        TABLE_BUFFER.with_borrow_mut(|t| {
            *t = make_position_table();
        });
    }

    fn render_pretty_table_row(&self, _r: &Renderer) {
        TABLE_BUFFER.with_borrow_mut(|t| {
            t.add_row(vec![
                short_addr(&self.market_address),
                self.family.clone(),
                opt_f(self.collateral_f64),
                self.settlement_state.clone().unwrap_or_else(|| "-".into()),
                if self.claimed { "yes".into() } else { "no".into() },
            ]);
        });
    }

    fn render_pretty_table_footer(&self, _r: &Renderer) {
        TABLE_BUFFER.with_borrow(|t| {
            println!("{t}");
        });
    }
}

/// Compact decoded position for a single market.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PositionShowView {
    pub(crate) market_address: String,
    pub(crate) trader: String,
    pub(crate) family: String,
    pub(crate) total_collateral: f64,
    pub(crate) flags: u32,
    pub(crate) extra: serde_json::Value,
}

impl Render for PositionShowView {
    fn render_pretty(&self, r: &Renderer) {
        r.header(&format!("Position {} on {}", self.trader, self.market_address));
        r.kv("family", &r.highlight(&self.family));
        r.kv("total_collateral", &format!("{:.6}", self.total_collateral));
        r.kv("flags", &format!("{:#x}", self.flags));
        if let serde_json::Value::Object(map) = &self.extra {
            for (key, val) in map {
                r.kv(&format!("entry.{key}"), &fmt_json_scalar(val));
            }
        }
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "market_address: {}", self.market_address)?;
        writeln!(w, "trader: {}", self.trader)?;
        writeln!(w, "family: {}", self.family)?;
        writeln!(w, "total_collateral: {:.6}", self.total_collateral)?;
        writeln!(w, "flags: {:#x}", self.flags)?;
        if let serde_json::Value::Object(map) = &self.extra {
            for (key, val) in map {
                writeln!(w, "entry.{key}: {}", fmt_json_scalar(val))?;
            }
        }
        Ok(())
    }
}

/// `deadeye config show` output.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConfigShowView {
    pub(crate) config_path: String,
    pub(crate) default_profile: Option<String>,
    pub(crate) active_profile: String,
    pub(crate) rpc_url: String,
    pub(crate) indexer_url: String,
    pub(crate) chain_id: String,
    pub(crate) address: Option<String>,
    /// `***` when set, `null` when unset. Never the actual key.
    pub(crate) private_key: Option<&'static str>,
}

impl Render for ConfigShowView {
    fn render_pretty(&self, r: &Renderer) {
        r.header("Resolved configuration");
        r.kv("config_path", &r.dim(&self.config_path));
        r.kv(
            "default_profile",
            self.default_profile
                .as_deref()
                .unwrap_or(&r.dim("(none)")),
        );
        r.kv("active_profile", &r.highlight(&self.active_profile));
        r.kv("rpc_url", &self.rpc_url);
        r.kv("indexer_url", &self.indexer_url);
        r.kv("chain_id", &self.chain_id);
        r.kv(
            "address",
            self.address.as_deref().unwrap_or(&r.dim("(unset)")),
        );
        r.kv(
            "private_key",
            self.private_key.unwrap_or(&r.dim("(unset; use DEADEYE_PRIVATE_KEY)")),
        );
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "config_path: {}", self.config_path)?;
        writeln!(
            w,
            "default_profile: {}",
            self.default_profile.as_deref().unwrap_or("-")
        )?;
        writeln!(w, "active_profile: {}", self.active_profile)?;
        writeln!(w, "rpc_url: {}", self.rpc_url)?;
        writeln!(w, "indexer_url: {}", self.indexer_url)?;
        writeln!(w, "chain_id: {}", self.chain_id)?;
        writeln!(w, "address: {}", self.address.as_deref().unwrap_or("-"))?;
        writeln!(w, "private_key: {}", self.private_key.unwrap_or("-"))?;
        Ok(())
    }
}

/// `deadeye config profile-list` row.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProfileRow {
    pub(crate) name: String,
    pub(crate) rpc_url: Option<String>,
    pub(crate) indexer_url: Option<String>,
    pub(crate) address: Option<String>,
    pub(crate) is_default: bool,
}

impl Render for ProfileRow {
    fn render_pretty(&self, r: &Renderer) {
        let star = if self.is_default { "*" } else { " " };
        println!(
            "{} {}",
            r.highlight(star),
            r.highlight(&self.name),
        );
        r.kv("rpc_url", self.rpc_url.as_deref().unwrap_or("-"));
        r.kv("indexer_url", self.indexer_url.as_deref().unwrap_or("-"));
        r.kv("address", self.address.as_deref().unwrap_or("-"));
    }

    fn render_plain(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "name: {}", self.name)?;
        writeln!(w, "default: {}", self.is_default)?;
        writeln!(w, "rpc_url: {}", self.rpc_url.as_deref().unwrap_or("-"))?;
        writeln!(
            w,
            "indexer_url: {}",
            self.indexer_url.as_deref().unwrap_or("-")
        )?;
        writeln!(w, "address: {}", self.address.as_deref().unwrap_or("-"))?;
        Ok(())
    }
}

fn opt_f(v: Option<f64>) -> String {
    v.map_or_else(|| "-".to_owned(), |x| format!("{x:.4}"))
}

fn yn(b: bool) -> String {
    if b { "yes".to_owned() } else { "no".to_owned() }
}

fn short_addr(s: &str) -> String {
    if s.len() <= 14 {
        return s.to_owned();
    }
    let head = &s[..8];
    let tail = &s[s.len() - 4..];
    format!("{head}…{tail}")
}

fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

fn fmt_json_scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_owned(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{f:.6}")
            } else {
                n.to_string()
            }
        },
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
