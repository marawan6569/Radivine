#!/usr/bin/env bash
# Display live status of the Radivine development environment (daemon, worker, profile, sockets,
# Nemo integration, installed service conflict detection, and log paths).

set -euo pipefail

# shellcheck source=scripts/dev/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

PROFILE="$(get_active_profile)"
TARGET_DIR="$(resolve_target_dir "${PROFILE}")"

printf '========================================================================\n'
printf 'Radivine Development Environment Status\n'
printf '========================================================================\n'
printf 'Workspace Root:      %s\n' "${REPO_ROOT}"
printf 'Active Profile:      %s (%s)\n' "${PROFILE}" "${TARGET_DIR}"

# 1. Daemon status
if daemon_pid="$(get_dev_daemon_pid)"; then
    daemon_exe="$(get_pid_exe "${daemon_pid}" || printf '%s/radivine' "${TARGET_DIR}")"
    printf 'Dev Daemon:          RUNNING (PID %s)\n' "${daemon_pid}"
    printf '  Executable:        %s\n' "${daemon_exe}"
else
    printf 'Dev Daemon:          STOPPED\n'
    printf '  Expected Binary:   %s/radivine\n' "${TARGET_DIR}"
fi
printf '  Daemon Socket:     %s\n' "${DEV_DAEMON_SOCKET}"

# 2. Worker status
if worker_pid="$(get_dev_worker_pid)"; then
    worker_exe="$(get_pid_exe "${worker_pid}" || printf '%s/radivine-worker' "${TARGET_DIR}")"
    printf 'Dev Worker:          RUNNING (PID %s)\n' "${worker_pid}"
    printf '  Executable:        %s\n' "${worker_exe}"
else
    printf 'Dev Worker:          IDLE / STOPPED (launches on demand or via dev-start.sh)\n'
    printf '  Expected Binary:   %s/radivine-worker\n' "${TARGET_DIR}"
fi
printf '  Worker Socket:     %s\n' "${DEV_WORKER_SOCKET}"
printf '  Jobs Database:     %s\n' "${DEV_WORKER_DB}"

# 3. Control Center & Viewer binaries
if control_pid="$(get_dev_control_pid)"; then
    printf 'Dev Control Center:  RUNNING (PID %s, %s/radivine-control)\n' "${control_pid}" "${TARGET_DIR}"
else
    printf 'Dev Control Center:  %s/radivine-control\n' "${TARGET_DIR}"
fi
printf '  Control Socket:    %s\n' "${DEV_CONTROL_SOCKET}"
printf 'Dev Viewer Binary:   %s/radivine-viewer\n' "${TARGET_DIR}"

# 4. Isolated configuration & logs
printf 'Isolated Config Dir: %s/radivine\n' "${DEV_CONFIG_HOME}"
printf 'Daemon Log:          %s\n' "${DEV_DAEMON_LOG}"
printf 'Worker Log:          %s\n' "${DEV_WORKER_LOG}"

# 5. Nemo integration status
"${DEV_SCRIPTS_DIR}/dev-nemo.sh" status

# 6. Installed service conflict check
INSTALLED_CONFLICT=0
INSTALLED_DETAIL=""
if is_installed_systemd_service_active; then
    INSTALLED_CONFLICT=1
    INSTALLED_DETAIL="systemd radivine.service is active"
fi
if sys_pid="$(get_system_socket_daemon_pid)"; then
    if ! is_pid_owned_by_workspace "${sys_pid}"; then
        INSTALLED_CONFLICT=1
        sys_exe="$(get_pid_exe "${sys_pid}" || printf '/usr/bin/radivine')"
        INSTALLED_DETAIL="${INSTALLED_DETAIL:+${INSTALLED_DETAIL}; }PID ${sys_pid} (${sys_exe}) listening on ${SYSTEM_RUNTIME_SOCKET}"
    fi
fi

if [[ "${INSTALLED_CONFLICT}" -eq 1 ]]; then
    printf 'Installed Service:   CONFLICT DETECTED (%s)\n' "${INSTALLED_DETAIL}"
    printf '                     Run `./scripts/dev/dev-start.sh --stop-installed` to resolve.\n'
else
    if [[ -f "${DEV_INSTALLED_STATE_FILE}" ]]; then
        printf 'Installed Service:   INACTIVE (stopped by dev mode; restore with `./scripts/dev/dev-stop.sh --restore-installed`)\n'
    else
        printf 'Installed Service:   NO CONFLICT (radivine.service is inactive)\n'
    fi
fi
printf '========================================================================\n'
