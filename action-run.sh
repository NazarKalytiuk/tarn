#!/usr/bin/env bash
set -euo pipefail

CMD=(tarn run)

# Add path
if [ -n "${TARN_PATH:-}" ]; then
  CMD+=("${TARN_PATH}")
fi

# Add environment
if [ -n "${TARN_ENV:-}" ]; then
  CMD+=(--env "${TARN_ENV}")
fi

# Add output format
if [ -n "${TARN_FORMAT:-}" ]; then
  CMD+=(--format "${TARN_FORMAT}")
fi

# Add tag filter
if [ -n "${TARN_TAG:-}" ]; then
  CMD+=(--tag "${TARN_TAG}")
fi

# Parse newline-separated vars into --var flags
if [ -n "${TARN_VARS:-}" ]; then
  while IFS= read -r line; do
    # Skip empty lines
    if [ -n "${line}" ]; then
      CMD+=(--var "${line}")
    fi
  done <<< "${TARN_VARS}"
fi

echo "Running: ${CMD[*]}"
"${CMD[@]}"
exit $?
