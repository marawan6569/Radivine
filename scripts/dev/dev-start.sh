#!/usr/bin/env bash
# Build and start the isolated Radivine development environment (daemon, worker, optional Control Center & Nemo action).

set -euo pipefail

# shellcheck source=scripts/dev/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
    cat <<'EOF'
Usage:
  ./scripts/dev/dev-start.sh [OPTIONS]

Options:
  --release              Build and run optimized release binaries (default)
  --debug                Build and run unoptimized debug binaries
  --skip-build           Skip `cargo build` and use existing binaries in target/<profile>/
  --control              Launch the development Radivine Control Center window
  --page <NAME>          Launch Control Center directly to <NAME> (dashboard, general, wheel,
                         actions, conversion, jobs, diagnostics, about)
  --enable-nemo          Enable development Nemo right-click integration (`Open in Radivine (Dev)`)
  --stop-installed       Temporarily stop an active installed `radivine.service` so it does not
                         conflict with the development X11 drag listener
  --idle-timeout <SECS>  Worker idle timeout in seconds (default: 3600)
  -h, --help             Show this help message
EOF
}

PROFILE="$(get_active_profile)"
SKIP_BUILD=0
OPEN_CONTROL=0
CONTROL_PAGE=""
ENABLE_NEMO=0
STOP_INSTALLED=0
WORKER_IDLE_SECS=3600

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            PROFILE="release"
            shift
            ;;
        --debug)
            PROFILE="debug"
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --control|--open-control)
            OPEN_CONTROL=1
            shift
            ;;
        --page)
            if [[ $# -lt 2 ]]; then
                dev_err "--page requires a page name"
                exit 2
            fi
            OPEN_CONTROL=1
            CONTROL_PAGE="$2"
            shift 2
            ;;
        --enable-nemo)
            ENABLE_NEMO=1
            shift
            ;;
        --stop-installed)
            STOP_INSTALLED=1
            shift
            ;;
        --idle-timeout)
            if [[ $# -lt 2 ]]; then
                dev_err "--idle-timeout requires seconds"
                exit 2
            fi
            WORKER_IDLE_SECS="$2"
            shift 2
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

ensure_dev_dirs

# 1. Check for conflicting installed `radivine.service` or system daemon on X11
INSTALLED_ACTIVE=0
INSTALLED_PID=""
if is_installed_systemd_service_active; then
    INSTALLED_ACTIVE=1
fi
if sys_pid="$(get_system_socket_daemon_pid)"; then
    # Make sure this PID isn't our own dev daemon
    if ! is_pid_owned_by_workspace "${sys_pid}"; then
        INSTALLED_ACTIVE=1
        INSTALLED_PID="${sys_pid}"
    fi
fi

if [[ "${INSTALLED_ACTIVE}" -eq 1 ]]; then
    if [[ "${STOP_INSTALLED}" -eq 1 ]]; then
        dev_info "Stopping active installed Radivine service to prevent X11 drag listener conflicts..."
        printf 'was_active=1\n' > "${DEV_INSTALLED_STATE_FILE}"
        if command -v systemctl >/dev/null 2>&1; then
            systemctl --user stop radivine.service 2>/dev/null || true
        fi
        if [[ -n "${INSTALLED_PID}" ]] && kill -0 "${INSTALLED_PID}" 2>/dev/null; then
            /usr/bin/radivine --socket "${SYSTEM_RUNTIME_SOCKET}" quit 2>/dev/null || true
        fi
        dev_ok "Installed Radivine service stopped (can be restored with ./scripts/dev/dev-stop.sh --restore-installed)"
    else
        dev_err "An installed Radivine daemon is currently active (radivine.service / ${SYSTEM_RUNTIME_SOCKET}${INSTALLED_PID:+, PID ${INSTALLED_PID}})."
        cat >&2 <<'EOF'
Running both the installed daemon and the development daemon on the same X11 session
will cause both processes to intercept Drag + Shift events in Nemo.

To temporarily stop the installed service and start the development environment, run:
  ./scripts/dev/dev-start.sh --stop-installed --enable-nemo

Or stop it manually with:
  systemctl --user stop radivine.service
EOF
        exit 1
    fi
fi

# 2. Build workspace binaries (incremental Cargo cache reused automatically)
TARGET_DIR="$(resolve_target_dir "${PROFILE}")"
if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    dev_info "Building workspace binaries (${PROFILE} profile)..."
    (
        cd "${REPO_ROOT}"
        if [[ "${PROFILE}" == "release" ]]; then
            cargo build --release \
                -p radivine \
                -p radivine-worker \
                -p radivine-control \
                -p radivine-viewer
        else
            cargo build \
                -p radivine \
                -p radivine-worker \
                -p radivine-control \
                -p radivine-viewer
        fi
    )
fi

for bin_name in radivine radivine-worker radivine-control radivine-viewer; do
    if [[ ! -x "${TARGET_DIR}/${bin_name}" ]]; then
        dev_err "Expected binary not found or not executable: ${TARGET_DIR}/${bin_name}"
        exit 1
    fi
done

# 3. If a dev daemon is already running from a DIFFERENT profile, stop it cleanly before switching
if existing_pid="$(get_dev_daemon_pid)"; then
    existing_exe="$(get_pid_exe "${existing_pid}" || true)"
    if [[ -n "${existing_exe}" && "${existing_exe}" != "${TARGET_DIR}/radivine" ]]; then
        dev_info "Switching active build profile (${existing_exe} -> ${TARGET_DIR}/radivine); stopping previous dev instance..."
        "${DEV_SCRIPTS_DIR}/dev-stop.sh"
    fi
fi

# 4. Write and load development environment variables
write_dev_env_file "${PROFILE}"
load_dev_env "${PROFILE}"

# 5. Start or verify the development daemon (`radivine daemon`)
if daemon_pid="$(get_dev_daemon_pid)"; then
    dev_ok "Development daemon already running (PID ${daemon_pid}, ${TARGET_DIR}/radivine)"
else
    rm -f "${DEV_DAEMON_SOCKET}" "${DEV_DAEMON_PID_FILE}"
    dev_info "Starting development daemon (${RADIVINE_DAEMON_BIN})..."
    (
        if [[ -z "${DISPLAY:-}" ]]; then
            export RADIVINE_HEADLESS_DAEMON="1"
        fi
        nohup "${RADIVINE_DAEMON_BIN}" --socket "${DEV_DAEMON_SOCKET}" daemon \
            >> "${DEV_DAEMON_LOG}" 2>&1 < /dev/null &
        printf '%s\n' "$!" > "${DEV_DAEMON_PID_FILE}"
    )

    # Poll IPC readiness up to 5 seconds
    READY=0
    for _ in $(seq 1 50); do
        if "${RADIVINE_DAEMON_BIN}" --socket "${DEV_DAEMON_SOCKET}" status --json >/dev/null 2>&1; then
            READY=1
            break
        fi
        sleep 0.1
    done

    if [[ "${READY}" -ne 1 ]]; then
        dev_err "Development daemon failed to become ready on ${DEV_DAEMON_SOCKET}. Recent log output (${DEV_DAEMON_LOG}):"
        tail -n 25 "${DEV_DAEMON_LOG}" >&2 2>/dev/null || true
        exit 1
    fi
    daemon_pid="$(get_dev_daemon_pid)"
    dev_ok "Development daemon ready (PID ${daemon_pid})"
fi

# 6. Start or verify the development worker (`radivine-worker serve`)
if worker_pid="$(get_dev_worker_pid)"; then
    dev_ok "Development worker already running (PID ${worker_pid}, ${TARGET_DIR}/radivine-worker)"
else
    rm -f "${DEV_WORKER_SOCKET}" "${DEV_WORKER_PID_FILE}"
    dev_info "Starting development worker (${RADIVINE_WORKER_BIN})..."
    (
        nohup "${RADIVINE_WORKER_BIN}" \
            --socket "${DEV_WORKER_SOCKET}" \
            --db "${DEV_WORKER_DB}" \
            --lock "${DEV_WORKER_LOCK}" \
            serve --idle-timeout "${WORKER_IDLE_SECS}" \
            >> "${DEV_WORKER_LOG}" 2>&1 < /dev/null &
        printf '%s\n' "$!" > "${DEV_WORKER_PID_FILE}"
    )

    WORKER_READY=0
    for _ in $(seq 1 50); do
        status_out="$("${RADIVINE_WORKER_BIN}" --socket "${DEV_WORKER_SOCKET}" status --json 2>/dev/null || true)"
        if printf '%s\n' "${status_out}" | grep -q '"running"[[:space:]]*:[[:space:]]*true'; then
            WORKER_READY=1
            break
        fi
        sleep 0.1
    done

    if [[ "${WORKER_READY}" -ne 1 ]]; then
        dev_err "Development worker failed to become ready on ${DEV_WORKER_SOCKET}. Recent log output (${DEV_WORKER_LOG}):"
        tail -n 25 "${DEV_WORKER_LOG}" >&2 2>/dev/null || true
        exit 1
    fi
    worker_pid="$(get_dev_worker_pid)"
    dev_ok "Development worker ready (PID ${worker_pid})"
fi

# 7. Enable or refresh Nemo integration if requested (or if already enabled)
if [[ "${ENABLE_NEMO}" -eq 1 ]] || is_nemo_dev_enabled; then
    "${DEV_SCRIPTS_DIR}/dev-nemo.sh" enable "--${PROFILE}"
fi

# 8. Optionally launch Control Center in the development environment
if [[ "${OPEN_CONTROL}" -eq 1 ]]; then
    dev_info "Launching development Control Center (${RADIVINE_CONTROL_BIN})..."
    control_args=(
        "--socket" "${DEV_DAEMON_SOCKET}"
        "--control-socket" "${DEV_CONTROL_SOCKET}"
    )
    if [[ -n "${CONTROL_PAGE}" ]]; then
        control_args+=("--page" "${CONTROL_PAGE}")
    fi
    (
        nohup "${RADIVINE_CONTROL_BIN}" "${control_args[@]}" \
            >/dev/null 2>&1 < /dev/null &
        printf '%s\n' "$!" > "${DEV_CONTROL_PID_FILE}"
    )
    dev_ok "Launched Radivine Control Center${CONTROL_PAGE:+ (--page ${CONTROL_PAGE})}"
fi

printf '\n'
"${DEV_SCRIPTS_DIR}/dev-status.sh"
