#!/usr/bin/env bash
# Gracefully stop the Radivine development daemon and worker without affecting unrelated processes
# or deleting configuration, job history, or converted files.

set -euo pipefail

# shellcheck source=scripts/dev/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
    cat <<'EOF'
Usage:
  ./scripts/dev/dev-stop.sh [OPTIONS]

Options:
  --restore-nemo        Remove the development Nemo action override and restore original Nemo state
  --restore-installed   Restart the installed `radivine.service` if it was stopped for dev mode
  --all                 Stop dev processes, restore Nemo integration, and restart installed service
  -h, --help            Show this help message
EOF
}

RESTORE_NEMO=0
RESTORE_INSTALLED=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --restore-nemo)
            RESTORE_NEMO=1
            shift
            ;;
        --restore-installed)
            RESTORE_INSTALLED=1
            shift
            ;;
        --all)
            RESTORE_NEMO=1
            RESTORE_INSTALLED=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            dev_err "Unknown option: $1"
            usage
            exit 2
            ;;
    esac
done

PROFILE="$(get_active_profile)"
TARGET_DIR="$(resolve_target_dir "${PROFILE}")"
DAEMON_BIN="${TARGET_DIR}/radivine"
WORKER_BIN="${TARGET_DIR}/radivine-worker"

# 1. Stop development Control Center window if tracked and owned by this workspace
if control_pid="$(get_dev_control_pid)"; then
    dev_info "Closing development Control Center (PID ${control_pid})..."
    kill -TERM "${control_pid}" 2>/dev/null || true
    for _ in $(seq 1 20); do
        if ! kill -0 "${control_pid}" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
fi
rm -f "${DEV_CONTROL_PID_FILE}" "${DEV_CONTROL_SOCKET}"

# 2. Gracefully stop development daemon (`radivine quit` over dev socket + verified PID check)
daemon_stopped=0
if daemon_pid="$(get_dev_daemon_pid)"; then
    dev_info "Stopping development daemon (PID ${daemon_pid})..."
    if [[ -x "${DAEMON_BIN}" && -S "${DEV_DAEMON_SOCKET}" ]]; then
        "${DAEMON_BIN}" --socket "${DEV_DAEMON_SOCKET}" quit >/dev/null 2>&1 || true
    fi
    for _ in $(seq 1 30); do
        if ! kill -0 "${daemon_pid}" 2>/dev/null; then
            daemon_stopped=1
            break
        fi
        sleep 0.1
    done
    if [[ "${daemon_stopped}" -eq 0 ]] && is_pid_owned_by_workspace "${daemon_pid}"; then
        kill -TERM "${daemon_pid}" 2>/dev/null || true
        for _ in $(seq 1 20); do
            if ! kill -0 "${daemon_pid}" 2>/dev/null; then
                daemon_stopped=1
                break
            fi
            sleep 0.1
        done
    fi
    dev_ok "Development daemon stopped (PID ${daemon_pid})"
else
    dev_info "Development daemon is not running"
fi
rm -f "${DEV_DAEMON_PID_FILE}" "${DEV_DAEMON_SOCKET}"

# 3. Gracefully stop development worker (`shutdown-if-idle` + cooperative SIGTERM on verified workspace PID)
worker_stopped=0
if worker_pid="$(get_dev_worker_pid)"; then
    dev_info "Stopping development worker (PID ${worker_pid})..."
    if [[ -x "${WORKER_BIN}" && -S "${DEV_WORKER_SOCKET}" ]]; then
        "${WORKER_BIN}" --socket "${DEV_WORKER_SOCKET}" shutdown-if-idle >/dev/null 2>&1 || true
    fi
    for _ in $(seq 1 20); do
        if ! kill -0 "${worker_pid}" 2>/dev/null; then
            worker_stopped=1
            break
        fi
        sleep 0.1
    done
    if [[ "${worker_stopped}" -eq 0 ]] && is_pid_owned_by_workspace "${worker_pid}"; then
        kill -TERM "${worker_pid}" 2>/dev/null || true
        for _ in $(seq 1 30); do
            if ! kill -0 "${worker_pid}" 2>/dev/null; then
                worker_stopped=1
                break
            fi
            sleep 0.1
        done
    fi
    dev_ok "Development worker stopped (PID ${worker_pid})"
else
    dev_info "Development worker is not running"
fi
rm -f "${DEV_WORKER_PID_FILE}" "${DEV_WORKER_SOCKET}" "${DEV_WORKER_LOCK}"

# 4. Optionally restore Nemo integration
if [[ "${RESTORE_NEMO}" -eq 1 ]]; then
    "${DEV_SCRIPTS_DIR}/dev-nemo.sh" restore
fi

# 5. Optionally restart installed `radivine.service`
if [[ "${RESTORE_INSTALLED}" -eq 1 ]]; then
    if command -v systemctl >/dev/null 2>&1; then
        dev_info "Restarting installed radivine.service..."
        if systemctl --user start radivine.service; then
            rm -f "${DEV_INSTALLED_STATE_FILE}"
            dev_ok "Installed radivine.service started"
        else
            dev_warn "Could not start radivine.service via systemctl --user"
        fi
    fi
fi
