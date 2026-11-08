
#!/usr/bin/env bash

set -euo pipefail

# ==========================================
# Configuration
# ==========================================

INSTANCE="osfix"
PORT=8080

REMOTE_MODEL="$HOME/models/nemotron-bolt/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB.gguf"
REMOTE_LLAMA="$HOME/llama.cpp"

# ==========================================
# Check Brev
# ==========================================

if ! command -v brev >/dev/null 2>&1; then
    echo "Error: Brev CLI is not installed."
    exit 1
fi

echo "Refreshing Brev instances..."
brev refresh

# ==========================================
# Create remote startup script
# ==========================================

REMOTE_SCRIPT=$(mktemp)
trap 'rm -f "$REMOTE_SCRIPT"' EXIT

cat > "$REMOTE_SCRIPT" <<'REMOTE'
#!/usr/bin/env bash

set -euo pipefail

LLAMA_DIR="$HOME/llama.cpp"
MODEL="$HOME/models/nemotron-bolt/NVIDIA-Nemotron-3.5-Bolt-74B-A9B-24GB.gguf"
PORT=8080

SERVER="$LLAMA_DIR/build/bin/llama-server"

# Check prerequisites
if [[ ! -f "$MODEL" ]]; then
    echo "Error: Model not found: $MODEL"
    exit 1
fi

if [[ ! -x "$SERVER" ]]; then
    echo "Error: llama-server not found: $SERVER"
    exit 1
fi

# Avoid starting duplicate servers
if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null; then
    echo "Nemotron is already running!"
    exit 0
fi

# Check GPU
echo "Checking GPU..."
nvidia-smi

# Start server
echo "Starting Nemotron..."

nohup "$SERVER" \
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
    --host 127.0.0.1 \
    --port "$PORT" \
    > "$HOME/nemotron.log" 2>&1 < /dev/null &

echo $! > "$HOME/nemotron.pid"

echo "Nemotron launched!"
REMOTE

# ==========================================
# Execute remotely
# ==========================================

echo "Launching Nemotron on $INSTANCE..."

brev exec "$INSTANCE" "@$REMOTE_SCRIPT"

# ==========================================
# Wait for API
# ==========================================

echo "Waiting for Nemotron to initialize..."

for i in {1..120}; do
    if brev exec "$INSTANCE" \
        "curl -sf http://127.0.0.1:$PORT/health >/dev/null"; then
        echo "Nemotron is ready!"
        break
    fi

    if [[ "$i" -eq 120 ]]; then
        echo "Error: Nemotron did not become ready."
        echo "Check remote logs with:"
        echo "brev exec $INSTANCE 'tail -100 ~/nemotron.log'"
        exit 1
    fi

    sleep 5
done

# ==========================================
# Port forwarding
# ==========================================

echo "=========================================="
echo "Nemotron API: http://localhost:$PORT"
echo "=========================================="

brev port-forward "$INSTANCE" --port "$PORT:$PORT"


