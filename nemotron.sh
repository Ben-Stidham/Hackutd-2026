
#!/usr/bin/env bash

set -Eeuo pipefail

# ==========================================
# Configuration
# ==========================================

INSTANCE="osfix"
PORT=8080

MODEL_REL="models/nemotron-bolt/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB.gguf"
LLAMA_REL="llama.cpp"

MAX_ATTEMPTS=30
RETRY_DELAY=10
MODEL_START_TIMEOUT=900

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/.env"

# ==========================================
# Logging
# ==========================================

info() {
    echo "[INFO] $*"
}

success() {
    echo "[OK] $*"
}

error() {
    echo "[ERROR] $*" >&2
}

section() {
    echo
    echo "=========================================="
    echo "$1"
    echo "=========================================="
}

# ==========================================
# Load environment
# ==========================================

section "Loading environment"

if [[ ! -f "$ENV_FILE" ]]; then
    error ".env file not found: $ENV_FILE"
    exit 1
fi

set -a
source "$ENV_FILE"
set +a

if [[ -z "${API_KEY:-}" ]]; then
    error "API_KEY is missing from .env"
    exit 1
fi

export BREV_API_KEY="$API_KEY"
unset API_KEY

success "Environment loaded"

# ==========================================
# Check dependencies
# ==========================================

if ! command -v brev >/dev/null 2>&1; then
    error "Brev CLI is not installed."
    exit 1
fi

# ==========================================
# Prepare Brev
# ==========================================

section "Preparing Brev"

info "Refreshing instances..."

brev refresh

success "Brev has been refreshed"

# ==========================================
# Start instance
# ==========================================

section "Launching Nemotron on $INSTANCE"

info "Starting instance..."

# An already-running instance may cause brev start
# to return a nonzero status. We verify SSH below.

if ! brev start "$INSTANCE"; then
    info "Instance may already be running. Checking connection..."
fi

brev refresh

# ==========================================
# Wait for SSH
# ==========================================

CONNECTED=false

for ((i=1; i<=MAX_ATTEMPTS; i++)); do
    info "Connecting to $INSTANCE (attempt $i/$MAX_ATTEMPTS)..."

    if brev exec "$INSTANCE" "true" >/dev/null 2>&1; then
        CONNECTED=true
        break
    fi

    if (( i < MAX_ATTEMPTS )); then
        info "Connection failed. Retrying in ${RETRY_DELAY} seconds..."
        sleep "$RETRY_DELAY"
    fi
done

if [[ "$CONNECTED" != true ]]; then
    error "Could not connect to $INSTANCE."
    exit 1
fi

success "Connected to $INSTANCE"

# ==========================================
# Create remote startup script
# ==========================================

REMOTE_SCRIPT="$(mktemp)"
trap 'rm -f "$REMOTE_SCRIPT"' EXIT

cat > "$REMOTE_SCRIPT" <<'REMOTE_SCRIPT_CONTENT'
#!/usr/bin/env bash

set -Eeuo pipefail

# ==========================================
# Remote configuration
# ==========================================

MODEL="$HOME/models/nemotron-bolt/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB.gguf"
LLAMA_DIR="$HOME/llama.cpp"
LLAMA_BIN="$LLAMA_DIR/build/bin/llama-server"

PORT=8080
START_TIMEOUT=900

LOG_FILE="$LLAMA_DIR/nemotron.log"
PID_FILE="$LLAMA_DIR/nemotron.pid"

# ==========================================
# Validate installation
# ==========================================

echo "Working directory: $LLAMA_DIR"

if [[ ! -f "$MODEL" ]]; then
    echo "ERROR: Model not found: $MODEL"
    exit 1
fi

if [[ ! -x "$LLAMA_BIN" ]]; then
    echo "ERROR: llama-server not found or not executable."
    exit 1
fi

cd "$LLAMA_DIR"

# ==========================================
# Detect existing Nemotron servers
# ==========================================

echo "Checking for existing Nemotron servers..."

CORRECT_PID=""
STALE_PIDS=()

mapfile -t SERVER_PIDS < <(pgrep -x llama-server || true)

for pid in "${SERVER_PIDS[@]}"; do
    [[ -r "/proc/$pid/cmdline" ]] || continue

    CMDLINE="$(tr '\0' ' ' < "/proc/$pid/cmdline")"

    if [[ "$CMDLINE" == *"$MODEL"* &&
          "$CMDLINE" == *"--host 0.0.0.0"* &&
          "$CMDLINE" == *"--port $PORT"* ]]; then

        if [[ -z "$CORRECT_PID" ]]; then
            CORRECT_PID="$pid"
            echo "Correct Nemotron server already running (PID $pid)"
        else
            STALE_PIDS+=("$pid")
        fi
    else
        STALE_PIDS+=("$pid")
    fi
done

# ==========================================
# Stop incorrectly configured servers
# ==========================================

for pid in "${STALE_PIDS[@]}"; do
    echo "Stopping old llama-server (PID $pid)..."
    kill "$pid" 2>/dev/null || true
done

for pid in "${STALE_PIDS[@]}"; do
    for ((i=0; i<10; i++)); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
    done

    if kill -0 "$pid" 2>/dev/null; then
        echo "Force stopping PID $pid..."
        kill -9 "$pid" 2>/dev/null || true
    fi
done

# ==========================================
# Launch Nemotron
# ==========================================

if [[ -z "$CORRECT_PID" ]]; then

    echo "Starting Nemotron..."

    nohup "$LLAMA_BIN" \
        --model "$MODEL" \
        --batch-size 2048 \
        --ubatch-size 512 \
        --ctx-size 32768 \
        --parallel 1 \
        -ngl 999 \
        --temp 1.0 \
        --top-p 0.95 \
        --flash-attn on \
        --jinja \
        --host 0.0.0.0 \
        --port "$PORT" \
        > "$LOG_FILE" 2>&1 < /dev/null &

    SERVER_PID=$!
    echo "$SERVER_PID" > "$PID_FILE"

    echo "Nemotron launched (PID $SERVER_PID)"

else
    SERVER_PID="$CORRECT_PID"
    echo "Using existing Nemotron server."
fi

# ==========================================
# Wait for API readiness
# ==========================================

echo "Waiting for Nemotron API..."

READY=false
MAX_CHECKS=$((START_TIMEOUT / 5))

for ((i=1; i<=MAX_CHECKS; i++)); do

    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "ERROR: Nemotron process exited unexpectedly."
        echo "Recent server logs:"
        tail -n 80 "$LOG_FILE" 2>/dev/null || true
        exit 1
    fi

    if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
        READY=true
        break
    fi

    sleep 5
done

if [[ "$READY" != true ]]; then
    echo "ERROR: Nemotron did not become ready in time."
    echo "Recent server logs:"
    tail -n 80 "$LOG_FILE" 2>/dev/null || true
    exit 1
fi

# ==========================================
# Success
# ==========================================

echo
echo "=========================================="
echo "Nemotron is ready!"
echo "=========================================="
echo "Instance:  osfix"
echo "PID:       $SERVER_PID"
echo "Port:      $PORT"
echo "Host:      0.0.0.0"
echo "Model:     $MODEL"
echo "API:       http://localhost:$PORT/v1"
echo "Logs:      $LOG_FILE"
echo "=========================================="

REMOTE_SCRIPT_CONTENT

# ==========================================
# Execute remote script
# ==========================================

section "Starting remote Nemotron server"

brev exec "$INSTANCE" "@$REMOTE_SCRIPT"

# ==========================================
# Finished
# ==========================================

section "Nemotron successfully started"

success "Your model is ready!"

echo
echo "Instance: osfix"
echo "Remote API: http://localhost:$PORT/v1"
echo
echo "To access the API locally, open another terminal:"
echo
echo "brev port-forward $INSTANCE --port $PORT:$PORT"
echo
echo "Then access:"
echo "http://localhost:$PORT/v1"
echo

