---
name: rememora-save
description: >
  ALWAYS use this skill after ANY of these events during your work:
  (1) You or the user made an architectural or design decision — choosing a database, framework, algorithm, API design, or trade-off.
  (2) You fixed a non-trivial bug — especially silent failures, race conditions, configuration gotchas, or environment-specific issues.
  (3) You discovered a codebase pattern — a naming convention, error handling approach, testing strategy, or architectural idiom.
  (4) You discovered an important entity — a service, API endpoint, config file, environment variable, or integration point.
  (5) The user corrected your approach or stated a preference.
  This skill fires AUTOMATICALLY — do not wait for the user to ask. Save knowledge AS you work.
allowed-tools: Bash
---

# Save Knowledge to Rememora

You just completed work that produced knowledge worth preserving. Save it NOW.

## What to save

Identify what happened and pick the right category:

| What happened | Category | Example |
|---|---|---|
| Design/architecture choice | `decision` | "Chose PostgreSQL over MongoDB for ACID transactions in payments" |
| Bug fix or workaround | `case` | "JWT middleware silently swallowed TokenExpiredError — added explicit 401 response" |
| Codebase pattern discovered | `pattern` | "All route handlers use asyncHandler wrapper for error propagation" |
| Important entity found | `entity` | "Payment service at src/services/payment.ts wraps Stripe API with retry logic" |
| User preference or correction | `preference` | "User prefers 2-space indent and explicit return types" |

## How to save

Run this command via Bash:

```bash
rememora save "<clear, specific description of what was learned>" \
  --category <decision|case|pattern|entity|preference> \
  --project <project-name> \
  --importance <0.5-1.0>
```

### Importance guide
- **0.9-1.0**: Architectural decisions, security-critical fixes, user-stated preferences
- **0.7-0.8**: Design choices, non-trivial bugs, key integration patterns  
- **0.5-0.6**: Minor patterns, entity discoveries, small conventions

## Rules

1. **Be specific**: Include file paths, function names, error messages, concrete choices
2. **Include the WHY**: "Chose X because Y" not just "Chose X"
3. **One save per distinct piece of knowledge** — don't bundle unrelated things
4. **Do NOT save**: Code that can be read from files, git history, anything in README/docs, temporary debugging state
5. **Save immediately** — don't batch or defer

## After saving

Say what you saved in one line, then continue with your work. Do not interrupt flow.
