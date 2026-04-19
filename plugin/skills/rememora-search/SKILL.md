---
name: rememora-search
description: >
  ALWAYS use this skill BEFORE any of these actions:
  (1) Implementing something non-trivial — search for prior decisions and patterns in this area before writing code.
  (2) Encountering unfamiliar code or architecture — search for entity knowledge before exploring from scratch.
  (3) The user references past work, past decisions, or "what we decided" — search for that context.
  (4) You are stuck or blocked on a problem — search for related cases and solutions.
  (5) Making a decision that could conflict with a prior one — search to check for consistency.
  This skill fires BEFORE you act — search first, then implement.
allowed-tools: Bash
---

# Search Rememora for Prior Knowledge

Before starting non-trivial work, check if there is relevant knowledge from
prior sessions. Rememora stores memories in three tiers (L0 abstract, L1
overview, L2 content), so you can filter cheaply before paying the full
token cost of a memory.

## Progressive-disclosure workflow (search → timeline → get)

Do not pull full memory content up front. Use the three verbs in order.

### 1. `rememora search --format compact` — filter

Scan for relevant memories by topic. Compact output is one line per hit
(~75 tokens/hit), enough to decide whether to drill in.

```bash
rememora search "<what you're looking for>" --project <project-name> --format compact
```

Each line looks like:

```
[decision] Picked Redis over Memcached — rememora://projects/foo/memories/decision/redis-caching (rank=-1.23)
```

Query tips:
- Use the topic or domain: `"database choice"`, `"auth middleware"`
- Use the problem area: `"JWT token expiration"`, `"rate limiting"`
- Use the entity name: `"Stripe integration"`, `"prisma schema"`

### 2. `rememora timeline --anchor <uri>` — context around a hit

Once a hit looks promising, pull the neighbours around it to understand
what else was happening when that decision was made. Sorted by creation
time by default (`--by ts`); use `--by hotness` to rank peers by
importance + hotness instead.

```bash
rememora timeline \
  --anchor rememora://projects/foo/memories/decision/redis-caching \
  --before 3 --after 3
```

You get the three closest older memories, the anchor, and the three
closest newer memories — each rendered in the same compact shape.

### 3. `rememora get <uri>` — full L2 content

Only when you know you want everything: pull the full abstract + overview +
content for a specific URI.

```bash
rememora get rememora://projects/foo/memories/decision/redis-caching
```

## Example flow

```bash
# Step 1: filter
rememora search "caching strategy" --project acme --format compact
# → hit: rememora://projects/acme/memories/decision/redis-caching (rank=-2.1)

# Step 2: context
rememora timeline --anchor rememora://projects/acme/memories/decision/redis-caching --before 3 --after 3
# → see what else was decided that week

# Step 3: full content (only for the most relevant hit)
rememora get rememora://projects/acme/memories/decision/redis-caching
```

## When to search

| Situation | Search for |
|---|---|
| About to implement a feature | Prior decisions about architecture, framework choices, patterns |
| Debugging a problem | Prior cases with similar symptoms or in the same area |
| User says "we decided..." or "remember when..." | The specific decision or event |
| Working with unfamiliar code | Entity knowledge about that module/service |
| Making a trade-off choice | Prior decisions to maintain consistency |

## After searching

- If results are relevant: incorporate them into your approach. Mention what you found.
- If no results: proceed normally. Not everything has prior context.
- **Do not fabricate memories** — only use what rememora returns.

## Rules

1. **Search BEFORE acting** — don't implement first and search after.
2. **Filter cheaply first** — `--format compact` for search, then `timeline`, then `get`. Do not default to the full `search` output — it burns tokens.
3. **Be brief** — one search, check results, move on.
4. **Don't search for things you can just read** — use Read/Grep for code, rememora for decisions and context.
