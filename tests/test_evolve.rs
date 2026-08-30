mod common;

use rememora::models::context::{self, InsertContext};

fn insert_memory(
    conn: &rusqlite::Connection,
    project: &str,
    category: &str,
    slug: &str,
    name: &str,
    content: &str,
    importance: f64,
) -> String {
    let uri = format!("rememora://projects/{project}/memories/{category}/{slug}");
    let parent = format!("rememora://projects/{project}/memories/{category}");
    context::insert(
        conn,
        &InsertContext {
            uri,
            parent_uri: Some(parent),
            context_type: "memory".into(),
            category: Some(category.into()),
            name: name.into(),
            abstract_text: name.into(),
            overview: content.into(),
            content: content.into(),
            tags: "[]".into(),
            source_agent: Some("claude-code".into()),
            source_session: None,
            importance,
        },
    )
    .unwrap()
}

/// Filler memories that give BM25 a corpus large enough for a term shared by
/// two documents to carry positive IDF. See
/// `test_tiny_corpora_produce_no_clusters` for why this matters.
fn insert_filler(conn: &rusqlite::Connection, project: &str) {
    let filler: [(&str, &str, &str); 5] = [
        ("tailwind", "Adopt Tailwind for styling", "Tailwind replaced the hand-written CSS modules."),
        ("fly", "Deploy on Fly for hosting", "Fly runs the production containers in two regions."),
        ("node-pin", "Pin Node twenty for builds", "CI pins Node twenty so builds are reproducible."),
        ("sops", "Encrypt secrets with SOPS", "SOPS with age keys guards every secret file."),
        ("ratelimit", "Rate limit the public endpoint", "A token bucket guards the public endpoint."),
    ];
    for (slug, name, content) in &filler {
        insert_memory(conn, project, "decision", slug, name, content, 0.4);
    }
}

#[test]
fn test_similar_memories_cluster_together() {
    let conn = common::create_test_db();

    // Register project
    rememora::models::project::add(
        &conn,
        "myapp",
        Some("/tmp/myapp"),
        "Test app",
        &["rust".into()],
    )
    .unwrap();

    // Insert two very similar memories about Zustand
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "use-zustand",
        "Use Zustand for state management",
        "We chose Zustand for state management due to minimal boilerplate and simpler API.",
        0.8,
    );
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "zustand-over-redux",
        "Zustand chosen over Redux for state",
        "After evaluating Redux and Zustand, we picked Zustand for state management. Less boilerplate.",
        0.7,
    );

    // Insert an unrelated memory in the same category
    insert_memory(
        &conn,
        "myapp",
        "decision",
        "use-postgres",
        "Use PostgreSQL for the database",
        "PostgreSQL was chosen as the primary database for its reliability and JSON support.",
        0.6,
    );

    insert_filler(&conn, "myapp");

    // Load all memories and run cluster detection
    let memories = context::list_by_scope(&conn, Some("memory"), None, Some("myapp"), 100).unwrap();
    assert_eq!(memories.len(), 8);

    // The CLI default. The original test used 0.05 because the broken mapping
    // scored genuine duplicates at ~0.17; on the relative scale they score
    // 0.5-1.0 and the default threshold is what should be exercised.
    let clusters = rememora::evolve::find_clusters(&conn, memories, 0.3).unwrap();

    // We should find at least one cluster containing the two Zustand memories
    assert!(
        !clusters.is_empty(),
        "Expected at least one cluster for similar Zustand memories"
    );

    // The Zustand memories should be in the same cluster
    let zustand_cluster = clusters.iter().find(|c| {
        c.memories
            .iter()
            .any(|m| m.name.contains("Zustand") || m.name.contains("zustand"))
    });
    assert!(
        zustand_cluster.is_some(),
        "Expected a cluster containing Zustand memories"
    );

    let zustand_cluster = zustand_cluster.unwrap();
    assert!(
        zustand_cluster.memories.len() >= 2,
        "Zustand cluster should have at least 2 memories, got {}",
        zustand_cluster.memories.len()
    );

    // The assertion the original test was missing: on the old mapping the
    // unrelated PostgreSQL memory landed in this same cluster, because it
    // shared nothing but stopword-tier terms and those scored ~1.0.
    assert!(
        !zustand_cluster
            .memories
            .iter()
            .any(|m| m.name.contains("PostgreSQL")),
        "unrelated memory must not be dragged into the Zustand cluster"
    );
}

/// BM25 gives a term that appears in more than half the documents a
/// non-positive IDF, which FTS5 clamps to 1e-6. In a three-document category
/// the very term that signals a duplicate ("Zustand", in 2 of 3 documents) is
/// clamped, so *every* rank in that corpus collapses to ~1e-6 and carries no
/// information at all.
///
/// Clustering there is guesswork, and this pipeline's output is irreversible
/// supersession, so the correct behaviour is to find nothing. The old mapping
/// did the opposite: it scored those collapsed ranks ~1.0 and clustered all
/// three memories — including the unrelated one — into a single cluster.
#[test]
fn test_tiny_corpora_produce_no_clusters() {
    let conn = common::create_test_db();
    rememora::models::project::add(&conn, "tiny", Some("/tmp/tiny"), "Tiny", &[]).unwrap();

    insert_memory(
        &conn,
        "tiny",
        "decision",
        "use-zustand",
        "Use Zustand for state management",
        "We chose Zustand for state management due to minimal boilerplate and simpler API.",
        0.8,
    );
    insert_memory(
        &conn,
        "tiny",
        "decision",
        "zustand-over-redux",
        "Zustand chosen over Redux for state",
        "After evaluating Redux and Zustand, we picked Zustand for state management.",
        0.7,
    );
    insert_memory(
        &conn,
        "tiny",
        "decision",
        "use-postgres",
        "Use PostgreSQL for the database",
        "PostgreSQL was chosen as the primary database for its reliability.",
        0.6,
    );

    let memories = context::list_by_scope(&conn, Some("memory"), None, Some("tiny"), 100).unwrap();
    let clusters = rememora::evolve::find_clusters(&conn, memories, 0.05).unwrap();

    assert!(
        clusters.is_empty(),
        "a corpus BM25 cannot discriminate must yield no clusters, got {:?}",
        clusters
            .iter()
            .map(|c| c.memories.iter().map(|m| &m.name).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_superseded_memories_excluded() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "myapp2",
        Some("/tmp/myapp2"),
        "Test app 2",
        &["rust".into()],
    )
    .unwrap();

    let id1 = insert_memory(
        &conn,
        "myapp2",
        "decision",
        "old-state",
        "Use Redux for state management",
        "Redux was chosen for state management.",
        0.7,
    );
    let id2 = insert_memory(
        &conn,
        "myapp2",
        "decision",
        "new-state",
        "Use Zustand for state management",
        "Switched from Redux to Zustand for state management.",
        0.8,
    );

    // Supersede old memory
    context::supersede(&conn, &id1, &id2).unwrap();

    // Load active (non-superseded) memories
    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("myapp2"), 100).unwrap();
    assert_eq!(memories.len(), 1, "Only the non-superseded memory should load");
    assert_eq!(memories[0].id, id2);
}

#[test]
fn test_no_clusters_with_single_memory() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "solo",
        Some("/tmp/solo"),
        "Solo project",
        &["rust".into()],
    )
    .unwrap();

    insert_memory(
        &conn,
        "solo",
        "entity",
        "only-one",
        "The only entity memory",
        "This is the only entity memory in the project.",
        0.5,
    );

    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("solo"), 100).unwrap();
    assert_eq!(memories.len(), 1);

    let clusters =
        rememora::evolve::find_clusters(&conn, memories, 0.05).unwrap();
    assert!(
        clusters.is_empty(),
        "Should not find clusters with only one memory"
    );
}

/// `min_similarity` has to actually move the needle.
///
/// The version of this test that shipped with the sign fix asserted only
/// `high_total <= low_total` over a three-document corpus. Under the relative
/// mapping BM25 cannot discriminate a corpus that small at all (see
/// `test_tiny_corpora_produce_no_clusters`), so both sides were 0 and the
/// assertion held for the wrong reason: it would have passed with clustering
/// deleted. This version gives BM25 a corpus it can work in and pins both ends
/// — the low threshold must find the Stripe pair, the high threshold must find
/// nothing — so a mapping that ignored the threshold would fail it.
#[test]
fn test_threshold_controls_how_much_clusters() {
    let conn = common::create_test_db();

    rememora::models::project::add(
        &conn,
        "varied",
        Some("/tmp/varied"),
        "Varied project",
        &["rust".into()],
    )
    .unwrap();

    // Two similar (Stripe-related) memories and one dissimilar...
    insert_memory(
        &conn,
        "varied",
        "decision",
        "stripe-api",
        "Stripe API integration for payments",
        "The project uses Stripe API for processing credit card payments with idempotency keys.",
        0.7,
    );
    insert_memory(
        &conn,
        "varied",
        "decision",
        "stripe-webhooks",
        "Stripe webhook handling for payments",
        "Stripe webhooks are used to handle payment success and failure events asynchronously.",
        0.7,
    );
    insert_memory(
        &conn,
        "varied",
        "decision",
        "kubernetes-deploy",
        "Kubernetes deployment for production",
        "Production deployments use Kubernetes with Helm charts and ArgoCD for GitOps.",
        0.6,
    );
    // ...plus enough unrelated documents for the shared terms to carry IDF.
    insert_filler(&conn, "varied");

    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("varied"), 100).unwrap();
    assert_eq!(memories.len(), 8);

    let low = rememora::evolve::find_clusters(&conn, memories.clone(), 0.3).unwrap();
    let high = rememora::evolve::find_clusters(&conn, memories, 0.99).unwrap();

    let low_names: Vec<Vec<&str>> = low
        .iter()
        .map(|c| c.memories.iter().map(|m| m.name.as_str()).collect())
        .collect();
    let high_total: usize = high.iter().map(|c| c.memories.len()).sum();

    assert_eq!(
        low.len(),
        1,
        "the default threshold must find exactly the Stripe pair, got {low_names:?}"
    );
    let mut clustered = low_names[0].clone();
    clustered.sort_unstable();
    assert_eq!(
        clustered,
        vec![
            "Stripe API integration for payments",
            "Stripe webhook handling for payments",
        ],
        "the only cluster must be the two Stripe memories"
    );

    assert_eq!(
        high_total, 0,
        "a near-perfect-match threshold must cluster nothing in this corpus"
    );
}

/// Regression test for the inverted BM25 mapping.
///
/// Four of these six memories share only the word "configuration", which
/// appears in more than half the corpus — FTS5 clamps its IDF, so they match
/// each other at rank ~-1.4e-6. Two of them are genuine near-duplicates about
/// Zustand and match at rank ~-4.96 against a self-match of ~-6.02.
///
/// The old mapping `1 / (1 + |rank|)` scored the stopword pairs 1.0000 and the
/// real duplicates 0.1677, so at the CLI's default `--min-similarity 0.3` it
/// clustered the four unrelated configuration memories together and left the
/// two real duplicates apart. The relative mapping must do exactly the
/// opposite.
#[test]
fn test_stopword_matches_do_not_cluster_but_real_duplicates_do() {
    const CLI_DEFAULT_MIN_SIMILARITY: f64 = 0.3;

    let conn = common::create_test_db();
    rememora::models::project::add(&conn, "stop", Some("/tmp/stop"), "Stopword corpus", &[])
        .unwrap();

    let corpus: [(&str, &str, &str); 6] = [
        (
            "use-zustand",
            "Use Zustand for state management",
            "We chose Zustand for state management due to minimal boilerplate.",
        ),
        (
            "zustand-redux",
            "Zustand chosen over Redux for state management",
            "After evaluating Redux and Zustand, we picked Zustand for state management.",
        ),
        (
            "k8s",
            "Kubernetes deployment configuration",
            "Production deployments use Kubernetes with Helm charts.",
        ),
        (
            "webpack",
            "Webpack build configuration",
            "Webpack bundles the frontend assets.",
        ),
        (
            "nginx",
            "Nginx proxy configuration",
            "Nginx terminates TLS and proxies to the API.",
        ),
        (
            "eslint",
            "ESLint lint configuration",
            "ESLint enforces the lint rules in CI.",
        ),
    ];
    for (slug, name, content) in &corpus {
        insert_memory(&conn, "stop", "decision", slug, name, content, 0.5);
    }

    let memories =
        context::list_by_scope(&conn, Some("memory"), None, Some("stop"), 100).unwrap();
    let clusters =
        rememora::evolve::find_clusters(&conn, memories, CLI_DEFAULT_MIN_SIMILARITY).unwrap();

    let names: Vec<Vec<&str>> = clusters
        .iter()
        .map(|c| c.memories.iter().map(|m| m.name.as_str()).collect())
        .collect();

    assert_eq!(
        clusters.len(),
        1,
        "expected only the Zustand pair to cluster, got {names:?}"
    );

    let mut clustered = names[0].clone();
    clustered.sort_unstable();
    assert_eq!(
        clustered,
        vec![
            "Use Zustand for state management",
            "Zustand chosen over Redux for state management",
        ],
        "the only cluster must be the two real duplicates"
    );

    assert!(
        !names
            .iter()
            .flatten()
            .any(|n| n.contains("configuration")),
        "memories sharing only a stopword-tier term must not cluster: {names:?}"
    );
}

/// The similarity scale is relative to the query's own self-match, so a raw
/// BM25 rank on its own says nothing.
#[test]
fn test_bm25_similarity_is_relative_not_absolute() {
    use rememora::evolve::bm25_similarity;

    // Measured ranks from the corpus above.
    let strong = bm25_similarity(-4.96, -6.02);
    let stopword = bm25_similarity(-0.0000015, -5.04);

    assert!(strong > 0.8, "strong match scored {strong}");
    assert_eq!(stopword, 0.0, "stopword-tier match scored {stopword}");

    // The old mapping did the reverse.
    let old = |rank: f64| 1.0 / (1.0 + rank.abs());
    assert!(old(-4.96) < old(-0.0000015));
}

/// One consolidation decision can never retire more than the cap, whatever
/// the model asks for, and an over-cap ask is rejected whole.
#[test]
fn test_supersede_cap_bounds_a_single_decision() {
    use rememora::evolve::{
        validate_fold_ids, validate_supersede_plan, MemoryCluster, MAX_SUPERSEDE_PER_DECISION,
    };

    let conn = common::create_test_db();
    rememora::models::project::add(&conn, "cap", Some("/tmp/cap"), "Cap", &[]).unwrap();

    let n = MAX_SUPERSEDE_PER_DECISION + 3;
    for i in 0..n {
        insert_memory(
            &conn,
            "cap",
            "decision",
            &format!("m{i}"),
            &format!("Decision number {i}"),
            &format!("Body of decision {i}"),
            0.5,
        );
    }
    let memories = context::list_by_scope(&conn, Some("memory"), None, Some("cap"), 100).unwrap();
    assert_eq!(memories.len(), n);

    let ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
    let cluster = MemoryCluster { memories };

    // At the cap: allowed.
    assert!(validate_fold_ids(&cluster, &ids[..MAX_SUPERSEDE_PER_DECISION]).is_ok());

    // One over the cap: refused, and refused as a whole rather than trimmed.
    let err = validate_fold_ids(&cluster, &ids[..MAX_SUPERSEDE_PER_DECISION + 1])
        .expect_err("over-cap fold must be rejected");
    assert!(err.to_string().contains("cap"), "unexpected error: {err}");

    // The same cap applies to the keep/supersede shape.
    assert!(validate_supersede_plan(&cluster, &ids[0], &ids[1..]).is_err());
    assert!(
        validate_supersede_plan(&cluster, &ids[0], &ids[1..=MAX_SUPERSEDE_PER_DECISION]).is_ok()
    );
}

/// Clusters big enough to be a clustering accident are withheld from the
/// automated path entirely.
#[test]
fn test_oversized_clusters_are_flagged() {
    use rememora::evolve::{is_oversized, MemoryCluster, MAX_CLUSTER_SIZE};

    let conn = common::create_test_db();
    rememora::models::project::add(&conn, "big", Some("/tmp/big"), "Big", &[]).unwrap();

    for i in 0..(MAX_CLUSTER_SIZE + 1) {
        insert_memory(
            &conn,
            "big",
            "decision",
            &format!("b{i}"),
            &format!("Decision number {i}"),
            &format!("Body of decision {i}"),
            0.5,
        );
    }
    let memories = context::list_by_scope(&conn, Some("memory"), None, Some("big"), 100).unwrap();

    let at_limit = MemoryCluster {
        memories: memories[..MAX_CLUSTER_SIZE].to_vec(),
    };
    let over_limit = MemoryCluster {
        memories: memories.clone(),
    };

    assert!(!is_oversized(&at_limit));
    assert!(is_oversized(&over_limit));
}

// --- Read-only mode -------------------------------------------------------
//
// `consolidate` hands its clusters to a Claude Code subagent that has Bash and
// runs `rememora` itself, so none of the caps, cluster-membership checks,
// transactions or journalling in `commands/evolve.rs` apply to anything that
// subagent does. Rather than advertise safety it does not have, `consolidate`
// no longer writes at all — and it backs that up by setting
// `REMEMORA_READONLY=1` in the subagent's environment, which the CLI refuses to
// write under.
//
// These drive the real binary in a subprocess, because the guard is only worth
// anything if it survives the process boundary the subagent sits behind.

/// Build a scratch database the CLI can open, seeded with one memory.
///
/// Unencrypted and inside a tempdir, so nothing here can reach `~/.rememora`.
fn scratch_db(dir: &std::path::Path) -> (std::path::PathBuf, String) {
    let db_path = dir.join("scratch.db");
    let conn = rememora::db::open_with_options(&db_path, true).unwrap();
    rememora::models::project::add(&conn, "ro", Some("/tmp/ro"), "Read-only", &[]).unwrap();
    let id = insert_memory(
        &conn,
        "ro",
        "decision",
        "use-zustand",
        "Use Zustand for state management",
        "We chose Zustand for state management due to minimal boilerplate.",
        0.8,
    );
    (db_path, id)
}

fn cli(db_path: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::cargo_bin("rememora").expect("binary built");
    cmd.env("REMEMORA_DB", db_path)
        .env("REMEMORA_READONLY", "1")
        .arg("--no-encryption");
    cmd
}

fn unguarded_cli(db_path: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = assert_cmd::Command::cargo_bin("rememora").expect("binary built");
    cmd.env("REMEMORA_DB", db_path)
        .env_remove("REMEMORA_READONLY")
        .arg("--no-encryption");
    cmd
}

fn memory_count(db_path: &std::path::Path) -> usize {
    let conn = rememora::db::open_with_options(db_path, true).unwrap();
    context::list_by_scope(&conn, Some("memory"), None, Some("ro"), 100)
        .unwrap()
        .len()
}

#[test]
fn readonly_mode_refuses_writes_from_a_subprocess() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, id) = scratch_db(dir.path());
    let before = memory_count(&db_path);

    cli(&db_path)
        .args([
            "save",
            "a brand new fact",
            "--category",
            "decision",
            "--project",
            "ro",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("read-only mode"));

    cli(&db_path)
        .args(["supersede", &id, "--by", &id])
        .assert()
        .failure()
        .stderr(predicates::str::contains("read-only mode"));

    assert_eq!(
        memory_count(&db_path),
        before,
        "read-only mode must leave the store untouched"
    );
}

#[test]
fn readonly_mode_still_allows_the_subagent_to_search() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, _) = scratch_db(dir.path());

    cli(&db_path)
        .args(["search", "Zustand", "--project", "ro"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Zustand"));
}

/// Without the guard armed the same write goes through, so the test above is
/// measuring the guard and not some unrelated failure.
#[test]
fn writes_succeed_when_readonly_mode_is_not_armed() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, _) = scratch_db(dir.path());
    let before = memory_count(&db_path);

    unguarded_cli(&db_path)
        .args([
            "save",
            "a brand new fact",
            "--category",
            "decision",
            "--project",
            "ro",
        ])
        .assert()
        .success();

    assert_eq!(memory_count(&db_path), before + 1);
}

/// `consolidate --apply` is refused outright rather than quietly proposing.
#[test]
fn consolidate_apply_is_refused_and_points_at_evolve() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, _) = scratch_db(dir.path());

    unguarded_cli(&db_path)
        .args(["consolidate", "--project", "ro", "--apply"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "rememora evolve --project ro --apply",
        ));
}

/// End-to-end: the subagent `consolidate` spawns cannot write, even when it
/// tries to.
///
/// This is the claim the branch has to earn. `consolidate` delegates its writes
/// to a Claude Code subagent, so the caps and journalling in
/// `commands/evolve.rs` never see them; the guard has to hold at the process
/// boundary instead. A stub `claude` on PATH stands in for the subagent and
/// does exactly what the consolidation prompt used to tell it to do — shell out
/// to `rememora` and retire a memory.
#[test]
fn a_consolidate_subagent_cannot_write_memories() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, _) = scratch_db(dir.path());
    // Enough of a corpus that clustering finds something to consolidate.
    {
        let conn = rememora::db::open_with_options(&db_path, true).unwrap();
        insert_memory(
            &conn,
            "ro",
            "decision",
            "zustand-redux",
            "Zustand chosen over Redux for state management",
            "After evaluating Redux and Zustand, we picked Zustand for state management.",
            0.7,
        );
        insert_filler(&conn, "ro");
    }
    let before = memory_count(&db_path);

    let bin_dir = dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let probe = dir.path().join("probe.txt");
    let stub = bin_dir.join("claude");
    std::fs::write(
        &stub,
        "#!/bin/sh\n\
         # Stand in for the consolidation subagent: try to retire a memory.\n\
         \"$REMEMORA_BIN\" --no-encryption save \"subagent wrote this\" \
         --category decision --project ro >\"$REMEMORA_PROBE\" 2>&1\n\
         echo \"exit=$?\" >>\"$REMEMORA_PROBE\"\n\
         echo '{\"type\":\"result\",\"result\":\"proposal only\"}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub, perms).unwrap();
    }

    unguarded_cli(&db_path)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("REMEMORA_BIN", assert_cmd::cargo::cargo_bin("rememora"))
        .env("REMEMORA_PROBE", &probe)
        .args(["consolidate", "--project", "ro"])
        .assert()
        .success()
        .stdout(predicates::str::contains("never writes to the database"));

    let attempt = std::fs::read_to_string(&probe).expect("the stub subagent must have run");
    assert!(
        attempt.contains("read-only mode"),
        "the subagent's write should have been refused, got: {attempt}"
    );
    assert!(
        !attempt.contains("exit=0"),
        "the subagent's write should have failed, got: {attempt}"
    );
    assert_eq!(
        memory_count(&db_path),
        before,
        "a consolidate run must not change the store"
    );
}

/// The undo journal is readable back out of the encrypted database — it is not
/// a file the user has to know about, and there is no cleartext sidecar.
#[test]
fn undo_log_reads_from_the_database_and_writes_no_sidecar() {
    let dir = tempfile::TempDir::new().unwrap();
    let (db_path, _) = scratch_db(dir.path());

    unguarded_cli(&db_path)
        .args(["evolve", "--project", "ro", "--undo-log"])
        .assert()
        .success()
        .stdout(predicates::str::contains("undo journal is empty"));

    assert!(
        !dir.path().join("evolve-undo").exists(),
        "the journal must live in the database, not in a cleartext sidecar"
    );
}
