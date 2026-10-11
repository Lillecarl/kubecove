#!/usr/bin/env bash
# Run on a Linux Docker host. No network, cluster or startup credential output.
set -euo pipefail
umask 077
image="${1:?image required}"
variant="${2:?full or slim required}"
case "$variant" in full) root_status=200 ;; slim) root_status=404 ;; *) exit 1 ;; esac
temporary="$(mktemp -d)"
container=""
cleanup() {
  if [[ -n "$container" ]]; then docker rm -f "$container" >/dev/null; fi
  # The mounted private directory belongs to the image user.
  docker run --rm --network none --user 0 --entrypoint /bin/sh \
    --mount "type=bind,src=$temporary,dst=/check" "$image" -c 'rm -rf /check/*'
  rmdir "$temporary"
}
trap cleanup EXIT
test "$(docker image inspect --format '{{.Config.User}}' "$image")" = '65534:65534'
docker run --rm --network none --user 0 --entrypoint /bin/sh \
  --mount "type=bind,src=$temporary,dst=/check" "$image" -c 'chown 65534:65534 /check; chmod 700 /check'
container="$(docker run -d --network none --log-driver none --read-only --cap-drop ALL \
  --security-opt no-new-privileges --mount "type=bind,src=$temporary,dst=/data/kubecove" \
  "$image" --socket /data/kubecove/check.sock --public-origin http://localhost:8080 \
  --data-dir /data/kubecove)"
while [[ ! -S "$temporary/check.sock" ]]; do
  test "$(docker inspect --format '{{.State.Running}}' "$container")" = true
  sleep 1
done
curl --silent --show-error \
  --unix-socket "$temporary/check.sock" http://localhost:8080/api/v1/auth > "$temporary/auth.json"
grep -q '"mode":"token"' "$temporary/auth.json"
status() {
  curl --silent --show-error --unix-socket "$temporary/check.sock" \
    --output /dev/null --write-out '%{http_code}' "$@"
}
test "$(status http://localhost:8080/)" = "$root_status"
test "$(status -H 'Host: untrusted.example' http://localhost:8080/)" = 403
test "$(status -X POST -H 'Origin: http://localhost:8080' -H 'Content-Type: application/json' \
  -d '{}' http://localhost:8080/api/v1/session)" = 401
docker exec "$container" /bin/sh -ec '
  test "$(id -u)" = 65534
  test -s /etc/ssl/certs/ca-certificates.crt
  test -x /usr/local/bin/kubeconform
  test "$(stat -c %a /data/kubecove/server-token)" = 600
  ! command -v cargo
  ! command -v bun
'
# Feed credentials through stdin, never through command arguments or logs.
docker exec "$container" cat /data/kubecove/server-token | \
  { read -r token || test -n "$token"; printf '{"token":"%s"}' "$token"; } > "$temporary/login.json"
test "$(status -X POST -H 'Origin: http://localhost:8080' -H 'Content-Type: application/json' \
  --data-binary @"$temporary/login.json" http://localhost:8080/api/v1/session)" = 200
docker stop "$container" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$container")" = 0
printf '%s image: non-root, read-only runtime, CA, sidecar, UI selection, authentication and shutdown passed\n' "$variant"
