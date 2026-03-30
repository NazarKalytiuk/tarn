#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP_DIR="$(mktemp -d)"
SERVER_LOG="${TMP_DIR}/demo-server.log"
SERVER_PORT=38080

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]]; then
    kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
  fi
  rm -rf "${TMP_DIR}"
}

trap cleanup EXIT

cd "${ROOT_DIR}"
cargo build --release -p tarn -p demo-server

PORT="${SERVER_PORT}" cargo run --quiet --release -p demo-server >"${SERVER_LOG}" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:${SERVER_PORT}/health" >/dev/null; then
    break
  fi
  sleep 0.2
done

if ! curl -fsS "http://127.0.0.1:${SERVER_PORT}/health" >/dev/null; then
  echo "demo-server did not start"
  cat "${SERVER_LOG}"
  exit 1
fi

PROJECT_DIR="${TMP_DIR}/project"
mkdir -p "${PROJECT_DIR}"
(
  cd "${PROJECT_DIR}"
  "${ROOT_DIR}/target/release/tarn" init
  cat <<EOF > "${PROJECT_DIR}/tarn.env.yaml"
base_url: "http://127.0.0.1:${SERVER_PORT}"
EOF
  "${ROOT_DIR}/target/release/tarn" run >/dev/null
)

FAIL_DIR="${TMP_DIR}/assertion-failure"
mkdir -p "${FAIL_DIR}/tests"
cat <<EOF > "${FAIL_DIR}/tarn.env.yaml"
base_url: "http://127.0.0.1:${SERVER_PORT}"
EOF
cat <<'EOF' > "${FAIL_DIR}/tests/failing.tarn.yaml"
name: Assertion failure
steps:
  - name: Wrong status
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 201
EOF

set +e
ASSERTION_OUTPUT="$("${ROOT_DIR}/target/release/tarn" run --format json "${FAIL_DIR}/tests/failing.tarn.yaml" 2>&1)"
ASSERTION_EXIT=$?
set -e

if [[ "${ASSERTION_EXIT}" -ne 1 ]]; then
  echo "Expected assertion failure exit code 1, got ${ASSERTION_EXIT}"
  echo "${ASSERTION_OUTPUT}"
  exit 1
fi

echo "${ASSERTION_OUTPUT}" | grep -q '"failure_category": "assertion_failed"'

set +e
RUNTIME_OUTPUT="$("${ROOT_DIR}/target/release/tarn" run --format json --var base_url=http://127.0.0.1:1 "${PROJECT_DIR}/tests/health.tarn.yaml" 2>&1)"
RUNTIME_EXIT=$?
set -e

if [[ "${RUNTIME_EXIT}" -ne 3 ]]; then
  echo "Expected runtime failure exit code 3, got ${RUNTIME_EXIT}"
  echo "${RUNTIME_OUTPUT}"
  exit 1
fi

echo "${RUNTIME_OUTPUT}" | grep -q '"failure_category": "connection_error"'
