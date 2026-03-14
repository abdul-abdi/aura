#!/usr/bin/env bash
set -euo pipefail

# Aura GCP Auto-Deploy
# Deploys Firestore database + Cloud Run consolidation service
#
# Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>
# Requires: gcloud CLI authenticated (run `gcloud auth login` first)

PROJECT_ID=""
REGION="us-central1"
ENVIRONMENT="staging"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --project) PROJECT_ID="$2"; shift 2;;
        --region) REGION="$2"; shift 2;;
        --environment) ENVIRONMENT="$2"; shift 2;;
        *) echo "Unknown arg: $1"; exit 1;;
    esac
done

if [[ -z "$PROJECT_ID" ]]; then
    echo "Usage: ./scripts/deploy-gcp.sh --project <GCP_PROJECT_ID>"
    echo ""
    echo "Options:"
    echo "  --project       GCP project ID (required)"
    echo "  --region        GCP region (default: us-central1)"
    echo "  --environment   staging or production (default: staging)"
    exit 1
fi

# Derive service name from environment
case "$ENVIRONMENT" in
    production|prod) ENV_SUFFIX="prod" ;;
    staging)         ENV_SUFFIX="staging" ;;
    *) echo "Error: --environment must be 'staging' or 'production'" >&2; exit 1 ;;
esac
SERVICE_NAME="aura-memory-agent-${ENV_SUFFIX}"

echo "==> Pre-flight checks..."

if [[ -z "${GEMINI_API_KEY:-}" ]]; then
    echo "Error: GEMINI_API_KEY environment variable is not set."
    echo "  export GEMINI_API_KEY=<your-api-key>"
    exit 1
fi

if [[ -z "${AURA_AUTH_TOKEN:-}" ]]; then
    echo "  AURA_AUTH_TOKEN not set — generating a random token..."
    AURA_AUTH_TOKEN=$(openssl rand -hex 32)
    echo "  Generated AURA_AUTH_TOKEN (saved to Secret Manager, retrieve with gcloud)"
    echo "  (Set cloud_run_auth_token in config.toml to this value)"
fi

echo "==> Checking gcloud CLI..."
command -v gcloud >/dev/null 2>&1 || { echo "Error: gcloud CLI not found. Install: https://cloud.google.com/sdk/docs/install"; exit 1; }

echo "==> Enabling required APIs..."
gcloud services enable \
    firestore.googleapis.com \
    run.googleapis.com \
    artifactregistry.googleapis.com \
    cloudbuild.googleapis.com \
    secretmanager.googleapis.com \
    --project "$PROJECT_ID"

echo "==> Creating Firestore database (Native mode)..."
if ! gcloud firestore databases describe --project "$PROJECT_ID" &>/dev/null; then
    gcloud firestore databases create --location="$REGION" --project "$PROJECT_ID" || {
        echo "ERROR: Failed to create Firestore database. Check permissions and billing."
        exit 1
    }
else
    echo "    Firestore database already exists, skipping."
fi

echo "==> Storing secrets in Secret Manager..."

# Gemini API key
SECRET_NAME="gemini-api-key"
if gcloud secrets describe "$SECRET_NAME" --project "$PROJECT_ID" --quiet &>/dev/null; then
    printf '%s' "$GEMINI_API_KEY" | gcloud secrets versions add "$SECRET_NAME" \
        --project "$PROJECT_ID" --data-file=-
else
    printf '%s' "$GEMINI_API_KEY" | gcloud secrets create "$SECRET_NAME" \
        --project "$PROJECT_ID" --replication-policy automatic --data-file=-
fi

# Consolidation auth token
SECRET_NAME="aura-consolidation-auth-token"
if gcloud secrets describe "$SECRET_NAME" --project "$PROJECT_ID" --quiet &>/dev/null; then
    printf '%s' "$AURA_AUTH_TOKEN" | gcloud secrets versions add "$SECRET_NAME" \
        --project "$PROJECT_ID" --data-file=-
else
    printf '%s' "$AURA_AUTH_TOKEN" | gcloud secrets create "$SECRET_NAME" \
        --project "$PROJECT_ID" --replication-policy automatic --data-file=-
fi

# Grant Cloud Run service account access to secrets
PROJECT_NUMBER=$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')
for secret in gemini-api-key aura-consolidation-auth-token; do
    gcloud secrets add-iam-policy-binding "$secret" \
        --project "$PROJECT_ID" \
        --member="serviceAccount:${PROJECT_NUMBER}-compute@developer.gserviceaccount.com" \
        --role="roles/secretmanager.secretAccessor" \
        --quiet
done

echo "==> Building and deploying Cloud Run service..."
gcloud run deploy "$SERVICE_NAME" \
    --source infrastructure/memory-agent/ \
    --project "$PROJECT_ID" \
    --region "$REGION" \
    --allow-unauthenticated \
    --set-secrets="GEMINI_API_KEY=gemini-api-key:latest,AURA_AUTH_TOKEN=aura-consolidation-auth-token:latest" \
    --set-env-vars "GCP_PROJECT_ID=$PROJECT_ID" \
    --memory 512Mi \
    --cpu 1 \
    --min-instances 0 \
    --max-instances 5

echo "==> Granting Firestore IAM permissions to Cloud Run service account..."
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
    --member="serviceAccount:${PROJECT_NUMBER}-compute@developer.gserviceaccount.com" \
    --role="roles/datastore.user" \
    --quiet

CLOUD_RUN_URL=$(gcloud run services describe "$SERVICE_NAME" --project "$PROJECT_ID" --region "$REGION" --format 'value(status.url)')

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
