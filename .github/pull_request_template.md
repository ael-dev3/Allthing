<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Title: a Conventional Commit, such as "fix(video): …" or "feat(profile): …". One focused change per pull request. -->

## Summary

<!-- What changes for readers or operators? -->

## Why

<!-- The problem this solves. Link the issue, if there is one ("Fixes #123"). -->

## How it was tested

- [ ] `bash scripts/check.sh`
- [ ] End to end at desktop width (1280×900): `npx playwright test` or the relevant specs
- [ ] End to end at phone width (390×844)
- [ ] Screenshots for UI changes, before and after, at both widths (below)

<!-- Anything else you checked: production (E2E_BASE_URL=https://allthing.social), the static copy (E2E_PAGES=1), the performance budget (e2e/perf.spec.ts). -->

## Checklist

- [ ] The title is a [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/)
- [ ] A line in `CHANGELOG.md` under `[Unreleased]`, if readers or operators would notice the change
- [ ] `docs/BUDGET.md` updated, if request, subrequest or CPU costs changed
- [ ] No secrets, tokens, `.env` or `.dev.vars` files, session URLs or personal data
- [ ] Still read-only, and no content hidden because of a label
