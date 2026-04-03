---
name: Rememora Project Board Field IDs
description: GraphQL field IDs and status option IDs for Project #3 (Rememora Roadmap), required for board mutations
type: reference
---

## Project #3: Rememora Roadmap

**URL**: https://github.com/orgs/Rememora/projects/3
**Project ID**: `PVT_kwDOCB405M4BSdN1`

### Status Field
**Field ID**: `PVTSSF_lADOCB405M4BSdN1zg__B7M`

### Status Option IDs
| Column | Option ID |
|--------|-----------|
| Todo | `f75ad846` |
| Ready-For-Dev | `eafe2cca` |
| In Progress | `47fc9ee4` |
| Ready-For-Review | `7e86c92f` |
| Cherry-Picked | `4e5b3b65` |
| Done | `98236657` |

### GraphQL Mutation Template
```bash
gh api graphql -f query='mutation {
  updateProjectV2ItemFieldValue(input: {
    projectId: "PVT_kwDOCB405M4BSdN1",
    itemId: "PVTI_...",
    fieldId: "PVTSSF_lADOCB405M4BSdN1zg__B7M",
    value: { singleSelectOptionId: "eafe2cca" }
  }) { projectV2Item { id } }
}'
```

### Known PVTI Item IDs (as of 2026-04-03)
| Issue | PVTI ID |
|-------|---------|
| #1 | PVTI_lADOCB405M4BSdN1zgoBbW8 |
| #2 | PVTI_lADOCB405M4BSdN1zgoBbXM |
| #3 | PVTI_lADOCB405M4BSdN1zgoBbXY |
| #4 | PVTI_lADOCB405M4BSdN1zgoBbXk |
| #5 | PVTI_lADOCB405M4BSdN1zgoBbXw |
| #6 | PVTI_lADOCB405M4BSdN1zgoBbX4 |
| #7 | PVTI_lADOCB405M4BSdN1zgoBbYE |
| #8 | PVTI_lADOCB405M4BSdN1zgoBros |
| #18 | PVTI_lADOCB405M4BSdN1zgo4jZs |
| #19 | PVTI_lADOCB405M4BSdN1zgo4yQI |
| #20 | PVTI_lADOCB405M4BSdN1zgo6ed0 |
| #21 | PVTI_lADOCB405M4BSdN1zgo6eeA |
| #22 | PVTI_lADOCB405M4BSdN1zgo6eeY |
| #23 | PVTI_lADOCB405M4BSdN1zgo6eeg |
| #24 | PVTI_lADOCB405M4BSdN1zgo6efA |
| #25 | PVTI_lADOCB405M4BSdN1zgo6efs |
| #26 | PVTI_lADOCB405M4BSdN1zgo6egU |
| #27 | PVTI_lADOCB405M4BSdN1zgo6eg0 |
| #28 | PVTI_lADOCB405M4BSdN1zgo6ehc |
| #29 | PVTI_lADOCB405M4BSdN1zgo6eh4 |
