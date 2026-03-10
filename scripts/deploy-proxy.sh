#!/usr/bin/env bash
#
# Deploy aura-proxy to Google Cloud Run.
#
# Usage:
#   bash scripts/deploy-proxy.sh                    # interactive (prompts for project)
#   bash scripts/deploy-proxy.sh --project my-proj  # non-interactive
#
# Prerequisites:
#   - gcloud CLI installed and authenticated (gcloud auth login)
#   - Docker or Podman installed (for local builds) OR gcloud builds submit
#
# After deployment, the proxy URL is saved to ~/.config/aura/config.toml
# so the Aura client uses it automatically.

set -euo pipefail

SERVICE_NAME="aura-proxy"
REGION="${AURA_CLOUD_REGION:-us-central1}"
DOCKERFILE="crates/aura-proxy/Dockerfile"

# --- Parse args ---
PROJECT_ID=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --project) PROJECT_ID="$2"; shift 2 ;;
        --region)  REGION="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# --- Check prerequisites ---
if ! command -v gcloud &>/dev/null; then
    echo "Error: gcloud CLI not found. Install it from https://cloud.google.com/sdk/docs/install"
    exit 1
fi

# Get or prompt for project ID
if [[ -z "$PROJECT_ID" ]]; then
    PROJECT_ID=$(gcloud config get-value project 2>/dev/null || true)
    if [[ -z "$PROJECT_ID" ]]; then
        echo "No GCP project configured."
        read -rp "Enter your GCP project ID: " PROJECT_ID
        if [[ -z "$PROJECT_ID" ]]; then
            echo "Error: Project ID required."
            exit 1
        fi
    fi
fi

echo "==> Deploying $SERVICE_NAME to Cloud Run"
echo "    Project: $PROJECT_ID"
echo "    Region:  $REGION"
echo ""

# --- Enable required APIs ---
echo "==> Enabling required GCP APIs..."
gcloud services enable \
    run.googleapis.com \
    cloudbuild.googleapis.com \
    artifactregistry.googleapis.com \
    --project "$PROJECT_ID" \
    --quiet

# --- Build and deploy using Cloud Build (no local Docker needed) ---
echo "==> Building and deploying via Cloud Build..."
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
    --max-instances 3 \
    --timeout 3600 \
    --quiet

# --- Get the deployed URL ---
PROXY_URL=$(gcloud run services describe "$SERVICE_NAME" \
    --project "$PROJECT_ID" \
    --region "$REGION" \
    --format "value(status.url)" \
    --quiet)

if [[ -z "$PROXY_URL" ]]; then
    echo "Error: Failed to get proxy URL after deployment."
    exit 1
fi

# Convert https:// to wss:// for WebSocket
WS_URL="${PROXY_URL/https:\/\//wss:\/\/}/ws"

echo ""
echo "==> Proxy deployed successfully!"
echo "    URL: $WS_URL"
echo ""

# --- Save proxy URL to Aura config ---
CONFIG_DIR="${HOME}/.config/aura"
CONFIG_FILE="${CONFIG_DIR}/config.toml"
mkdir -p "$CONFIG_DIR"

if [[ -f "$CONFIG_FILE" ]]; then
    # Remove existing proxy_url line if present
    if grep -q '^proxy_url' "$CONFIG_FILE"; then
        sed -i '' '/^proxy_url/d' "$CONFIG_FILE"
    fi
    echo "proxy_url = \"$WS_URL\"" >> "$CONFIG_FILE"
else
    echo "proxy_url = \"$WS_URL\"" > "$CONFIG_FILE"
fi

echo "==> Saved proxy URL to $CONFIG_FILE"
echo "    Aura will use the proxy automatically on next launch."
echo ""
echo "Done! To verify: curl ${PROXY_URL}/health"
