#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
harness_source="$repo_root/testdata/tauri-upgrade/native-schema-11/harness"
compatible_harness_source="$repo_root/testdata/tauri-upgrade/native-schema-11/compatible-harness"
source_package="$repo_root/testdata/packages/with-avatar.charx"
tag_name="native-baseline-before-tauri-2026-08-02"
tag_object="9a4a3d5ee08c3457fed9842ccf4184272805e0d0"
peeled_commit="66e398fa6256f17b04c82569e6764a9e5332265c"
source_repository="https://github.com/Dokpamo/lorepia-native-reference.git"

for command in cargo git; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command is unavailable: $command" >&2
    exit 1
  fi
done
if command -v sha256sum >/dev/null 2>&1; then
  sha256_file() {
    sha256sum "$1" | awk '{print $1}'
  }
elif command -v shasum >/dev/null 2>&1; then
  sha256_file() {
    shasum -a 256 "$1" | awk '{print $1}'
  }
else
  echo "Required SHA-256 command is unavailable (sha256sum or shasum)." >&2
  exit 1
fi

runtime_temp="$(mktemp -d "${TMPDIR:-/tmp}/lorepia-schema11-runtime.XXXXXX")"
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  rm -rf -- "$runtime_temp"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

native_checkout="${LOREPIA_NATIVE_REFERENCE_CHECKOUT:-}"
if [[ -z "$native_checkout" ]]; then
  if [[ -z "${LOREPIA_NATIVE_REFERENCE_TOKEN:-}" ]]; then
    echo \
      "LOREPIA_NATIVE_REFERENCE_TOKEN is required when no approved local checkout is supplied." \
      >&2
    exit 1
  fi
  native_checkout="$runtime_temp/native-reference"
  git init --quiet "$native_checkout"
  git -C "$native_checkout" remote add origin "$source_repository"
  GIT_TERMINAL_PROMPT=0 git \
    -c 'credential.helper=!f() { test "$1" = get && printf "%s\n" "username=x-access-token" "password=$LOREPIA_NATIVE_REFERENCE_TOKEN"; }; f' \
    -c credential.useHttpPath=true \
    -C "$native_checkout" fetch \
    --quiet \
    --depth=1 \
    origin \
    "refs/tags/$tag_name:refs/tags/$tag_name"
  git -C "$native_checkout" checkout --quiet --detach "$peeled_commit"
fi
unset LOREPIA_NATIVE_REFERENCE_TOKEN
native_checkout="$(cd "$native_checkout" && pwd)"

actual_tag_object="$(git -C "$native_checkout" rev-parse "refs/tags/$tag_name")"
actual_tag_type="$(git -C "$native_checkout" cat-file -t "$actual_tag_object")"
actual_peeled_commit="$(git -C "$native_checkout" rev-parse "refs/tags/$tag_name^{}")"
actual_head="$(git -C "$native_checkout" rev-parse HEAD)"
if [[ "$actual_tag_object" != "$tag_object" \
  || "$actual_tag_type" != "tag" \
  || "$actual_peeled_commit" != "$peeled_commit" \
  || "$actual_head" != "$peeled_commit" ]]; then
  echo "Frozen native reference identity does not match the approved annotated tag." >&2
  exit 1
fi
if [[ -n "$(git -C "$native_checkout" status --short)" ]]; then
  echo "Frozen native reference checkout must be clean." >&2
  exit 1
fi

prepare_harness() {
  local destination=$1
  local core_path=$2
  local escaped_core_path=""
  mkdir -p "$destination/src"
  cp "$harness_source/src/main.rs" "$destination/src/main.rs"
  cp "$harness_source/Cargo.lock" "$destination/Cargo.lock"
  escaped_core_path="$(printf '%s' "$core_path" | sed 's/[&|]/\\&/g')"
  sed \
    "s|@LOREPIA_CORE_PATH@|$escaped_core_path|" \
    "$harness_source/Cargo.toml.in" >"$destination/Cargo.toml"
}

prepare_compatible_harness() {
  local destination=$1
  local shell_api_path=$2
  local escaped_shell_api_path=""
  mkdir -p "$destination/src"
  cp "$compatible_harness_source/src/main.rs" "$destination/src/main.rs"
  cp "$compatible_harness_source/Cargo.lock" "$destination/Cargo.lock"
  escaped_shell_api_path="$(printf '%s' "$shell_api_path" | sed 's/[&|]/\\&/g')"
  sed \
    "s|@LOREPIA_SHELL_API_PATH@|$escaped_shell_api_path|" \
    "$compatible_harness_source/Cargo.toml.in" >"$destination/Cargo.toml"
}

frozen_harness="$runtime_temp/frozen-harness"
prepare_harness "$frozen_harness" "$native_checkout/crates/core"
compatible_harness="$runtime_temp/compatible-harness"
prepare_compatible_harness "$compatible_harness" "$repo_root/crates/shell-api"

core_root="$runtime_temp/core-root"
runtime_manifest="$runtime_temp/frozen-runtime-manifest.json"
canonical_database="$core_root/db/lorepia.sqlite3"
candidate_state="$runtime_temp/candidate-state.json"
current_reopen_test_log="$runtime_temp/current-reopen-test.log"
expected_reopen_test_output="test state::tests::tauri_app_state_shell_api_reopens_compatible_recovery_writes ... ok"

# Resolve/download the frozen adapter's locked dependency graph before the
# proof execution so the old-runtime phase itself is offline.
cargo fetch --locked --manifest-path "$frozen_harness/Cargo.toml"
cargo clippy \
  --locked \
  --offline \
  --quiet \
  --manifest-path "$frozen_harness/Cargo.toml" \
  --target-dir "$runtime_temp/target-frozen" \
  -- \
  -D warnings

# Build the source-compatible Shell API recovery client before write A. The
# later recovery phase executes this exact artifact directly and never rebuilds
# it.
cargo fetch --locked --manifest-path "$compatible_harness/Cargo.toml"
cargo clippy \
  --locked \
  --offline \
  --quiet \
  --manifest-path "$compatible_harness/Cargo.toml" \
  --target-dir "$runtime_temp/target-compatible" \
  --all-targets \
  -- \
  -D warnings
cargo build \
  --locked \
  --offline \
  --quiet \
  --manifest-path "$compatible_harness/Cargo.toml" \
  --target-dir "$runtime_temp/target-compatible"
compatible_binary="$runtime_temp/target-compatible/debug/lorepia-schema11-compatible-rollback-harness"
if [[ ! -x "$compatible_binary" && -x "$compatible_binary.exe" ]]; then
  compatible_binary="$compatible_binary.exe"
fi
if [[ ! -x "$compatible_binary" ]]; then
  echo "Prebuilt source-compatible rollback client is missing." >&2
  exit 1
fi
compatible_artifact_sha256="$(sha256_file "$compatible_binary")"

cargo run \
  --locked \
  --offline \
  --quiet \
  --manifest-path "$frozen_harness/Cargo.toml" \
  --target-dir "$runtime_temp/target-frozen" \
  -- \
  seed "$core_root" "$source_package" "$runtime_manifest"

canonical_before="$(sha256_file "$canonical_database")"

LOREPIA_SCHEMA11_RUNTIME_ROOT="$core_root" \
LOREPIA_SCHEMA11_RUNTIME_STATE="$candidate_state" \
  cargo test \
    --locked \
    -p lorepia-tauri \
    --lib \
    state::tests::tauri_app_state_shell_api_writes_active_generation_for_compatible_recovery \
    -- \
    --ignored \
    --exact \
    --nocapture

canonical_after="$(sha256_file "$canonical_database")"
if [[ "$canonical_after" != "$canonical_before" ]]; then
  echo "Current cutover changed the canonical schema-eleven database bytes." >&2
  exit 1
fi
committed_generation_count="$({
  find "$core_root/db/schema-cutover" \
    -mindepth 2 \
    -maxdepth 2 \
    -type f \
    -name generation-committed.json
} | wc -l | tr -d '[:space:]')"
if [[ "$committed_generation_count" -lt 1 ]]; then
  echo "Current cutover did not publish a committed database generation." >&2
  exit 1
fi
if [[ ! -s "$candidate_state" ]]; then
  echo "Current Core continuity test did not publish its candidate state." >&2
  exit 1
fi

"$compatible_binary" round-trip "$core_root" "$candidate_state"
if [[ "$(sha256_file "$compatible_binary")" != "$compatible_artifact_sha256" ]]; then
  echo "Source-compatible rollback client artifact changed after write A." >&2
  exit 1
fi
if ! grep -Fq \
  "\"compatible_rollback_artifact_sha256\": \"$compatible_artifact_sha256\"" \
  "$candidate_state"; then
  echo "Source-compatible rollback state did not bind the prebuilt artifact." >&2
  exit 1
fi

LOREPIA_SCHEMA11_RUNTIME_ROOT="$core_root" \
LOREPIA_SCHEMA11_RUNTIME_STATE="$candidate_state" \
  cargo test \
    --locked \
    -p lorepia-tauri \
    --lib \
    state::tests::tauri_app_state_shell_api_reopens_compatible_recovery_writes \
    -- \
    --ignored \
    --exact \
    --nocapture \
    2>&1 | tee "$current_reopen_test_log"
if ! grep -Fqx "$expected_reopen_test_output" "$current_reopen_test_log"; then
  echo "Current AppState reopen test did not report its exact success line." >&2
  exit 1
fi

cargo run \
  --locked \
  --offline \
  --quiet \
  --manifest-path "$frozen_harness/Cargo.toml" \
  --target-dir "$runtime_temp/target-frozen" \
  -- \
  inspect-legacy "$core_root" "$runtime_manifest" "$candidate_state"

canonical_final="$(sha256_file "$canonical_database")"
if [[ "$canonical_final" != "$canonical_before" ]]; then
  echo "Frozen Core inspection changed the canonical schema-eleven database bytes." >&2
  exit 1
fi

echo "exact frozen runtime preserved-canonical schema11 readback: PASS"
echo "Tauri AppState shell-api active-generation write A: PASS"
echo "source-compatible shell-api recovery client active-generation A/B round trip: PASS"
echo "active-generation A/B visible to exact frozen runtime: EXPECTED_FALSE"
echo "signed/platform rollback-client drill: NOT_RUN"
