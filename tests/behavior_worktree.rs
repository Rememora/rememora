//! Behavior tests: project resolution across git worktrees.
//!
//! Agents do their work in linked worktrees. Every write path used to name the
//! project after whatever directory it was standing in, so a worktree produced
//! a project name matching no registered project — and because the project
//! filter is a hard `uri LIKE 'rememora://projects/<name>/%'` prefix match,
//! those memories were unreachable from the moment they were written.
//!
//! These tests drive `project::resolve_write_target` and `project::reconcile`
//! against **real** git worktrees rather than a mock, because the whole fix
//! rests on what `git rev-parse --git-common-dir` reports inside a linked
//! worktree. A fake would test the mock.

mod scenarios;

use std::path::{Path, PathBuf};
use std::process::Command;

use rememora::models::context;
use rememora::models::project;
use rusqlite::Connection;
use scenarios::{memory, session};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be on PATH for these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Repo {
    /// Held so the directory outlives the test.
    _tmp: tempfile::TempDir,
    main: PathBuf,
    worktree: PathBuf,
}

impl Repo {
    fn main_str(&self) -> &str {
        self.main.to_str().unwrap()
    }
    fn worktree_str(&self) -> &str {
        self.worktree.to_str().unwrap()
    }
}

/// A git repo checked out at `<tmp>/<name>` with a linked worktree at
/// `<tmp>/<worktree>` on branch `feature/x`.
fn repo_with_worktree(name: &str, worktree: &str) -> Repo {
    // Hyphen-free so the encoded-path tests can round-trip a path inside it.
    let tmp = hyphen_free_tempdir();
    let main = tmp.path().join(name);
    std::fs::create_dir_all(&main).unwrap();

    git(&main, &["init", "-q", "-b", "main"]);
    // A worktree cannot be added to a repo with no commits, and the identity
    // must be local so the test does not depend on the developer's gitconfig.
    git(&main, &["config", "user.email", "test@example.com"]);
    git(&main, &["config", "user.name", "Test"]);
    git(&main, &["config", "commit.gpgsign", "false"]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let wt = tmp.path().join(worktree);
    git(
        &main,
        &["worktree", "add", "-q", "-b", "feature/x", wt.to_str().unwrap()],
    );

    Repo {
        _tmp: tmp,
        main,
        worktree: wt,
    }
}

/// Claude Code's transcript-directory encoding: both `/` and `.` become `-`.
fn encode_path(path: &str) -> String {
    path.replace(['/', '.'], "-")
}

fn db() -> Connection {
    rememora::db::open_memory().unwrap()
}

/// A tempdir guaranteed to contain no `-` in its path.
///
/// The Claude Code path encoding maps both `/` and `.` to `-`, so it cannot
/// round-trip a real path component that already contains one. The two
/// encoded-path tests below used to `return` early when the ambient tempdir
/// happened to have a hyphen — asserting nothing while still reporting as
/// passed, which would have hidden the exact rung an earlier draft got wrong.
/// Rooting the fixture in `/tmp` with a hyphen-free prefix removes the
/// conditional entirely; the assert fires loudly if that assumption ever breaks.
fn hyphen_free_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("rmwt")
        .tempdir_in("/tmp")
        .expect("create fixture dir under /tmp");
    assert!(
        !dir.path().to_str().unwrap().contains('-'),
        "fixture root must be hyphen-free for the path encoding to round-trip, got {}",
        dir.path().display()
    );
    dir
}

fn register(conn: &Connection, name: &str, path: &str) {
    project::add(conn, name, Some(path), "test project", &[]).unwrap();
}

// ---------------------------------------------------------------------------
// The resolution ladder
// ---------------------------------------------------------------------------

#[test]
fn a_worktree_write_lands_in_the_main_checkouts_project() {
    // Given: `myapp` is registered at its main checkout, and an agent is
    // working in a linked worktree, passing the worktree's own directory name
    // as --project (what every hook and agent instruction used to produce).
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let target = project::resolve_write_target(&conn, Some("wt-brave-meadow"), repo.worktree_str());

    assert_eq!(
        target.project.as_deref(),
        Some("myapp"),
        "a fabricated worktree name must fold onto the main checkout's project"
    );
}

#[test]
fn a_worktree_write_records_which_worktree_and_branch_it_came_from() {
    // Folding the project onto the main checkout loses which tree produced the
    // memory. The provenance columns are what keep it.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let target = project::resolve_write_target(&conn, Some("wt-brave-meadow"), repo.worktree_str());

    assert_eq!(target.worktree.as_deref(), Some("wt-brave-meadow"));
    assert_eq!(target.branch.as_deref(), Some("feature/x"));
}

#[test]
fn a_write_from_the_main_checkout_has_no_worktree() {
    // NULL means "written from the main checkout" — a real answer, not a
    // missing one. Tests that we do not stamp every write with a worktree.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let target = project::resolve_write_target(&conn, Some("myapp"), repo.main_str());

    assert_eq!(target.project.as_deref(), Some("myapp"));
    assert_eq!(
        target.worktree, None,
        "the main checkout is not a worktree and must not be labelled as one"
    );
}

#[test]
fn an_explicit_registered_project_survives_a_worktree() {
    // The one rule that can name a project the working directory disagrees
    // with: saving an `otherapp` memory while sitting in the `myapp` worktree
    // must not be rewritten to `myapp`.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    register(&conn, "otherapp", "/nonexistent/otherapp");

    let target = project::resolve_write_target(&conn, Some("otherapp"), repo.worktree_str());

    assert_eq!(
        target.project.as_deref(),
        Some("otherapp"),
        "a deliberate cross-project save must be honored"
    );
    assert_eq!(
        target.worktree.as_deref(),
        Some("wt-brave-meadow"),
        "provenance still describes where the write happened"
    );
}

#[test]
fn a_project_name_is_matched_case_insensitively() {
    // A real store accumulated both `ana` and `Ana` as separate URI
    // namespaces — two homes for one project's memories.
    let conn = db();
    register(&conn, "ana", "/nonexistent/ana");

    let target = project::resolve_write_target(&conn, Some("Ana"), "/nonexistent");

    assert_eq!(target.project.as_deref(), Some("ana"));
}

#[test]
fn an_unscoped_save_stays_global() {
    // Agents are instructed to save global preferences with no --project.
    // Auto-namespacing those would silently reclassify them.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let target = project::resolve_write_target(&conn, None, repo.worktree_str());

    assert_eq!(
        target.project, None,
        "omitting --project means global scope, worktree or not"
    );
}

#[test]
fn an_unregistered_project_outside_git_is_taken_at_its_word() {
    // Nothing to resolve against — a genuinely new project should not be
    // mangled into something else.
    let conn = db();
    let tmp = tempfile::tempdir().unwrap();

    let target = project::resolve_write_target(&conn, Some("brand-new"), tmp.path().to_str().unwrap());

    assert_eq!(target.project.as_deref(), Some("brand-new"));
}

#[test]
fn an_unregistered_worktree_still_resolves_to_the_main_checkout_name() {
    // Even with no project registered, the answer must be the main checkout's
    // name — that one survives the worktree being deleted.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();

    let target = project::resolve_write_target(&conn, Some("wt-brave-meadow"), repo.worktree_str());

    assert_eq!(target.project.as_deref(), Some("myapp"));
}

// ---------------------------------------------------------------------------
// Never invent a namespace
//
// The failure mode this whole change exists to remove is a project name that
// matches nothing. A resolver that guesses is worse than one that gives up,
// because `reconcile` writes its guesses to disk.
// ---------------------------------------------------------------------------

#[test]
fn an_unresolvable_encoded_path_does_not_invent_a_project() {
    // `/nonexistent/deep/deleted-thing` is gone. Walking prefixes back until
    // *something* exists always succeeds eventually — on a real machine it
    // lands on `/Users/<you>` or `/Users/<you>/Projects` and mints a project
    // called `ovidb` or `Projects`, which then collides across every unrelated
    // repo beneath it. Giving up is the only correct answer.
    let conn = db();
    let tmp = tempfile::tempdir().unwrap();
    let encoded = "-nonexistent-deep-deleted-thing";

    let target = project::resolve_write_target(&conn, Some(encoded), tmp.path().to_str().unwrap());

    assert_eq!(
        target.project.as_deref(),
        Some(encoded),
        "an unresolvable encoded path must fall through untouched, never resolve \
         to a generic ancestor directory's name"
    );
}

#[test]
fn a_dot_directory_in_an_encoded_path_still_decodes() {
    // Claude Code maps both `/` and `.` to `-`, so `~/.claude/worktrees/x`
    // encodes with an empty segment. Rebuilding that as `//` (which POSIX
    // collapses) meant the path never existed and the walk fell through to a
    // generic ancestor — the exact fabrication above.
    let repo = repo_with_worktree("myapp", "unused");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let dotted = repo.main.join(".agents").join("wt");
    std::fs::create_dir_all(&dotted).unwrap();
    let encoded = encode_path(dotted.to_str().unwrap());

    let target = project::resolve_write_target(&conn, Some(&encoded), "/");

    assert_eq!(
        target.project.as_deref(),
        Some("myapp"),
        "the dot-directory segment must decode, not collapse"
    );
}

#[test]
fn an_encoded_path_resolves_to_an_unregistered_repo_but_not_to_its_container() {
    // "Is a git working-tree root" is the line between a project and a
    // container. A repo nobody registered is still a real project; `~/Projects`
    // is not, however many repos live under it.
    let tmp = hyphen_free_tempdir();
    let container = tmp.path().join("container");
    let repo = container.join("myrepo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let conn = db();
    let encode = |p: &std::path::Path| encode_path(p.to_str().unwrap());

    // Overshoots into a path that no longer exists, but lands on a repo root.
    let into_repo = encode(&repo.join("gone"));
    let resolved = project::resolve_write_target(&conn, Some(&into_repo), "/");
    assert_eq!(resolved.project.as_deref(), Some("myrepo"));

    // Overshoots into a path under a plain container directory. There is no
    // repo to land on, so it must give up rather than answer "container".
    let into_container = encode(&container.join("gone"));
    let unresolved = project::resolve_write_target(&conn, Some(&into_container), "/");
    assert_eq!(
        unresolved.project.as_deref(),
        Some(into_container.as_str()),
        "a container directory is not a project"
    );
}

#[test]
fn an_explicit_unregistered_project_is_not_hijacked_by_the_cwd() {
    // The documented workflow is "save first, `project add` later". If the cwd
    // could override any name that is not yet registered, then `--project ana`
    // from inside the myapp worktree would write ana's memory into myapp,
    // `search --project ana` would return myapp's memories, and `evolve
    // --project ana` would consolidate myapp's. Only a name the *tooling*
    // synthesised may be overridden.
    let repo = repo_with_worktree("myapp", "wt-brave-meadow");
    let conn = db();
    register(&conn, "myapp", repo.main_str());

    let target = project::resolve_write_target(&conn, Some("ana"), repo.worktree_str());

    assert_eq!(
        target.project.as_deref(),
        Some("ana"),
        "a name the caller chose is the caller's to keep, registered or not"
    );
}

#[test]
fn a_submodule_does_not_resolve_to_a_project_called_modules() {
    // A submodule's `--git-common-dir` is `<super>/.git/modules/<name>`, so the
    // parent of the common dir is not a working tree at all. Naming the project
    // after it files every submodule of every repo on the machine into
    // `rememora://projects/modules/`.
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("libsub");
    let sup = tmp.path().join("super");
    for dir in [&sub, &sup] {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
        git(dir, &["config", "commit.gpgsign", "false"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }
    git(
        &sup,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "--quiet",
            "add",
            sub.to_str().unwrap(),
            "libsub",
        ],
    );

    let conn = db();
    let inside = sup.join("libsub");
    let target = project::resolve_write_target(&conn, Some("libsub"), inside.to_str().unwrap());

    assert_eq!(
        target.project.as_deref(),
        Some("libsub"),
        "a submodule keeps its own name"
    );
    assert_ne!(target.project.as_deref(), Some("modules"));
}

#[test]
fn git_facts_survive_a_cwd_with_no_working_tree() {
    // `--show-toplevel` exits 128 inside a `.git` directory. Batching it with
    // `--git-common-dir` in one rev-parse discarded the common-dir line that had
    // succeeded, so resolution stopped working from those directories.
    let repo = repo_with_worktree("myapp", "wt-1");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    let git_dir = repo.main.join(".git");

    let target = project::resolve_write_target(&conn, Some("myapp"), git_dir.to_str().unwrap());

    assert_eq!(target.project.as_deref(), Some("myapp"));
    assert_eq!(
        target.worktree, None,
        "a .git directory is not a linked worktree"
    );
}

// ---------------------------------------------------------------------------
// Reconciling namespaces that are already stranded
// ---------------------------------------------------------------------------

#[test]
fn a_stranded_worktree_namespace_is_found_and_traced_through_its_session() {
    // The worktree directory is usually long gone by the time anyone notices,
    // so the name alone cannot be resolved. `sessions.cwd` is the trail back.
    let repo = repo_with_worktree("myapp", "wt-1");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    session("stranded work")
        .project("wt-1")
        .cwd(repo.worktree_str())
        .insert(&conn);
    memory("A stranded decision")
        .project("wt-1")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();

    assert_eq!(plan.len(), 1, "exactly one namespace is unclaimed");
    assert_eq!(plan[0].name, "wt-1");
    assert_eq!(plan[0].memories, 1);
    assert_eq!(plan[0].target.as_deref(), Some("myapp"));
}

#[test]
fn a_registered_project_is_never_reported_as_stranded() {
    let conn = db();
    register(&conn, "myapp", "/nonexistent/myapp");
    memory("A normal decision")
        .project("myapp")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();

    assert!(plan.is_empty(), "got {plan:?}");
}

#[test]
fn reconcile_rewrites_the_uri_the_parent_uri_and_the_provenance() {
    let repo = repo_with_worktree("myapp", "wt-1");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    session("stranded work")
        .project("wt-1")
        .cwd(repo.worktree_str())
        .insert(&conn);
    memory("A stranded decision")
        .project("wt-1")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();
    let outcome = project::reconcile(&conn, &plan).unwrap();

    assert_eq!(outcome.memories_moved, 1);
    assert_eq!(outcome.sessions_moved, 1);
    assert_eq!(outcome.conflicts, 0);

    let moved = context::get_by_uri(
        &conn,
        "rememora://projects/myapp/memories/decision/a-stranded-decision",
    )
    .unwrap()
    .expect("the memory must now live under the real project");

    assert_eq!(
        moved.parent_uri.as_deref(),
        Some("rememora://projects/myapp/memories/decision"),
        "parent_uri is part of the hierarchy and has to move with the row"
    );
    assert_eq!(
        moved.worktree.as_deref(),
        Some("wt-1"),
        "the namespace being retired is exactly the worktree it came from"
    );
}

#[test]
fn reconcile_makes_a_stranded_memory_reachable_by_project_search() {
    // The whole point: before the rewrite the project filter cannot see it.
    let repo = repo_with_worktree("myapp", "wt-1");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    session("stranded work")
        .project("wt-1")
        .cwd(repo.worktree_str())
        .insert(&conn);
    memory("Zustand beats Redux here")
        .project("wt-1")
        .category("decision")
        .insert(&conn);

    let before = rememora::search::search(&conn, "Zustand", Some("myapp"), None, 10).unwrap();
    assert!(
        before.is_empty(),
        "precondition: a stranded memory is invisible to its own project"
    );

    let plan = project::find_stranded(&conn).unwrap();
    project::reconcile(&conn, &plan).unwrap();

    let after = rememora::search::search(&conn, "Zustand", Some("myapp"), None, 10).unwrap();
    assert_eq!(after.len(), 1, "after reconcile the project can see it");
}

#[test]
fn reconcile_leaves_a_colliding_row_in_place_rather_than_losing_it() {
    // The same memory saved twice, once under each name. `contexts.uri` is
    // UNIQUE, so one of them cannot move — dropping it silently is not this
    // command's call to make.
    let repo = repo_with_worktree("myapp", "wt-1");
    let conn = db();
    register(&conn, "myapp", repo.main_str());
    session("stranded work")
        .project("wt-1")
        .cwd(repo.worktree_str())
        .insert(&conn);
    memory("Duplicated decision")
        .project("wt-1")
        .category("decision")
        .insert(&conn);
    memory("Duplicated decision")
        .project("myapp")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();
    let outcome = project::reconcile(&conn, &plan).unwrap();

    assert_eq!(outcome.memories_moved, 0);
    assert_eq!(outcome.conflicts, 1);
    assert!(
        context::get_by_uri(
            &conn,
            "rememora://projects/wt-1/memories/decision/duplicated-decision"
        )
        .unwrap()
        .is_some(),
        "the colliding row must still exist, not be dropped"
    );
}

#[test]
fn repairing_case_drift_does_not_disturb_the_correctly_named_rows() {
    // SQLite's LIKE is ASCII-case-insensitive, so a naive
    // `uri LIKE 'rememora://projects/Ana/%'` also selects every
    // `rememora://projects/ana/...` row. Case drift is one of the exact shapes
    // this command repairs, so the prefix match has to be case-sensitive.
    let conn = db();
    register(&conn, "ana", "/nonexistent/ana");
    memory("Correctly filed decision")
        .project("ana")
        .category("decision")
        .insert(&conn);
    memory("Drifted decision")
        .project("Ana")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();
    let outcome = project::reconcile(&conn, &plan).unwrap();

    assert_eq!(outcome.memories_moved, 1, "only the drifted row moves");
    assert_eq!(
        outcome.conflicts, 0,
        "the correctly-named rows must not be swept in and counted against themselves"
    );
    assert!(
        context::get_by_uri(
            &conn,
            "rememora://projects/ana/memories/decision/correctly-filed-decision"
        )
        .unwrap()
        .is_some(),
        "the already-correct row is untouched"
    );
    assert!(
        context::get_by_uri(
            &conn,
            "rememora://projects/ana/memories/decision/drifted-decision"
        )
        .unwrap()
        .is_some(),
        "the drifted row landed in the canonical namespace"
    );
}

#[test]
fn an_unregistered_but_correctly_named_namespace_is_left_alone() {
    // A project nobody ever ran `project add` for. Its memories are filed
    // coherently; the fix is registration, not a rewrite.
    let repo = repo_with_worktree("renotes", "wt-1");
    let conn = db();
    session("work")
        .project("renotes")
        .cwd(repo.main_str())
        .insert(&conn);
    memory("A coherent decision")
        .project("renotes")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();
    assert_eq!(plan[0].target.as_deref(), Some("renotes"));

    let outcome = project::reconcile(&conn, &plan).unwrap();

    assert_eq!(outcome.memories_moved, 0);
    assert_eq!(outcome.conflicts, 0, "a no-op must not read as a conflict");
    assert_eq!(outcome.needs_registration, vec!["renotes".to_string()]);
}

#[test]
fn an_unresolvable_namespace_is_reported_rather_than_guessed() {
    let conn = db();
    memory("Orphaned decision")
        .project("ghost-project")
        .category("decision")
        .insert(&conn);

    let plan = project::find_stranded(&conn).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].target, None);

    let outcome = project::reconcile(&conn, &plan).unwrap();
    assert_eq!(outcome.memories_moved, 0);
    assert_eq!(outcome.unresolved, vec!["ghost-project".to_string()]);
    assert!(
        context::get_by_uri(
            &conn,
            "rememora://projects/ghost-project/memories/decision/orphaned-decision"
        )
        .unwrap()
        .is_some(),
        "an unresolvable memory stays exactly where it is"
    );
}
