#!/usr/bin/env bash
set -euo pipefail

# Aura GCP Auto-Deploy
# Deploys Firestore database + Cloud Run consolidation service
#
# Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>
# Requires: gcloud CLI authenticated (run `gcloud auth login` first)

PROJECT_ID=""
REGION="us-central1"
SERVICE_NAME="aura-consolidation"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --project) PROJECT_ID="$2"; shift 2;;
        --region) REGION="$2"; shift 2;;
        *) echo "Unknown arg: $1"; exit 1;;
    esac
done

if [[ -z "$PROJECT_ID" ]]; then
    echo "Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>"
    echo ""
    echo "Options:"
    echo "  --project   GCP project ID (required)"
    echo "  --region    GCP region (default: us-central1)"
    exit 1
fi

echo "==> Pre-flight checks..."

if [[ -z "${GEMINI_API_KEY:-}" ]]; then
    echo "Error: GEMINI_API_KEY environment variable is not set."
    echo "  export GEMINI_API_KEY=<your-api-key>"
    exit 1
fi

if [[ -z "${AURA_AUTH_TOKEN:-}" ]]; then
    echo "  AURA_AUTH_TOKEN not set — generating a random token..."
    AURA_AUTH_TOKEN=$(openssl rand -hex 32)
    echo "  Generated AURA_AUTH_TOKEN: $AURA_AUTH_TOKEN"
    echo "  (Save this — you will need it in config.toml)"
fi

echo "==> Checking gcloud CLI..."
command -v gcloud >/dev/null 2>&1 || { echo "Error: gcloud CLI not found. Install: https://cloud.google.com/sdk/docs/install"; exit 1; }

echo "==> Setting project to $PROJECT_ID..."
gcloud config set project "$PROJECT_ID"

echo "==> Enabling required APIs..."
gcloud services enable \
    firestore.googleapis.com \
    run.googleapis.com \
    artifactregistry.googleapis.com \
    cloudbuild.googleapis.com

echo "==> Creating Firestore database (Native mode)..."
gcloud firestore databases create --location="$REGION" 2>/dev/null || echo "    Firestore database already exists, skipping."

echo "==> Building and deploying Cloud Run service..."
gcloud run deploy "$SERVICE_NAME" \
    --source infrastructure/ \
    --region "$REGION" \
    --allow-unauthenticated \
    --set-env-vars "GEMINI_API_KEY=${GEMINI_API_KEY},AURA_AUTH_TOKEN=${AURA_AUTH_TOKEN},GCP_PROJECT_ID=$PROJECT_ID" \
    --memory 256Mi \
    --cpu 1 \
    --min-instances 0 \
    --max-instances 3

echo "==> Granting Firestore IAM permissions to Cloud Run service account..."
PROJECT_NUMBER=$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
    --member="serviceAccount:${PROJECT_NUMBER}-compute@developer.gserviceaccount.com" \
    --role="roles/datastore.user" \
    --quiet

CLOUD_RUN_URL=$(gcloud run services describe "$SERVICE_NAME" --region "$REGION" --format 'value(status.url)')

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Deployment complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Cloud Run URL: $CLOUD_RUN_URL"
echo "  Firestore:     projects/$PROJECT_ID/databases/(default)"
echo ""
echo "  Add to ~/.config/aura/config.toml:"
echo ""
echo "    firestore_project_id = \"$PROJECT_ID\""
echo "    cloud_run_url = \"$CLOUD_RUN_URL\""
echo "    cloud_run_auth_token = \"$AURA_AUTH_TOKEN\""
echo ""
