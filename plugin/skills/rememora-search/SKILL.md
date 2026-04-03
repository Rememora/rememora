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

Before starting this work, check if there's relevant knowledge from prior sessions.

## How to search

Run this command via Bash:

```bash
rememora search "<what you're looking for>" --project <project-name>
```

### Search query tips
- Use the topic or domain: `"database choice"`, `"auth middleware"`, `"payment processing"`
- Use the problem area: `"JWT token expiration"`, `"rate limiting"`, `"test flakiness"`
- Use the entity name: `"Stripe integration"`, `"user service"`, `"prisma schema"`

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

1. **Search BEFORE acting** — don't implement first and search after
2. **Be brief** — one search, check results, move on
3. **Don't search for things you can just read** — use Read/Grep for code, rememora for decisions and context
