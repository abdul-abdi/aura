#!/usr/bin/env bash
#
# Deploy aura-proxy to Google Cloud Run.
#
# Usage:
#   bash scripts/deploy-proxy.sh                         # interactive
#   bash scripts/deploy-proxy.sh --project my-proj       # specify project
#   bash scripts/deploy-proxy.sh --project my-proj \
#       --region europe-west1 \
#       --auth-token "$(openssl rand -hex 32)"           # full non-interactive
#
# What this script does:
#   1. Enables the required GCP APIs (Cloud Run, Cloud Build, Artifact Registry).
#   2. Submits the source tree to Cloud Build which builds crates/aura-proxy/Dockerfile.
#   3. Deploys the resulting image as a Cloud Run service.
#   4. Optionally stores AURA_PROXY_AUTH_TOKEN as a Secret Manager secret and
#      mounts it into the service.
#   5. Writes the deployed WebSocket URL to ~/.config/aura/config.toml.
#
# Prerequisites:
#   - gcloud CLI installed and authenticated:  gcloud auth login
#   - Billing enabled on the GCP project.
#   - Run from the repo root (the same directory as Cargo.toml).
#
# Environment variables (all optional — use flags or interactive prompts instead):
#   AURA_CLOUD_PROJECT   GCP project ID
#   AURA_CLOUD_REGION    Cloud Run region  (default: us-central1)
#   AURA_PROXY_AUTH_TOKEN  Shared secret clients must send to connect
#                          Leave empty to run without authentication (not recommended).

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
SERVICE_NAME="aura-proxy"
REGION="${AURA_CLOUD_REGION:-us-central1}"
# Dockerfile lives inside the crate; gcloud run deploy --source resolves it
# relative to the build context (repo root).
DOCKERFILE="crates/aura-proxy/Dockerfile"

# ── Argument parsing ──────────────────────────────────────────────────────────
PROJECT_ID="${AURA_CLOUD_PROJECT:-}"
AUTH_TOKEN="${AURA_PROXY_AUTH_TOKEN:-}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --project)    PROJECT_ID="$2";  shift 2 ;;
        --region)     REGION="$2";      shift 2 ;;
        --auth-token) AUTH_TOKEN="$2";  shift 2 ;;
        -h|--help)
            sed -n '2,/^set /p' "$0" | grep '^#' | sed 's/^# \?//'
            exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

# ── Prerequisite checks ───────────────────────────────────────────────────────
if ! command -v gcloud &>/dev/null; then
    echo "Error: gcloud CLI not found."
    echo "  Install it from: https://cloud.google.com/sdk/docs/install"
    exit 1
fi

# Must be run from repo root (Cargo.toml must be present).
if [[ ! -f Cargo.toml ]]; then
    echo "Error: run this script from the repo root (where Cargo.toml lives)."
    exit 1
fi

# ── Resolve project ID ────────────────────────────────────────────────────────
if [[ -z "$PROJECT_ID" ]]; then
    PROJECT_ID=$(gcloud config get-value project 2>/dev/null || true)
fi
if [[ -z "$PROJECT_ID" ]]; then
    echo "No GCP project configured."
    read -rp "Enter your GCP project ID: " PROJECT_ID
fi
if [[ -z "$PROJECT_ID" ]]; then
    echo "Error: GCP project ID is required." >&2
    exit 1
fi

# ── Auth token reminder ───────────────────────────────────────────────────────
if [[ -z "$AUTH_TOKEN" ]]; then
    echo ""
    echo "WARNING: --auth-token not provided."
    echo "  The proxy will accept connections from anyone with a valid Gemini API key."
    echo "  To add authentication, pass --auth-token <secret> or set AURA_PROXY_AUTH_TOKEN."
    echo ""
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo "==> Deploying $SERVICE_NAME to Cloud Run"
echo "    Project:    $PROJECT_ID"
echo "    Region:     $REGION"
echo "    Dockerfile: $DOCKERFILE"
if [[ -n "$AUTH_TOKEN" ]]; then
    echo "    Auth:       token set (will be stored in Secret Manager)"
else
    echo "    Auth:       NONE (unauthenticated)"
fi
echo ""

# ── Enable required GCP APIs ──────────────────────────────────────────────────
echo "==> Enabling required GCP APIs (run.googleapis.com, cloudbuild, artifactregistry)..."
gcloud services enable \
    run.googleapis.com \
    cloudbuild.googleapis.com \
    artifactregistry.googleapis.com \
    --project "$PROJECT_ID" \
    --quiet

# ── Store auth token in Secret Manager (optional) ────────────────────────────
SECRET_FLAGS=()
if [[ -n "$AUTH_TOKEN" ]]; then
    SECRET_NAME="aura-proxy-auth-token"
    echo "==> Storing auth token in Secret Manager as '${SECRET_NAME}'..."
    gcloud services enable secretmanager.googleapis.com \
        --project "$PROJECT_ID" --quiet

    # Create or update the secret.
    if gcloud secrets describe "$SECRET_NAME" \
            --project "$PROJECT_ID" --quiet &>/dev/null; then
        printf '%s' "$AUTH_TOKEN" | gcloud secrets versions add "$SECRET_NAME" \
            --project "$PROJECT_ID" --data-file=-
    else
        printf '%s' "$AUTH_TOKEN" | gcloud secrets create "$SECRET_NAME" \
            --project "$PROJECT_ID" \
            --replication-policy automatic \
            --data-file=-
    fi

    # Mount it as an env var inside Cloud Run.
    SECRET_FLAGS=(
        "--set-secrets=AURA_PROXY_AUTH_TOKEN=${SECRET_NAME}:latest"
    )
fi

# ── Build and deploy via Cloud Build ─────────────────────────────────────────
# `--source .` uploads the repo root as the build context.
# Cloud Build locates the Dockerfile via --dockerfile (relative to context).
# `--timeout 3600` sets the Cloud Run *request* timeout to 1 h — required for
# long-lived WebSocket connections (Gemini sessions can run many minutes).
echo "==> Building image via Cloud Build and deploying to Cloud Run..."
gcloud run deploy "$SERVICE_NAME" \
    --source . \
    --dockerfile "$DOCKERFILE" \
    --project "$PROJECT_ID" \
    --region "$REGION" \
    --platform managed \
    --allow-unauthenticated \
    --port 8080 \
    --memory 256Mi \
    --cpu 1 \
    --min-instances 0 \
    --max-instances 10 \
    --timeout 3600 \
    "${SECRET_FLAGS[@]+"${SECRET_FLAGS[@]}"}" \
    --quiet

# ── Retrieve the deployed service URL ────────────────────────────────────────
PROXY_URL=$(gcloud run services describe "$SERVICE_NAME" \
    --project "$PROJECT_ID" \
    --region "$REGION" \
    --format "value(status.url)" \
    --quiet)

if [[ -z "$PROXY_URL" ]]; then
    echo "Error: could not retrieve proxy URL after deployment." >&2
    exit 1
fi

# Cloud Run returns an https:// URL; the Aura client expects wss://.
WS_URL="${PROXY_URL/https:\/\//wss:\/\/}/ws"

echo ""
echo "==> Deployment complete!"
echo "    Service URL:  $PROXY_URL"
echo "    WebSocket URL: $WS_URL"
echo ""

# ── Write proxy URL to the local Aura config ─────────────────────────────────
CONFIG_DIR="${HOME}/.config/aura"
CONFIG_FILE="${CONFIG_DIR}/config.toml"
mkdir -p "$CONFIG_DIR"

if [[ -f "$CONFIG_FILE" ]]; then
    # Remove any existing proxy_url line (portable sed: no -i '' needed).
    tmp=$(mktemp)
    grep -v '^proxy_url' "$CONFIG_FILE" > "$tmp" || true
    printf 'proxy_url = "%s"\n' "$WS_URL" >> "$tmp"
    mv "$tmp" "$CONFIG_FILE"
else
    printf 'proxy_url = "%s"\n' "$WS_URL" > "$CONFIG_FILE"
fi

echo "==> Saved proxy_url to $CONFIG_FILE"
echo "    Restart Aura to pick up the new proxy address."
echo ""
echo "Verify the deployment:"
echo "  curl ${PROXY_URL}/health"
