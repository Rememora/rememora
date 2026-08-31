//! Fast deterministic retrieval eval (Tier 1).
//!
//! No agents, no processes, no network, no encryption. Loads a committed
//! corpus + golden query set into an in-memory DB and calls the ranking
//! functions directly. Designed to run in well under a second so ranking
//! changes can be iterated on in a tight loop.
//!
//!   cargo test --test eval_retrieval                 # gate (asserts floors)
//!   REMEMORA_EVAL_REPORT=1 cargo test --test eval_retrieval -- --nocapture
//!
//! Defaults to the committed synthetic fixtures (`corpus-synthetic.jsonl` +
//! `golden-synthetic.jsonl`), which are invented rather than exported, so the
//! floors below gate in CI rather than skipping.
//!
//! Fixture paths are overridable so an operator can point the same scorer at a
//! private corpus exported from their own DB (see bench/golden/build-corpus.sh):
//!   REMEMORA_EVAL_CORPUS=/path/corpus.jsonl
//!   REMEMORA_EVAL_GOLDEN=/path/golden.jsonl

use rusqlite::Connection;
use std::collections::HashSet;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bench/golden")
}

fn path_from_env(var: &str, default_name: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => fixture_dir().join(default_name),
    }
}

/// Resolve the corpus, defaulting to the committed synthetic fixture.
///
/// The synthetic corpus is entirely invented, so it can be committed and makes
/// these assertions actually gate in CI. A corpus built from a real DB contains
/// real memory text and is .gitignore'd; point at it with REMEMORA_EVAL_CORPUS
/// (plus REMEMORA_EVAL_GOLDEN) to score against your own memories.
///
/// Missing fixtures are a hard failure, not a skip. A test that silently skips
/// reports "ok" while measuring nothing, which is how the floors below sat
/// decorative — passing in 0.00s against an empty corpus.
fn corpus_or_skip() -> Option<PathBuf> {
    let p = path_from_env("REMEMORA_EVAL_CORPUS", "corpus-synthetic.jsonl");
    assert!(
        p.exists(),
        "eval corpus missing at {} — the synthetic fixture is committed and must be present",
        p.display()
    );
    Some(p)
}

struct GoldenCase {
    id: String,
    query: String,
    project: Option<String>,
    relevant: Vec<String>,
}

/// Load the corpus JSONL into a fresh in-memory DB with FTS5 populated.
fn load_corpus(path: &PathBuf) -> Connection {
    let conn = rememora::db::open_memory().expect("open_memory");
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("corpus {}: {e}", path.display()));

    let mut projects: HashSet<String> = HashSet::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("corpus row");
        let uri = v["uri"].as_str().expect("uri").to_string();
        if let Some(p) = uri
            .strip_prefix("rememora://projects/")
            .and_then(|r| r.split('/').next())
        {
            if projects.insert(p.to_string()) {
                let _ = rememora::models::project::add(
                    &conn,
                    p,
                    Some(&format!("/tmp/{p}")),
                    &format!("eval fixture: {p}"),
                    &[],
                );
            }
        }

        // Insert with the fixture's own id so goldens can reference stable ids.
        conn.execute(
            "INSERT INTO contexts (id, uri, parent_uri, context_type, category, name,
                                   abstract, overview, content, tags, source_agent,
                                   source_session, importance, active_count,
                                   created_at, updated_at)
             VALUES (?1, ?2, ?3, 'memory', ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, 0,
                     datetime('now'), datetime('now'))",
            rusqlite::params![
                v["id"].as_str().expect("id"),
                uri,
                v["parent_uri"].as_str(),
                v["category"].as_str(),
                v["name"].as_str().unwrap_or(""),
                v["abstract"].as_str().unwrap_or(""),
                v["overview"].as_str().unwrap_or(""),
                v["content"].as_str().unwrap_or(""),
                v["tags"].as_str().unwrap_or("[]"),
                v["source_agent"].as_str(),
                v["importance"].as_f64().unwrap_or(0.5),
            ],
        )
        .expect("insert corpus row");
    }
    conn
}

fn load_golden(path: &PathBuf) -> Vec<GoldenCase> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("golden {}: {e}", path.display()));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).expect("golden row");
            GoldenCase {
                id: v["id"].as_str().unwrap_or("?").to_string(),
                query: v["input"]["query"].as_str().expect("query").to_string(),
                project: v["metadata"]["project"].as_str().map(str::to_string),
                relevant: v["expected"]["relevant_ids"]
                    .as_array()
                    .expect("relevant_ids")
                    .iter()
                    .map(|x| x.as_str().unwrap().to_string())
                    .collect(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct Metrics {
    r_at_1: f64,
    r_at_3: f64,
    r_at_5: f64,
    r_at_10: f64,
    p_at_3: f64,
    mrr: f64,
    ndcg_at_10: f64,
    n: f64,
}

impl Metrics {
    fn add(&mut self, ranked: &[String], relevant: &[String]) {
        let rel: HashSet<&str> = relevant.iter().map(String::as_str).collect();
        let hits_at = |k: usize| {
            ranked
                .iter()
                .take(k)
                .filter(|d| rel.contains(d.as_str()))
                .count() as f64
        };
        let denom = rel.len().max(1) as f64;
        self.r_at_1 += hits_at(1) / denom;
        self.r_at_3 += hits_at(3) / denom;
        self.r_at_5 += hits_at(5) / denom;
        self.r_at_10 += hits_at(10) / denom;
        self.p_at_3 += hits_at(3) / 3.0;
        self.mrr += ranked
            .iter()
            .position(|d| rel.contains(d.as_str()))
            .map(|i| 1.0 / (i + 1) as f64)
            .unwrap_or(0.0);

        let dcg: f64 = ranked
            .iter()
            .take(10)
            .enumerate()
            .filter(|(_, d)| rel.contains(d.as_str()))
            .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())
            .sum();
        let idcg: f64 = (0..rel.len().min(10))
            .map(|i| 1.0 / ((i + 2) as f64).log2())
            .sum();
        self.ndcg_at_10 += if idcg > 0.0 { dcg / idcg } else { 0.0 };
        self.n += 1.0;
    }
    fn mean(&self) -> Metrics {
        let n = self.n.max(1.0);
        Metrics {
            r_at_1: self.r_at_1 / n,
            r_at_3: self.r_at_3 / n,
            r_at_5: self.r_at_5 / n,
            r_at_10: self.r_at_10 / n,
            p_at_3: self.p_at_3 / n,
            mrr: self.mrr / n,
            ndcg_at_10: self.ndcg_at_10 / n,
            n: self.n,
        }
    }
}

/// A ranking strategy under test. Adding a candidate ranker = adding an arm here.
type Arm = fn(&Connection, &GoldenCase) -> Vec<String>;

fn arm_bm25_project(conn: &Connection, g: &GoldenCase) -> Vec<String> {
    rememora::search::search(conn, &g.query, g.project.as_deref(), None, 10)
        .expect("search")
        .into_iter()
        .map(|r| r.context.id)
        .collect()
}

/// What the production hook actually sends when cwd is a git worktree:
/// `basename(cwd)` is not a registered project, so the project filter matches
/// nothing and only `rememora://global/%` survives.
fn arm_bm25_wrong_project(conn: &Connection, g: &GoldenCase) -> Vec<String> {
    rememora::search::search(conn, &g.query, Some("worktree-feat-x"), None, 10)
        .expect("search")
        .into_iter()
        .map(|r| r.context.id)
        .collect()
}

fn arm_bm25_no_project(conn: &Connection, g: &GoldenCase) -> Vec<String> {
    rememora::search::search(conn, &g.query, None, None, 10)
        .expect("search")
        .into_iter()
        .map(|r| r.context.id)
        .collect()
}

fn run_arm(conn: &Connection, golden: &[GoldenCase], arm: Arm) -> Metrics {
    run_arm_logged(conn, golden, arm, "", &mut Vec::new())
}

/// Score one arm, appending per-query rows in the Braintrust interchange shape
/// (`input` / `output` / `expected` / `scores` / `metadata` as objects) that
/// bench/src/scorer.ts already emits — so both tiers land in one dataset.
fn run_arm_logged(
    conn: &Connection,
    golden: &[GoldenCase],
    arm: Arm,
    arm_name: &str,
    rows: &mut Vec<String>,
) -> Metrics {
    let mut m = Metrics::default();
    for g in golden {
        let ranked = arm(conn, g);
        let mut one = Metrics::default();
        one.add(&ranked, &g.relevant);
        m.add(&ranked, &g.relevant);
        if !arm_name.is_empty() {
            rows.push(
                serde_json::json!({
                    "id": format!("{}/{}", arm_name, g.id),
                    "input": { "query": g.query, "project": g.project },
                    "output": { "ranked_ids": ranked },
                    "expected": { "relevant_ids": g.relevant },
                    "scores": {
                        "recall_at_3": one.r_at_3,
                        "recall_at_10": one.r_at_10,
                        "precision_at_3": one.p_at_3,
                        "mrr": one.mrr,
                        "ndcg_at_10": one.ndcg_at_10,
                    },
                    "metadata": { "tier": "retrieval", "arm": arm_name },
                })
                .to_string(),
            );
        }
    }
    m.mean()
}

/// Write the JSONL rows when `REMEMORA_EVAL_OUT` names a destination file.
fn write_rows(rows: &[String]) {
    if let Ok(p) = std::env::var("REMEMORA_EVAL_OUT") {
        if !p.is_empty() {
            let body = format!("{}\n", rows.join("\n"));
            if let Err(e) = std::fs::write(&p, body) {
                eprintln!("REMEMORA_EVAL_OUT write failed ({p}): {e}");
            } else {
                println!("wrote {} rows -> {p}", rows.len());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn retrieval_quality_meets_floor() {
    let Some(corpus) = corpus_or_skip() else { return };
    let t0 = std::time::Instant::now();
    let conn = load_corpus(&corpus);
    let golden = load_golden(&path_from_env("REMEMORA_EVAL_GOLDEN", "golden-synthetic.jsonl"));
    let loaded = t0.elapsed();

    let arms: &[(&str, Arm)] = &[
        ("bm25 (correct project)", arm_bm25_project),
        ("bm25 (no project filter)", arm_bm25_no_project),
        ("bm25 (worktree basename)", arm_bm25_wrong_project),
    ];

    let t1 = std::time::Instant::now();
    let mut baseline = Metrics::default();
    let mut rows: Vec<String> = Vec::new();
    for (i, (name, arm)) in arms.iter().enumerate() {
        let m = run_arm_logged(&conn, &golden, *arm, name, &mut rows);
        if i == 0 {
            baseline = m;
        }
        println!(
            "{name:28}  n={:>3}  R@1={:.3} R@3={:.3} R@5={:.3} R@10={:.3}  P@3={:.3}  MRR={:.3}  nDCG@10={:.3}",
            m.n as i64, m.r_at_1, m.r_at_3, m.r_at_5, m.r_at_10, m.p_at_3, m.mrr, m.ndcg_at_10
        );
    }
    println!(
        "\nload={:.1}ms  score={:.1}ms  ({:.2}ms/query/arm)",
        loaded.as_secs_f64() * 1000.0,
        t1.elapsed().as_secs_f64() * 1000.0,
        t1.elapsed().as_secs_f64() * 1000.0 / (golden.len() * arms.len()) as f64
    );
    write_rows(&rows);

    // Regression floors — raise these deliberately when a ranking change lands.
    assert!(
        baseline.r_at_3 >= 0.60,
        "recall@3 regressed: {:.3} < 0.60",
        baseline.r_at_3
    );
    assert!(
        baseline.mrr >= 0.70,
        "MRR regressed: {:.3} < 0.70",
        baseline.mrr
    );
}

/// The failure the founder actually experiences: from a git worktree the hook
/// derives a project name that does not exist, the filter drops every project
/// memory, and three irrelevant globals get injected instead.
#[test]
fn worktree_basename_project_destroys_precision() {
    let Some(corpus) = corpus_or_skip() else { return };
    let conn = load_corpus(&corpus);
    let golden = load_golden(&path_from_env("REMEMORA_EVAL_GOLDEN", "golden-synthetic.jsonl"));
    let good = run_arm(&conn, &golden, arm_bm25_project);
    let bad = run_arm(&conn, &golden, arm_bm25_wrong_project);
    assert!(
        bad.r_at_3 < good.r_at_3 * 0.5,
        "expected an unregistered project name to collapse recall (good={:.3} bad={:.3})",
        good.r_at_3,
        bad.r_at_3
    );
}
