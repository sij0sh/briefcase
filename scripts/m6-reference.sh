#!/usr/bin/env bash
# Build, deploy locally and observe the M6 reference application.
#
# Produces the retained inputs that the M6 acceptance consumes:
#   evaluation/sources/m6/deployment/staging/build/*.build.json   build manifests
#   evaluation/sources/m6/deployment/staging/build/*.oci-manifest.json
#   evaluation/sources/m6/deployment/staging/k8s/*.yaml            desired topology
#   evaluation/sources/m6/runtime/staging/traces.jsonl             OTLP JSON from the Collector
#   evaluation/sources/m6/runtime/staging/collection.json          interval/collector record
#
# The source snapshot is captured with the declared policy
# (evaluation/sources/m6/policy/architecture.json), exactly as the acceptance does.
#
# Images are real podman builds; the OCI manifest digest is the immutable
# artifact identity. Services run as containers on one network with an
# OpenTelemetry Collector (file exporter). Kubernetes resource attributes are
# supplied through OTEL_RESOURCE_ATTRIBUTES the way the downward API would; no
# cluster is required. The storefront load generator deliberately carries no
# deployment identity.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP="$ROOT/evaluation/sources/m6/reference"
OUT="$ROOT/evaluation/sources/m6"
ENVIRONMENT=staging
NAMESPACE=shop
REPOSITORY_ID=m6-reference
NODE_IMAGE=docker.io/library/node:24.10.0-bookworm-slim
COLLECTOR_IMAGE=docker.io/otel/opentelemetry-collector-contrib:0.137.0
POSTGRES_IMAGE=docker.io/library/postgres:16.10-alpine
NET=m6-reference
WORK=$(mktemp -d)
cleanup() {
  podman rm -f m6-orders m6-payments m6-otelcol m6-postgres >/dev/null 2>&1 || true
  podman network rm -f "$NET" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" --bin briefcase
SNAPSHOT=$("$ROOT/target/debug/briefcase" --db "$WORK/snapshot.db" --json scan "$APP" \
  --language typescript --calls --repository-id "$REPOSITORY_ID" --policy "$OUT/policy/architecture.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["snapshot_id"])')
echo "source snapshot $SNAPSHOT"

mkdir -p "$OUT/deployment/$ENVIRONMENT/build" "$OUT/deployment/$ENVIRONMENT/k8s" "$OUT/runtime/$ENVIRONMENT"
declare -A DIGEST CONFIG
for svc in orders payments; do
  ctx="$WORK/context-$svc"
  mkdir -p "$ctx"
  # Exactly the inputs named by Dockerfile.$svc COPY instructions.
  (cd "$APP" && cp --parents package.json package-lock.json tsconfig.build.json "Dockerfile.$svc" "$ctx/" \
    && cp -r --parents prisma src/shared "src/$svc" "$ctx/")
  podman build --quiet --pull=never -f "$ctx/Dockerfile.$svc" -t "localhost/m6/$svc:$ENVIRONMENT" "$ctx" >/dev/null
  podman push --quiet --format oci "localhost/m6/$svc:$ENVIRONMENT" "oci:$WORK/oci-$svc:$ENVIRONMENT"
  manifest=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["manifests"][0]["digest"])' "$WORK/oci-$svc/index.json")
  cp "$WORK/oci-$svc/blobs/sha256/${manifest#sha256:}" "$OUT/deployment/$ENVIRONMENT/build/$svc.oci-manifest.json"
  DIGEST[$svc]=$manifest
  CONFIG[$svc]=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["config"]["digest"])' "$OUT/deployment/$ENVIRONMENT/build/$svc.oci-manifest.json")
  python3 - "$APP" "$ctx" "$svc" "$SNAPSHOT" "$manifest" "$(podman --version)" "$(podman image inspect --format '{{.Digest}}' $NODE_IMAGE)" \
    > "$OUT/deployment/$ENVIRONMENT/build/$svc.build.json" <<'PY'
import hashlib, json, os, sys
app, ctx, svc, snapshot, digest, builder, base = sys.argv[1:]
def sha(p): return "sha256:" + hashlib.sha256(open(p, "rb").read()).hexdigest()
files = sorted(({"path": os.path.relpath(os.path.join(d, f), ctx), "digest": sha(os.path.join(d, f))}
                for d, _, fs in os.walk(ctx) for f in fs if not f.startswith("Dockerfile")), key=lambda f: f["path"])
canonical = json.dumps(files, separators=(",", ":"), sort_keys=True, ensure_ascii=False).encode()
print(json.dumps({
  "schema_version": "1.0", "kind": "briefcase-build-manifest",
  "source_snapshot_id": snapshot, "repository": f"localhost/m6/{svc}", "tag": "staging",
  "dockerfile": {"path": f"Dockerfile.{svc}", "digest": sha(os.path.join(app, f"Dockerfile.{svc}"))},
  "context_files": files, "context_digest": "sha256:" + hashlib.sha256(canonical).hexdigest(),
  "build_configuration": {"builder": builder, "platform": "linux/amd64", "base_image": f"docker.io/library/node:24.10.0-bookworm-slim@{base}", "build_args": {}},
  "image_digest": digest, "oci_manifest": f"{svc}.oci-manifest.json"}, indent=2))
PY
done

for svc in orders payments; do
  cat > "$OUT/deployment/$ENVIRONMENT/k8s/$svc.yaml" <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: $svc
  namespace: $NAMESPACE
spec:
  replicas: 1
  selector:
    matchLabels:
      app: $svc
  template:
    metadata:
      labels:
        app: $svc
        tier: backend
    spec:
      containers:
        - name: $svc
          image: localhost/m6/$svc:$ENVIRONMENT@${DIGEST[$svc]}
          ports:
            - containerPort: 8080
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: database
                  key: url
$( [ "$svc" = orders ] && printf '            - name: PAYMENTS_URL\n              value: http://payments.%s.svc.cluster.local:8080\n' "$NAMESPACE" )
---
apiVersion: v1
kind: Service
metadata:
  name: $svc
  namespace: $NAMESPACE
spec:
  selector:
    app: $svc
  ports:
    - port: 8080
      targetPort: 8080
YAML
  sed -i '/^$/d' "$OUT/deployment/$ENVIRONMENT/k8s/$svc.yaml"
done

# ---- Observed runtime ---------------------------------------------------------
podman network create "$NET" >/dev/null
mkdir -p "$WORK/otel" && chmod 777 "$WORK/otel"
cat > "$WORK/collector.yaml" <<'YAML'
receivers:
  otlp:
    protocols:
      http:
        endpoint: 0.0.0.0:4318
processors:
  batch:
    timeout: 1s
exporters:
  file:
    path: /out/traces.jsonl
service:
  pipelines:
    traces:
      receivers: [otlp]
      processors: [batch]
      exporters: [file]
YAML
podman run -d --name m6-otelcol --network "$NET" -v "$WORK/collector.yaml:/etc/otelcol/config.yaml:Z" \
  -v "$WORK/otel:/out:Z" "$COLLECTOR_IMAGE" --config /etc/otelcol/config.yaml >/dev/null
podman run -d --name m6-postgres --network "$NET" -e POSTGRES_USER=app -e POSTGRES_PASSWORD=app -e POSTGRES_DB=app \
  "$POSTGRES_IMAGE" >/dev/null
until podman exec m6-postgres pg_isready -U app -d app >/dev/null 2>&1; do sleep 1; done
sleep 2
podman exec -i m6-postgres psql -q -U app -d app < "$APP/prisma/migrations/0001_init/migration.sql"
START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
for svc in orders payments; do
  extra=""
  [ "$svc" = orders ] && extra="-e PAYMENTS_URL=http://m6-payments:8080"
  podman run -d --name "m6-$svc" --network "$NET" $extra \
    -e DATABASE_URL=postgresql://app:app@m6-postgres:5432/app \
    -e OTEL_EXPORTER_OTLP_ENDPOINT=http://m6-otelcol:4318 -e OTEL_TRACES_SAMPLER=always_on -e OTEL_BSP_SCHEDULE_DELAY=500 \
    -e OTEL_SERVICE_NAME="$svc" \
    -e OTEL_RESOURCE_ATTRIBUTES="service.namespace=$NAMESPACE,deployment.environment.name=$ENVIRONMENT,k8s.namespace.name=$NAMESPACE,k8s.deployment.name=$svc,k8s.pod.uid=$(cat /proc/sys/kernel/random/uuid),container.image.id=${DIGEST[$svc]},container.image.name=localhost/m6/$svc" \
    "localhost/m6/$svc:$ENVIRONMENT" >/dev/null
done
for svc in orders payments; do
  until podman run --rm --network "$NET" "$NODE_IMAGE" node -e "require('net').connect(8080,'m6-$svc').on('connect',()=>process.exit(0)).on('error',()=>process.exit(1))" >/dev/null 2>&1; do sleep 1; done
done
podman run --rm --network "$NET" -e OTEL_EXPORTER_OTLP_ENDPOINT=http://m6-otelcol:4318 -e OTEL_TRACES_SAMPLER=always_on \
  -e OTEL_BSP_SCHEDULE_DELAY=200 -e OTEL_SERVICE_NAME=storefront -e OTEL_RESOURCE_ATTRIBUTES=deployment.environment.name=$ENVIRONMENT \
  "localhost/m6/orders:$ENVIRONMENT" node --require ./dist/shared/telemetry.js -e "
const base = 'http://m6-orders:8080';
const post = body => fetch(base + '/orders', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
(async () => {
  const ids = [];
  for (let i = 0; i < 5; i++) ids.push((await (await post({ customerId: 'customer-' + i, amount: 12 + i })).json()).id);
  await post({ customerId: 'customer-large', amount: 2000 });
  for (const id of ids.slice(0, 2)) await fetch(base + '/orders/' + id);
  await new Promise(r => setTimeout(r, 2000));
  process.exit(0);
})();"
sleep 4
END=$(date -u +%Y-%m-%dT%H:%M:%SZ)
podman stop -t 5 m6-orders m6-payments >/dev/null
sleep 2
podman stop -t 5 m6-otelcol >/dev/null
cp "$WORK/otel/traces.jsonl" "$OUT/runtime/$ENVIRONMENT/traces.jsonl"
python3 - "$START" "$END" "$COLLECTOR_IMAGE" "$(podman image inspect --format '{{.Digest}}' $COLLECTOR_IMAGE)" "${DIGEST[orders]}" "${DIGEST[payments]}" "${CONFIG[orders]}" "${CONFIG[payments]}" \
  > "$OUT/runtime/$ENVIRONMENT/collection.json" <<'PY'
import json, sys
start, end, image, digest, orders, payments, orders_config, payments_config = sys.argv[1:]
print(json.dumps({"schema_version": "1.0", "environment": "staging", "interval_start": start, "interval_end": end,
  "collector": {"id": f"otelcol-contrib@{digest}", "image": image, "exporter": "file", "receiver": "otlp/http"},
  "sampling": "complete", "sampler": "always_on",
  "note": "Kubernetes resource attributes were supplied via OTEL_RESOURCE_ATTRIBUTES; storefront carries no deployment identity.",
  "images": {"orders": {"manifest": orders, "config": orders_config}, "payments": {"manifest": payments, "config": payments_config}}}, indent=2))
PY
echo "retained runtime interval $START .. $END"
