//! Structured trade log for accounting / replay.
//!
//! Every submission (trade, sell, liquidity, claim, settle) is
//! appended to a JSONL file via [`TradeJournal::append`]. The journal
//! is **opt-in**: writers that don't want the I/O cost simply don't
//! wrap themselves in a journal-aware adapter. The plain
//! `MarketWriter::execute_*` paths are unchanged.
//!
//! ## On-disk format
//!
//! Newline-delimited JSON (RFC 7464 + a trailing `\n`). Each line is
//! a single [`JournalEntry`]. The file is opened append-only and is
//! safe for multiple processes to append concurrently *as long as*
//! each append fits inside a single `write(2)` syscall (POSIX
//! typically guarantees writes ≤ 4 KiB are atomic). For multi-process
//! safety beyond that bound, prefer one journal file per bot.
//!
//! ## Replay
//!
//! [`TradeJournal::replay`] streams entries back out. Useful for
//! end-of-day reconciliation, P&L computation, and post-incident
//! analysis. The iterator yields `Result<JournalEntry, JournalError>`
//! so corrupted lines (e.g. crash-truncated) can be skipped without
//! aborting the whole replay.
//!
//! ## Pluggable backends
//!
//! [`JournalSink`] is the trait every writer-level wrapper depends
//! on. The default [`TradeJournal`] implementation writes to disk; a
//! production deployment can substitute an S3 / queue / in-memory
//! sink without touching the writer code.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use deadeye_starknet::Felt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::bulk::Family;

/// Type tag for every entry written to the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Buy / move-mu trade.
    Trade,
    /// `sell_position` / `sell_position_guarded`.
    Sell,
    /// `add_liquidity`.
    AddLiquidity,
    /// `remove_liquidity`.
    RemoveLiquidity,
    /// `claim` / `claim_for`.
    Claim,
    /// Market settlement (admin-driven).
    Settle,
}

/// One row in the trade log.
///
/// Designed so a downstream accounting pipeline can reconstruct
/// position state, P&L, and submission outcome without touching the
/// chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// When the entry was created.
    pub timestamp: SystemTime,
    /// Market family that owns this entry.
    pub family: Family,
    /// Market contract address (hex-encoded).
    #[serde(with = "felt_hex")]
    pub market: Felt,
    /// Trader account address (hex-encoded).
    #[serde(with = "felt_hex")]
    pub trader: Felt,
    /// What kind of entry this is.
    pub kind: EntryKind,
    /// Off-chain quote / inputs the writer submitted (JSON blob —
    /// each family serializes its own `TradeQuote` shape).
    pub off_chain_quote: JsonValue,
    /// Transaction hash if submission reached the network. `None`
    /// for pre-submission failures.
    #[serde(default, with = "opt_felt_hex")]
    pub tx_hash: Option<Felt>,
    /// Receipt / response (JSON blob — typically a serialized
    /// `ExecutionReceipt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<JsonValue>,
    /// Realised `PnL` once the underlying market settles. Backfilled
    /// during reconciliation; `None` while the market is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realized_pnl_at_settlement: Option<f64>,
}

impl JournalEntry {
    /// Lightweight constructor; `tx_hash`, `receipt`, and
    /// `realized_pnl` are left empty for the caller to set on the
    /// path that has them.
    #[must_use]
    pub fn new(
        family: Family,
        market: Felt,
        trader: Felt,
        kind: EntryKind,
        off_chain_quote: JsonValue,
    ) -> Self {
        Self {
            timestamp: SystemTime::now(),
            family,
            market,
            trader,
            kind,
            off_chain_quote,
            tx_hash: None,
            receipt: None,
            realized_pnl_at_settlement: None,
        }
    }

    /// Fluent setter for `tx_hash`.
    #[must_use]
    pub const fn with_tx_hash(mut self, tx_hash: Felt) -> Self {
        self.tx_hash = Some(tx_hash);
        self
    }

    /// Fluent setter for `receipt`.
    #[must_use]
    pub fn with_receipt(mut self, receipt: JsonValue) -> Self {
        self.receipt = Some(receipt);
        self
    }
}

/// Pluggable backend trait. The default file-backed implementation
/// lives in [`TradeJournal`]; tests / custom strategies can implement
/// this directly to stream to stdout, a queue, or in-memory.
pub trait JournalSink: Send {
    /// Persist an entry. Errors abort the batch.
    fn append(&mut self, entry: &JournalEntry) -> Result<(), JournalError>;
    /// Flush any buffered I/O.
    fn flush(&mut self) -> Result<(), JournalError>;
}

/// Errors emitted by [`TradeJournal`] and [`JournalSink`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JournalError {
    /// I/O-level failure (open, write, flush).
    #[error("journal I/O: {0}")]
    Io(#[from] io::Error),
    /// (De)serialization failure.
    #[error("journal serde: {0}")]
    Serde(#[from] serde_json::Error),
}

/// File-backed trade journal (JSONL on disk).
#[derive(Debug)]
pub struct TradeJournal {
    writer: BufWriter<File>,
    path: PathBuf,
}

impl TradeJournal {
    /// Open (or create) `path` in append mode. Existing files are
    /// preserved; new entries are appended.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            path: path.to_path_buf(),
        })
    }

    /// Path the journal is bound to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `entry` as a single JSONL line and flush.
    ///
    /// Durability: after `BufWriter::flush()` we call `sync_data()` on
    /// the underlying file. For an MM, "I submitted a trade but the
    /// journal crashed before flushing" is unacceptable; the cost of
    /// an fsync per write (≈ 0.1–1 ms on SSD) is dominated by the
    /// network round-trip of the trade itself. If a deployment trades
    /// throughput for durability, a `JournalSink` impl backed by
    /// async-flushing storage can substitute this default.
    #[allow(
        clippy::same_name_method,
        reason = "inherent `append` deliberately mirrors the `JournalSink::append` trait method — the trait impl forwards to it"
    )]
    pub fn append(&mut self, entry: &JournalEntry) -> Result<(), JournalError> {
        let line = serde_json::to_string(entry)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        // `sync_data` flushes file contents (not metadata) to disk.
        // Survives kernel/power crash; `flush()` alone only survives
        // process crash.
        self.writer.get_ref().sync_data()?;
        Ok(())
    }

    /// Replay every well-formed entry from `path`. Bad lines are
    /// yielded as `Err`; the caller decides whether to abort or
    /// continue.
    pub fn replay(path: &Path) -> Result<JournalReplay, JournalError> {
        let file = OpenOptions::new().read(true).open(path)?;
        Ok(JournalReplay {
            lines: BufReader::new(file).lines(),
        })
    }
}

impl JournalSink for TradeJournal {
    fn append(&mut self, entry: &JournalEntry) -> Result<(), JournalError> {
        Self::append(self, entry)
    }
    fn flush(&mut self) -> Result<(), JournalError> {
        self.writer.flush().map_err(Into::into)
    }
}

// ─── Writer wrappers (one per family) ─────────────────────────────
//
// These wrap the four `MarketWriter`s and call `journal.append()`
// after every successful submission. The wrappers are deliberately
// minimal — they re-export the most common write paths
// (`execute_quote`, `sell_position`, liquidity, claim, settle) and
// leave the more specialised paths (e.g. `execute_sparse_with_runtime`)
// to the underlying writer for advanced users.

use deadeye_starknet::{
    Account, BivariateMarketWriter, ExecutionReceipt, Felt as StarknetFelt, LognormalMarketWriter,
    MultinoulliMarketWriter, NormalMarketWriter, NormalTradeQuote, Provider as DeadeyeProvider,
    TradeResult, bivariate_amm::BivariateTradeQuote, lognormal_amm::LognormalTradeQuote,
    multinoulli_amm::MultinoulliTradeQuote,
};
use serde_json::json;

/// Journal-wrapped [`NormalMarketWriter`].
///
/// Every successful submission (`execute_quote`, `sell_position`)
/// auto-appends a [`JournalEntry`]. Errors propagate unchanged from
/// the inner writer.
#[derive(Debug)]
pub struct JournalledNormalWriter<P: DeadeyeProvider, A: Account, S: JournalSink> {
    inner: NormalMarketWriter<P, A>,
    sink: S,
}

impl<P: DeadeyeProvider, A: Account, S: JournalSink> JournalledNormalWriter<P, A, S> {
    /// Wrap an existing writer with a journal sink.
    pub const fn new(inner: NormalMarketWriter<P, A>, sink: S) -> Self {
        Self { inner, sink }
    }

    /// Borrow the inner writer (read-only methods, build_*, etc.).
    pub const fn inner(&self) -> &NormalMarketWriter<P, A> {
        &self.inner
    }

    /// Borrow the sink (for inspection / flushing).
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Mutable access to the sink (for flushing).
    pub const fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Consume the wrapper, returning the inner writer and sink.
    pub fn into_parts(self) -> (NormalMarketWriter<P, A>, S) {
        (self.inner, self.sink)
    }

    /// Submit a quote, then append a `Trade` entry on success.
    pub async fn execute_quote(
        &mut self,
        quote: NormalTradeQuote,
    ) -> TradeResult<ExecutionReceipt> {
        // Serialise pre-submit so we don't have to clone the (large)
        // raw distribution.
        let payload = serialize_normal_quote(&quote);
        let receipt = self.inner.execute_quote(quote).await?;
        let entry = JournalEntry::new(
            Family::Normal,
            self.inner.reader().address(),
            Account::address(self.inner.account()),
            EntryKind::Trade,
            payload,
        )
        .with_tx_hash(receipt.transaction_hash)
        .with_receipt(serialize_receipt(receipt));
        // Journal errors are emitted as tracing warnings so a disk
        // outage doesn't fail an otherwise-successful trade.
        if let Err(e) = self.sink.append(&entry) {
            tracing::warn!(error = %e, "journal append failed (trade)");
        }
        Ok(receipt)
    }

    /// Sell, then append a `Sell` entry on success.
    pub async fn sell_position(
        &mut self,
        runtime: StarknetFelt,
        min_token_out: u128,
    ) -> TradeResult<ExecutionReceipt> {
        let receipt = self.inner.sell_position(runtime, min_token_out).await?;
        let entry = JournalEntry::new(
            Family::Normal,
            self.inner.reader().address(),
            Account::address(self.inner.account()),
            EntryKind::Sell,
            json!({"runtime": format!("{runtime:#x}"), "min_token_out": min_token_out}),
        )
        .with_tx_hash(receipt.transaction_hash)
        .with_receipt(serialize_receipt(receipt));
        if let Err(e) = self.sink.append(&entry) {
            tracing::warn!(error = %e, "journal append failed (sell)");
        }
        Ok(receipt)
    }
}

/// Journal-wrapped [`LognormalMarketWriter`].
#[derive(Debug)]
pub struct JournalledLognormalWriter<P: DeadeyeProvider, A: Account, S: JournalSink> {
    inner: LognormalMarketWriter<P, A>,
    sink: S,
}

impl<P: DeadeyeProvider, A: Account, S: JournalSink> JournalledLognormalWriter<P, A, S> {
    /// Wrap an existing writer.
    pub const fn new(inner: LognormalMarketWriter<P, A>, sink: S) -> Self {
        Self { inner, sink }
    }

    /// Borrow the inner writer.
    pub const fn inner(&self) -> &LognormalMarketWriter<P, A> {
        &self.inner
    }

    /// Borrow the sink.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Submit and journal.
    pub async fn execute_quote(
        &mut self,
        quote: LognormalTradeQuote,
    ) -> TradeResult<ExecutionReceipt> {
        let payload = serialize_lognormal_quote(&quote);
        let receipt = self.inner.execute_quote(quote).await?;
        let entry = JournalEntry::new(
            Family::Lognormal,
            self.inner.reader().address(),
            Account::address(self.inner.account()),
            EntryKind::Trade,
            payload,
        )
        .with_tx_hash(receipt.transaction_hash)
        .with_receipt(serialize_receipt(receipt));
        if let Err(e) = self.sink.append(&entry) {
            tracing::warn!(error = %e, "journal append failed (trade)");
        }
        Ok(receipt)
    }
}

/// Journal-wrapped [`BivariateMarketWriter`].
#[derive(Debug)]
pub struct JournalledBivariateWriter<P: DeadeyeProvider, A: Account, S: JournalSink> {
    inner: BivariateMarketWriter<P, A>,
    sink: S,
}

impl<P: DeadeyeProvider, A: Account, S: JournalSink> JournalledBivariateWriter<P, A, S> {
    /// Wrap.
    pub const fn new(inner: BivariateMarketWriter<P, A>, sink: S) -> Self {
        Self { inner, sink }
    }
    /// Borrow inner.
    pub const fn inner(&self) -> &BivariateMarketWriter<P, A> {
        &self.inner
    }
    /// Borrow sink.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Submit and journal.
    pub async fn execute_quote(
        &mut self,
        quote: BivariateTradeQuote,
    ) -> TradeResult<ExecutionReceipt> {
        let payload = serialize_bivariate_quote(&quote);
        let receipt = self.inner.execute_quote(quote).await?;
        let entry = JournalEntry::new(
            Family::Bivariate,
            self.inner.reader().address(),
            Account::address(self.inner.account()),
            EntryKind::Trade,
            payload,
        )
        .with_tx_hash(receipt.transaction_hash)
        .with_receipt(serialize_receipt(receipt));
        if let Err(e) = self.sink.append(&entry) {
            tracing::warn!(error = %e, "journal append failed (trade)");
        }
        Ok(receipt)
    }
}

/// Journal-wrapped [`MultinoulliMarketWriter`].
#[derive(Debug)]
pub struct JournalledMultinoulliWriter<P: DeadeyeProvider, A: Account, S: JournalSink> {
    inner: MultinoulliMarketWriter<P, A>,
    sink: S,
}

impl<P: DeadeyeProvider, A: Account, S: JournalSink> JournalledMultinoulliWriter<P, A, S> {
    /// Wrap.
    pub const fn new(inner: MultinoulliMarketWriter<P, A>, sink: S) -> Self {
        Self { inner, sink }
    }
    /// Borrow inner.
    pub const fn inner(&self) -> &MultinoulliMarketWriter<P, A> {
        &self.inner
    }
    /// Borrow sink.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Submit and journal.
    pub async fn execute_quote(
        &mut self,
        quote: MultinoulliTradeQuote,
    ) -> TradeResult<ExecutionReceipt> {
        let candidate_clone = quote.candidate.clone();
        let receipt = self.inner.execute_quote(quote).await?;
        let entry = JournalEntry::new(
            Family::Multinoulli,
            self.inner.reader().address(),
            Account::address(self.inner.account()),
            EntryKind::Trade,
            json!({
                "candidate_outcomes": candidate_clone.probs.len(),
            }),
        )
        .with_tx_hash(receipt.transaction_hash)
        .with_receipt(serialize_receipt(receipt));
        if let Err(e) = self.sink.append(&entry) {
            tracing::warn!(error = %e, "journal append failed (trade)");
        }
        Ok(receipt)
    }
}

// ─── Helpers ──────────────────────────────────────────────────────

fn serialize_receipt(receipt: ExecutionReceipt) -> JsonValue {
    json!({
        "transaction_hash": format!("{:#x}", receipt.transaction_hash),
        "call_count": receipt.call_count,
    })
}

fn serialize_normal_quote(quote: &NormalTradeQuote) -> JsonValue {
    json!({
        "candidate": {
            "mean": deadeye_core::Sq128::from_raw(quote.candidate.mean).to_f64(),
            "sigma": deadeye_core::Sq128::from_raw(quote.candidate.sigma).to_f64(),
        },
        "x_star": deadeye_core::Sq128::from_raw(quote.x_star).to_f64(),
        "required_collateral":
            deadeye_core::Sq128::from_raw(quote.required_collateral).to_f64(),
        "padded_collateral":
            deadeye_core::Sq128::from_raw(quote.padded_collateral).to_f64(),
        "on_chain_will_accept": quote.on_chain_will_accept,
    })
}

fn serialize_lognormal_quote(quote: &LognormalTradeQuote) -> JsonValue {
    json!({
        "candidate": {
            "mu": deadeye_core::Sq128::from_raw(quote.candidate.mu).to_f64(),
            "sigma": deadeye_core::Sq128::from_raw(quote.candidate.sigma).to_f64(),
        },
        "x_star": deadeye_core::Sq128::from_raw(quote.x_star).to_f64(),
        "required_collateral":
            deadeye_core::Sq128::from_raw(quote.required_collateral).to_f64(),
        "padded_collateral":
            deadeye_core::Sq128::from_raw(quote.padded_collateral).to_f64(),
        "on_chain_will_accept": quote.on_chain_will_accept,
    })
}

fn serialize_bivariate_quote(quote: &BivariateTradeQuote) -> JsonValue {
    json!({
        "candidate": {
            "mu1": deadeye_core::Sq128::from_raw(quote.candidate.mu1).to_f64(),
            "mu2": deadeye_core::Sq128::from_raw(quote.candidate.mu2).to_f64(),
            "sigma1": deadeye_core::Sq128::from_raw(quote.candidate.sigma1).to_f64(),
            "sigma2": deadeye_core::Sq128::from_raw(quote.candidate.sigma2).to_f64(),
            "rho": deadeye_core::Sq128::from_raw(quote.candidate.rho).to_f64(),
        },
        "x_star_x1": deadeye_core::Sq128::from_raw(quote.x_star.x1).to_f64(),
        "x_star_x2": deadeye_core::Sq128::from_raw(quote.x_star.x2).to_f64(),
        "supplied_collateral":
            deadeye_core::Sq128::from_raw(quote.supplied_collateral).to_f64(),
        "on_chain_will_accept": quote.on_chain_will_accept,
    })
}

/// Iterator returned by [`TradeJournal::replay`].
#[derive(Debug)]
pub struct JournalReplay {
    lines: io::Lines<BufReader<File>>,
}

impl Iterator for JournalReplay {
    type Item = Result<JournalEntry, JournalError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let line = self.lines.next()?;
            match line {
                // Skip blanks — the journal occasionally ships an empty
                // line on rotation.
                Ok(s) if s.trim().is_empty() => {},
                Ok(s) => {
                    return Some(serde_json::from_str(&s).map_err(JournalError::from));
                },
                Err(e) => return Some(Err(JournalError::from(e))),
            }
        }
    }
}

/// Hex-encoded [`Felt`] in JSON.
mod felt_hex {
    use deadeye_starknet::Felt;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(felt: &Felt, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{felt:#x}"))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Felt, D::Error> {
        let s = String::deserialize(d)?;
        Felt::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Hex-encoded `Option<Felt>` in JSON.
#[allow(
    clippy::ref_option,
    reason = "serde `with` helpers must accept `&Option<T>` — the trait signature is fixed"
)]
mod opt_felt_hex {
    use deadeye_starknet::Felt;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(felt: &Option<Felt>, s: S) -> Result<S::Ok, S::Error> {
        match felt {
            Some(f) => s.serialize_str(&format!("{f:#x}")),
            None => s.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Felt>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            Some(s) => Felt::from_hex(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use serde_json::json;

    use super::*;

    fn dummy_entry() -> JournalEntry {
        JournalEntry::new(
            Family::Normal,
            Felt::from(0x1234_u64),
            Felt::from(0xABCD_u64),
            EntryKind::Trade,
            json!({
                "candidate": {"mean": 42.0, "sigma": 8.0},
                "x_star": 43.0,
                "required_collateral": 1.23,
            }),
        )
        .with_tx_hash(Felt::from(0xCAFE_u64))
        .with_receipt(json!({"transaction_hash": "0xcafe", "call_count": 1}))
    }

    #[test]
    fn json_round_trip_preserves_fields() {
        let entry = dummy_entry();
        let s = serde_json::to_string(&entry).unwrap();
        let back: JournalEntry = serde_json::from_str(&s).unwrap();
        assert_eq!(back.family, entry.family);
        assert_eq!(back.market, entry.market);
        assert_eq!(back.trader, entry.trader);
        assert_eq!(back.kind, entry.kind);
        assert_eq!(back.tx_hash, entry.tx_hash);
        assert_eq!(back.off_chain_quote, entry.off_chain_quote);
    }

    #[test]
    fn append_and_replay_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.jsonl");
        let entry = dummy_entry();
        {
            let mut journal = TradeJournal::open(&path).unwrap();
            journal.append(&entry).unwrap();
            journal.append(&entry).unwrap();
            JournalSink::flush(&mut journal).unwrap();
        }
        let replayed: Vec<JournalEntry> = TradeJournal::replay(&path)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(replayed.len(), 2);
        for r in &replayed {
            assert_eq!(r.kind, EntryKind::Trade);
            assert_eq!(r.tx_hash, Some(Felt::from(0xCAFE_u64)));
        }
    }

    #[test]
    fn replay_skips_empty_lines_and_surfaces_bad_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dirty.jsonl");
        std::fs::write(&path, b"{\n\n{not valid json}\n").unwrap();
        let results: Vec<_> = TradeJournal::replay(&path).unwrap().collect();
        // Two non-empty lines were yielded; both fail (truncated +
        // invalid JSON), so we expect two Errs.
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(Result::is_err));
    }
}
