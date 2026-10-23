#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep calibrated smoothness measurements alone on the runner. The other
# fixtures run first with two workers, under their existing assertions.
set -u
cd "$(dirname "$0")/.." || exit 1

if [ "$#" -ne 0 ]; then
  echo "Usage: sh scripts/test-browser-fixtures.sh" >&2
  exit 2
fi

functional=0
echo "Functional fixtures (2 workers)"
PLAYWRIGHT_HTML_OUTPUT_DIR=playwright-report/functional \
  npx playwright test '^(?!.*[/\\]smoothness\.fixture\.spec\.ts$).*\.fixture\.spec\.ts$' \
  --workers=2 --output=test-results/functional || functional=$?

smoothness=0
echo "Smoothness fixtures (1 worker, after functional fixtures finish)"
PLAYWRIGHT_HTML_OUTPUT_DIR=playwright-report/smoothness \
  npx playwright test '[/\\]smoothness\.fixture\.spec\.ts$' \
  --workers=1 --output=test-results/smoothness || smoothness=$?

# Run both phases, including after an assertion failure or runner error.
if [ "$functional" -ne 0 ] || [ "$smoothness" -ne 0 ]; then
  exit 1
fi
