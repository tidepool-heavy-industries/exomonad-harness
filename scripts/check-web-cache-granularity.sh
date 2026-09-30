#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BUCK2="${BUCK2:-buck2}"
SOURCE="web/src/ws-client.ts"
LOCK="web/package-lock.json"

if ! command -v "$BUCK2" >/dev/null 2>&1; then
  echo "buck2 was not found; enter the pinned harness build environment first" >&2
  exit 2
fi

if [ -n "$(git status --porcelain=v1 --untracked-files=all -- web)" ]; then
  echo "refusing to edit web inputs while the web tree has existing changes" >&2
  git status --short -- web >&2
  exit 2
fi

EVIDENCE="$(mktemp -d "${TMPDIR:-/tmp}/harness-web-cache-granularity.XXXXXX")"
cp -p "$SOURCE" "$EVIDENCE/ws-client.ts.baseline"
cp -p "$LOCK" "$EVIDENCE/package-lock.json.baseline"

hash_file() {
  sha256sum "$1" | cut -d ' ' -f1
}

BASE_SOURCE_HASH="$(hash_file "$SOURCE")"
BASE_LOCK_HASH="$(hash_file "$LOCK")"
SOURCE_MUTATED_HASH=""
LOCK_MUTATED_HASH=""

restore_if_owned() {
  local path="$1" backup="$2" base_hash="$3" mutated_hash="$4"
  local current_hash

  current_hash="$(hash_file "$path")"
  if [ "$current_hash" = "$base_hash" ]; then
    return 0
  fi
  if [ -z "$mutated_hash" ] || [ "$current_hash" != "$mutated_hash" ]; then
    echo "refusing to overwrite concurrent change to $path" >&2
    echo "baseline backup retained at $backup" >&2
    return 1
  fi

  cat "$backup" > "$path"
  current_hash="$(hash_file "$path")"
  if [ "$current_hash" != "$base_hash" ]; then
    echo "restoration hash mismatch for $path" >&2
    echo "baseline backup retained at $backup" >&2
    return 1
  fi
}

cleanup() {
  local status=$?
  set +e
  restore_if_owned "$SOURCE" "$EVIDENCE/ws-client.ts.baseline" \
    "$BASE_SOURCE_HASH" "$SOURCE_MUTATED_HASH" || status=1
  restore_if_owned "$LOCK" "$EVIDENCE/package-lock.json.baseline" \
    "$BASE_LOCK_HASH" "$LOCK_MUTATED_HASH" || status=1
  if [ "$(hash_file "$SOURCE")" = "$BASE_SOURCE_HASH" ] && \
     [ "$(hash_file "$LOCK")" = "$BASE_LOCK_HASH" ]; then
    echo "input restoration verified" | tee "$EVIDENCE/restoration.txt"
  else
    echo "input restoration needs attention; backups and logs are retained in $EVIDENCE" >&2
    status=1
  fi
  echo "evidence: $EVIDENCE"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cat > "$EVIDENCE/metadata.txt" <<EOF
head=$(git rev-parse HEAD)
branch=$(git branch --show-current)
buck2=$($BUCK2 --version)
source=$SOURCE
source_baseline_sha256=$BASE_SOURCE_HASH
lock=$LOCK
lock_baseline_sha256=$BASE_LOCK_HASH
EOF

python3 - "$ROOT/web/package.json" "$ROOT/$LOCK" <<'PY' | tee "$EVIDENCE/npm-identity.txt"
import json
import sys
from pathlib import Path

package = json.loads(Path(sys.argv[1]).read_text())
lock = json.loads(Path(sys.argv[2]).read_text())
root = lock["packages"][""]
identity = (package["name"], package["version"])
lock_identity = (lock["name"], lock["version"])
package_root_identity = (root["name"], root["version"])
if identity != lock_identity or identity != package_root_identity:
    raise SystemExit(
        f"package/lock identity mismatch: {identity!r}, {lock_identity!r}, "
        f"{package_root_identity!r}"
    )
print(f"npm identity: {identity[0]}@{identity[1]} (lockfileVersion {lock['lockfileVersion']})")
PY

run_build() {
  local phase="$1"
  shift
  local event_log="$EVIDENCE/$phase.json-lines"
  local build_id_file="$EVIDENCE/$phase.build-id"

  echo "== $phase: buck2 build $* ==" | tee "$EVIDENCE/$phase.heading.txt"
  "$BUCK2" build --local-only -c remote.enabled=false \
    --event-log "$event_log" --write-build-id "$build_id_file" \
    --show-full-simple-output -v 1 "$@" 2>&1 | tee "$EVIDENCE/$phase.build.log"
  printf '%s\n' "$event_log" > "$EVIDENCE/$phase.event-log-path.txt"
  "$BUCK2" log what-ran --no-remote --format readable "$event_log" \
    > "$EVIDENCE/$phase.actions.txt"
  "$BUCK2" log what-ran --no-remote --format json "$event_log" \
    > "$EVIDENCE/$phase.actions.jsonl"
  "$BUCK2" log what-ran --no-remote --emit-cache-queries --format json "$event_log" \
    > "$EVIDENCE/$phase.cache-events.jsonl"
  "$BUCK2" log summary --no-remote "$event_log" \
    > "$EVIDENCE/$phase.summary.txt"
}

assert_action_records() {
  local phase="$1" expected="$2"
  python3 - "$EVIDENCE/$phase.actions.jsonl" "$expected" \
    "$EVIDENCE/$phase.build-id" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
expected = sys.argv[2]
build_id = Path(sys.argv[3]).read_text().strip()
records = []
for line in path.read_text().splitlines():
    line = line.strip()
    if line.startswith("{"):
        records.append(json.loads(line))

identities = {}
for record in records:
    identity = record.get("identity", "").split(" (", 1)[0]
    if identity.startswith("root//web:"):
        identities[identity.rsplit(":", 1)[1]] = record

if expected == "source-only":
    wanted = {"check", "test", "dist"}
    if not wanted.issubset(identities):
        raise SystemExit(f"expected executed web actions {sorted(wanted)}, got {sorted(identities)}")
    if "npm_dependencies" in identities:
        raise SystemExit("source-only edit executed //web:npm_dependencies")
    for target in wanted:
        record = identities[target]
        if record.get("reason") != "build" or record.get("reproducer", {}).get("executor") != "Local":
            raise SystemExit(f"//web:{target} was not recorded as a local build action")
        details = record.get("reproducer", {}).get("details", {})
        if details.get("env", {}).get("BUCK_BUILD_ID") != build_id:
            raise SystemExit(f"//web:{target} does not belong to build {build_id}")
        if "src/ws-client.ts" not in details.get("env", {}).get("SRCS", ""):
            raise SystemExit(f"//web:{target} does not declare the probe source as an input")
elif expected == "lockfile-only":
    if "npm_dependencies" not in identities:
        raise SystemExit("lockfile input change did not execute //web:npm_dependencies")
    record = identities["npm_dependencies"]
    details = record.get("reproducer", {}).get("details", {})
    if record.get("reason") != "build" or record.get("reproducer", {}).get("executor") != "Local":
        raise SystemExit("//web:npm_dependencies was not recorded as a local build action")
    sources = details.get("env", {}).get("SRCS", "")
    if "package.json" not in sources or "package-lock.json" not in sources:
        raise SystemExit(f"npm dependency action has unexpected SRCS: {sources!r}")
    if details.get("env", {}).get("BUCK_BUILD_ID") != build_id:
        raise SystemExit(f"//web:npm_dependencies does not belong to build {build_id}")
else:
    raise SystemExit(f"unknown action assertion: {expected}")

print(f"{expected} action records verified in {path}")
PY
}

run_build warm-baseline //web:check //web:test //web:dist

SOURCE_MUTATED_HASH="$( { cat "$SOURCE"; printf '\n// temporary source-only cache granularity probe\n'; } | sha256sum | cut -d ' ' -f1 )"
printf '\n// temporary source-only cache granularity probe\n' >> "$SOURCE"
echo "source_probe_sha256=$SOURCE_MUTATED_HASH" >> "$EVIDENCE/metadata.txt"
run_build source-only //web:check //web:test //web:dist
assert_action_records source-only source-only

restore_if_owned "$SOURCE" "$EVIDENCE/ws-client.ts.baseline" \
  "$BASE_SOURCE_HASH" "$SOURCE_MUTATED_HASH"
SOURCE_MUTATED_HASH=""

LOCK_MUTATED_HASH="$(python3 - "$LOCK" <<'PY'
import hashlib
import sys
from pathlib import Path

before = Path(sys.argv[1]).read_bytes()
if not before.endswith(b"\n"):
    raise SystemExit("expected package-lock.json to end with a newline")
print(hashlib.sha256(before + b"\n").hexdigest())
PY
)"
python3 - "$LOCK" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
before = path.read_bytes()
if not before.endswith(b"\n"):
    raise SystemExit("expected package-lock.json to end with a newline")
after = before + b"\n"
if json.loads(before) != json.loads(after):
    raise SystemExit("whitespace probe changed package-lock.json semantics")
path.write_bytes(after)
PY
echo "lock_whitespace_probe_sha256=$LOCK_MUTATED_HASH" >> "$EVIDENCE/metadata.txt"
run_build lockfile-only //web:npm_dependencies
assert_action_records lockfile-only lockfile-only

restore_if_owned "$LOCK" "$EVIDENCE/package-lock.json.baseline" \
  "$BASE_LOCK_HASH" "$LOCK_MUTATED_HASH"
LOCK_MUTATED_HASH=""

run_build restored-baseline //web:check //web:test //web:dist //web:npm_dependencies
if [ "$(hash_file "$SOURCE")" != "$BASE_SOURCE_HASH" ] || \
   [ "$(hash_file "$LOCK")" != "$BASE_LOCK_HASH" ]; then
  echo "baseline files differ after restored-baseline build" >&2
  exit 1
fi
echo "restored_source_sha256=$BASE_SOURCE_HASH" >> "$EVIDENCE/metadata.txt"
echo "restored_lock_sha256=$BASE_LOCK_HASH" >> "$EVIDENCE/metadata.txt"
echo "Acceptance evidence retained at $EVIDENCE"
