# Engineering Insights

Lessons learned while building rememora. Each insight captures a non-obvious gotcha, design decision, or pattern discovered during development.

---

## Git Worktrees Don't Inherit Local Config

**Context:** `rememora agent-run` creates git worktrees for isolated agent work.

Git worktrees inherit global config (`~/.gitconfig`) but **not** the parent repo's local config (`.git/config`). This means `user.name`, `user.email`, and `commit.gpgsign` set locally on the parent repo are silently lost in the worktree. Commits from the worktree may go unsigned or use the wrong identity.

**Fix:** After creating a worktree, read signing config from the parent repo via `git config --get` (which resolves the full system -> global -> local cascade) and propagate each key into the worktree with `git config --local`.

**Also propagate `gpg.format`** — some setups use SSH signing (`gpg.format = ssh`). Without it, the worktree tries GPG signing even if the parent repo uses SSH keys.

---

## sqlite-vec: CTE Pattern for KNN + Filters

**Context:** Vector search with sqlite-vec and post-query filters.

sqlite-vec's `vec0` virtual table query planner only handles `MATCH` and `k =` constraints. Adding `WHERE` conditions on joined tables (e.g., `c.superseded_by IS NULL`, `c.category = ?`) in the same query level as `MATCH` causes `"unable to use function MATCH in the requested context"`.

**Fix:** Use a CTE to isolate the KNN query:

```sql
WITH knn AS (
    SELECT context_id, distance
    FROM vec_contexts
    WHERE embedding MATCH ?1 AND k = ?2
)
SELECT c.* FROM knn
JOIN contexts c ON c.id = knn.context_id
WHERE c.superseded_by IS NULL AND c.category = ?3
```

**Over-fetch to compensate:** Since `k` is set before filtering, some of the `k` rows will be filtered out. Use `k = limit * 5` and `LIMIT` in the outer query to ensure enough results survive post-filtering.

Reference: [sqlite-vec issue #116](https://github.com/asg017/sqlite-vec/issues/116)

---

## sqlite-vec: Default Distance Metric is L2, Not Cosine

**Context:** `all-MiniLM-L6-v2` produces L2-normalized embeddings designed for cosine similarity.

The `vec0` virtual table defaults to **L2 (Euclidean)** distance. If you use `1.0 - distance` to convert to a similarity score, this only makes sense for cosine distance (range 0-2). With L2 distance, the formula produces nonsensical scores that can go negative.

**Fix:** Explicitly set `distance_metric=cosine` in the DDL:

```sql
CREATE VIRTUAL TABLE vec_contexts USING vec0(
    context_id TEXT PRIMARY KEY,
    embedding float[384] distance_metric=cosine
);
```

---

## Eval JSONL: Braintrust-Aligned Universal Interchange

**Context:** Making eval benchmark output importable into multiple platforms.

Every major eval platform (AI Foundry, Langfuse, LangSmith, Braintrust, OpenAI Evals) converges on the same shape: input, output, expected, scores, metadata. Braintrust's format is the closest to a universal interchange — their top-level fields map directly, while other platforms need only thin renames:

| Platform | Adapter |
|----------|---------|
| AI Foundry | `input.query` -> `query`, `output` -> `response`, `expected` -> `ground_truth` |
| Langfuse | `expected` -> `expectedOutput`, `scores` -> separate `{name, value}` objects |
| LangSmith | `input` -> `inputs`, `scores` -> `{key, score}` feedback objects |
| OpenAI Evals | `input.query` -> `[{role:"user", content}]`, `expected` -> `ideal` |
| Braintrust | Direct import, no transformation needed |

**Key design choice:** `scores` is a `Record<string, number>` dict (not a single number) because every platform models scores as named key-value pairs.

---

## Test File Convention: `*.eval.ts` vs `*.test.ts`

**Context:** Separating eval format validation from unit tests.

`*.test.ts` files are unit tests (testing internal logic). `*.eval.ts` files are evaluation tests (testing output contracts against external platform specs). Different concerns, different file extension, independently runnable via vitest config:

```ts
// vitest.config.ts
export default defineConfig({
  test: { include: ["src/**/*.eval.ts"] },
});
```

---

## Eval Scenarios: Directive vs Conversational Prompts

**Context:** Claude Code eval scenarios for testing rememora CLI usage.

When testing whether an agent follows tool-use instructions, **conversational phrasing fails non-deterministically**. "Can you look that up in memory?" reads as a casual question. "Search our persistent memory for that decision." is an imperative that reliably triggers CLI tool use.

Passing scenarios use **actionable specifics**: session IDs, "initialize your memory system", explicit verbs like "search", "save". Failing scenarios used **vague language**: "remember this", "look that up", "save to memory so we remember it."

**Fix:** Use directive language with domain-specific terms ("persistent memory", "as a case", "search") rather than conversational phrasing. Also provide `--append-system-prompt` with rememora CLI instructions to make evals self-contained.
