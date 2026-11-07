#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="error_detection.service"
SERVICE_SRC="${SCRIPT_DIR}/${SERVICE_NAME}"
SERVICE_DEST="/etc/systemd/system/${SERVICE_NAME}"
BIN_DEST="/usr/local/bin/error_detection"

echo "==> Building error_detection (release)..."
if [[ "${EUID}" -eq 0 && -n "${SUDO_USER:-}" ]]; then
    sudo -u "${SUDO_USER}" cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
else
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
fi

BIN_SRC="${SCRIPT_DIR}/target/release/error_detection"

if [[ "${EUID}" -eq 0 ]]; then
    SUDO=""
else
    SUDO="sudo"
fi

echo "==> Installing binary to ${BIN_DEST}..."
${SUDO} install -m 0755 "${BIN_SRC}" "${BIN_DEST}"

echo "==> Copying ${SERVICE_NAME} to ${SERVICE_DEST}..."
${SUDO} install -m 0644 "${SERVICE_SRC}" "${SERVICE_DEST}"

echo "==> Reloading systemd daemon..."
${SUDO} systemctl daemon-reload

echo "==> Starting ${SERVICE_NAME} (without enabling on boot)..."
${SUDO} systemctl start "${SERVICE_NAME}"

echo "==> Service status:"
${SUDO} systemctl status "${SERVICE_NAME}" --no-pager

