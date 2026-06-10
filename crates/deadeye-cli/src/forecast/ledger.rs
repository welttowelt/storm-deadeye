//! File-backed forecast workspace — one directory per market under the
//! deadeye data dir (`~/.local/share/deadeye/forecasts/<market>/`, override
//! with `DEADEYE_FORECAST_DIR`). It is the durable substrate where an agent
//! accumulates evidence and reference classes and curates them into a
//! committed forecast snapshot:
//!
//! ```text
//! <market>/
//!   question.json     the market question + outcome space
//!   evidence.jsonl    append-only, timestamped evidence items
//!   base_rates.json   reference classes + blended prior
//!   snapshot.json     the current curated (mean, sd) forecast + rationale
//! ```
//!
//! Append-only evidence keeps the reasoning trail auditable; the snapshot is
//! the thing you hand to `deadeye trade quote --mean --variance`.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Unix epoch seconds, best-effort (0 before the epoch).
#[must_use]
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The market question being forecast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Question {
    /// Market contract address (the workspace key).
    pub(crate) market: String,
    /// Human question / market title.
    pub(crate) title: String,
    /// What outcome resolves the market (free text).
    #[serde(default)]
    pub(crate) resolution_criteria: String,
    /// Lower bound of the outcome range, if continuous.
    #[serde(default)]
    pub(crate) lower_bound: Option<f64>,
    /// Upper bound of the outcome range, if continuous.
    #[serde(default)]
    pub(crate) upper_bound: Option<f64>,
    /// Creation timestamp (unix seconds).
    pub(crate) created_at: u64,
}

/// Which way a piece of evidence pushes the forecast.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Stance {
    /// Pushes the outcome up / supports the hypothesis.
    Up,
    /// Pushes the outcome down / cuts against.
    Down,
    /// Background context, no directional signal.
    Context,
    /// Mixed signal.
    Mixed,
}

/// One timestamped, source-linked evidence item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvidenceItem {
    /// Short id (assigned on add).
    pub(crate) id: String,
    /// When it was recorded (unix seconds).
    pub(crate) captured_at: u64,
    /// The headline claim.
    pub(crate) claim: String,
    /// Optional source label (e.g. `FRED CPIAUCSL`, `BLS`).
    #[serde(default)]
    pub(crate) source: Option<String>,
    /// Optional source URL.
    #[serde(default)]
    pub(crate) url: Option<String>,
    /// Direction of the signal.
    pub(crate) stance: Stance,
    /// Source reliability `[0, 1]`.
    #[serde(default)]
    pub(crate) reliability: Option<f64>,
    /// Relevance to the question `[0, 1]`.
    #[serde(default)]
    pub(crate) relevance: Option<f64>,
}

/// A reference class contributing to the base-rate prior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReferenceClass {
    /// Name / description of the class.
    pub(crate) name: String,
    /// Class base rate (probability in `[0,1]`, or a numeric anchor).
    pub(crate) base_rate: f64,
    /// Applicability to this question `[0, 1]`.
    pub(crate) applicability: f64,
    /// Within-class uncertainty `>= 0`.
    #[serde(default)]
    pub(crate) uncertainty: f64,
}

/// The current curated forecast for the market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    /// Forecast mean (μ) — feed to `trade quote --mean`.
    pub(crate) mean: f64,
    /// Forecast sd (σ).
    pub(crate) sd: f64,
    /// Variance (σ²) — feed to `trade quote --variance`.
    pub(crate) variance: f64,
    /// Pooling / aggregation method used.
    #[serde(default)]
    pub(crate) method: String,
    /// Prose rationale.
    #[serde(default)]
    pub(crate) rationale: String,
    /// Crisp talking points that would move the forecast up.
    #[serde(default)]
    pub(crate) reasons_up: Vec<String>,
    /// Crisp talking points that would move it down.
    #[serde(default)]
    pub(crate) reasons_down: Vec<String>,
    /// What would change the forecaster's mind.
    #[serde(default)]
    pub(crate) change_my_mind: Vec<String>,
    /// When this snapshot was committed (unix seconds).
    pub(crate) created_at: u64,
}

/// Handle to one market's forecast workspace.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    dir: PathBuf,
    market: String,
}

impl Workspace {
    /// Resolve (but do not create) the workspace for `market`.
    pub(crate) fn resolve(market: &str) -> Result<Self> {
        Ok(Self::with_base(&base_dir()?, market))
    }

    /// Resolve under an explicit base directory (used by tests).
    #[must_use]
    pub(crate) fn with_base(base: &Path, market: &str) -> Self {
        let key = market.trim().to_lowercase();
        Self {
            dir: base.join(&key),
            market: key,
        }
    }

    /// The market key (lowercased address).
    #[must_use]
    pub(crate) fn market(&self) -> &str {
        &self.market
    }

    /// The on-disk directory.
    #[must_use]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the workspace exists on disk.
    #[must_use]
    pub(crate) fn exists(&self) -> bool {
        self.question_path().exists()
    }

    fn question_path(&self) -> PathBuf {
        self.dir.join("question.json")
    }
    fn evidence_path(&self) -> PathBuf {
        self.dir.join("evidence.jsonl")
    }
    fn base_rates_path(&self) -> PathBuf {
        self.dir.join("base_rates.json")
    }
    fn snapshot_path(&self) -> PathBuf {
        self.dir.join("snapshot.json")
    }

    fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating forecast dir {}", self.dir.display()))
    }

    /// Write the question record, creating the workspace.
    pub(crate) fn save_question(&self, q: &Question) -> Result<()> {
        self.ensure_dir()?;
        write_json(&self.question_path(), q)
    }

    /// Load the question record.
    pub(crate) fn load_question(&self) -> Result<Question> {
        read_json(&self.question_path())
    }

    /// Append one evidence item (assigning it the next id). Returns the id.
    pub(crate) fn add_evidence(&self, mut item: EvidenceItem) -> Result<String> {
        self.ensure_dir()?;
        let next = self.load_evidence()?.len() + 1;
        item.id = format!("e{next}");
        let line = serde_json::to_string(&item).context("serializing evidence item")?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.evidence_path())
            .with_context(|| format!("opening {}", self.evidence_path().display()))?;
        writeln!(f, "{line}").context("appending evidence")?;
        Ok(item.id)
    }

    /// Load all evidence items in capture order.
    pub(crate) fn load_evidence(&self) -> Result<Vec<EvidenceItem>> {
        let path = self.evidence_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).context("parsing evidence line"))
            .collect()
    }

    /// Replace the set of reference classes.
    pub(crate) fn save_base_rates(&self, classes: &[ReferenceClass]) -> Result<()> {
        self.ensure_dir()?;
        write_json(&self.base_rates_path(), &classes.to_vec())
    }

    /// Load reference classes (empty if none).
    pub(crate) fn load_base_rates(&self) -> Result<Vec<ReferenceClass>> {
        if !self.base_rates_path().exists() {
            return Ok(Vec::new());
        }
        read_json(&self.base_rates_path())
    }

    /// Commit the curated snapshot.
    pub(crate) fn save_snapshot(&self, snap: &Snapshot) -> Result<()> {
        self.ensure_dir()?;
        write_json(&self.snapshot_path(), snap)
    }

    /// Load the current snapshot, if any.
    pub(crate) fn load_snapshot(&self) -> Result<Option<Snapshot>> {
        if !self.snapshot_path().exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&self.snapshot_path())?))
    }
}

/// List every market that has a forecast workspace.
pub(crate) fn list_markets() -> Result<Vec<String>> {
    list_markets_in(&base_dir()?)
}

/// List markets under an explicit base dir (used by tests).
pub(crate) fn list_markets_in(base: &Path) -> Result<Vec<String>> {
    if !base.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(base).with_context(|| format!("reading {}", base.display()))? {
        let entry = entry?;
        if entry.path().join("question.json").exists()
            && let Some(name) = entry.file_name().to_str()
        {
            out.push(name.to_owned());
        }
    }
    out.sort();
    Ok(out)
}

/// Base directory for all forecast workspaces.
fn base_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("DEADEYE_FORECAST_DIR") {
        return Ok(PathBuf::from(p));
    }
    let mut dir = dirs::data_dir()
        .context("could not locate user data dir; set DEADEYE_FORECAST_DIR to override")?;
    dir.push("deadeye");
    dir.push("forecasts");
    Ok(dir)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value).context("serializing JSON")?;
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on I/O or parse failure")]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_question_evidence_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::with_base(tmp.path(), "0xABC");
        assert!(!ws.exists());
        ws.save_question(&Question {
            market: "0xabc".into(),
            title: "US CPI YoY May".into(),
            resolution_criteria: "BLS first print".into(),
            lower_bound: Some(0.0),
            upper_bound: Some(10.0),
            created_at: now_unix(),
        })
        .unwrap();
        assert!(ws.exists());

        let id1 = ws
            .add_evidence(EvidenceItem {
                id: String::new(),
                captured_at: now_unix(),
                claim: "Gas prices fell".into(),
                source: Some("EIA".into()),
                url: None,
                stance: Stance::Down,
                reliability: Some(0.9),
                relevance: Some(0.7),
            })
            .unwrap();
        let id2 = ws
            .add_evidence(EvidenceItem {
                id: String::new(),
                captured_at: now_unix(),
                claim: "Shelter sticky".into(),
                source: Some("BLS".into()),
                url: None,
                stance: Stance::Up,
                reliability: Some(0.95),
                relevance: Some(0.9),
            })
            .unwrap();
        assert_eq!(id1, "e1");
        assert_eq!(id2, "e2");
        assert_eq!(ws.load_evidence().unwrap().len(), 2);

        ws.save_snapshot(&Snapshot {
            mean: 3.1,
            sd: 0.2,
            variance: 0.04,
            method: "log_odds_pool".into(),
            rationale: "Base rate plus disinflation".into(),
            reasons_up: vec!["shelter".into()],
            reasons_down: vec!["energy".into()],
            change_my_mind: vec!["surprise core".into()],
            created_at: now_unix(),
        })
        .unwrap();
        let snap = ws.load_snapshot().unwrap().unwrap();
        assert!((snap.mean - 3.1).abs() < 1e-9);

        assert_eq!(
            list_markets_in(tmp.path()).unwrap(),
            vec!["0xabc".to_owned()]
        );
    }
}
