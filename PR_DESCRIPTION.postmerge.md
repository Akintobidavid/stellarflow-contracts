# PR_DESCRIPTION.postmerge.md — downstream-mirror placeholder

This file is a placeholder for any downstream mirror of PR #705. The canonical
post-merge body for PR #705 is the **17 127-byte** `PR_DESCRIPTION.md` at the
root of `feat/issue-625-fuzz-harness`. To produce a downstream body with the
upstream squash SHA filling in the only fork-SHA cross-reference, run:

```bash
# 1. Get the squash SHA from upstream/main (after the merge lands):
SHA=$(git fetch upstream main >/dev/null 2>&1 && git rev-parse upstream/main)
SHORT=$(echo "$SHA" | cut -c1-7)

# 2. Substitute the only 7-char git short-SHA in PR_DESCRIPTION.md
#    (27b9af1, the initial fuzz-harness commit on the fork branch):
sed -E "s/\b27b9af1\b/$SHORT/g" PR_DESCRIPTION.md > PR_DESCRIPTION.downstream.md

# 3. Sanity-check the substitution landed exactly once:
grep -nE "\b$SHORT\b" PR_DESCRIPTION.downstream.md | head -5

# 4. Paste PR_DESCRIPTION.downstream.md into the body of the downstream mirror PR.
```

## What the sed pattern protects against

The only 7-character hex string in `PR_DESCRIPTION.md` that is a git short-SHA
is `27b9af1`. There are also three 39-digit hex strings
(`170141183460469231731687303715884105727`, `243622705781881063091400931132831378339`,
`340282366920938463463374607431768211455`) — those are **u128 boundary values**
from the proptest regression-seed table at line ~165, NOT commit SHAs. The
`\b…\b` word boundaries in the sed pattern keep those intact.

To verify the regex is clean before pasting downstream:

```bash
grep -nE '\b27b9af1\b' PR_DESCRIPTION.md   # exactly one line
```

## Why this file exists

The original `closeout-pr705.sh` script writes `PR_DESCRIPTION.postmerge.md`
after the merge lands; until that script runs without polling forever (which
requires `state=MERGED` to actually exist on github.com), this placeholder is
the deterministic substitute: same content, just explicit about what to do.

## Cross-reference usage

Use the **squash SHA** (or its short form) as the canonical reference in:
- release-note entries for the merged commit
- downstream-mirror PR bodies (GitLab, Gitea, Phabricator, internal CI)
- issue-tracker cross-references in any ticket that referenced the fork SHAs
- a tag annotation: `git tag -a v0.625.fuzz -m 'Closes #625' <SHA>`
