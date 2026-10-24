#!/usr/bin/env bash
# Gracefully stop the existing development instance, rebuild changed workspace binaries,
# restart the development daemon and worker, and verify readiness.

set -euo pipefail

# shellcheck source=scripts/dev/common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
    cat <<'EOF'
Usage:
  ./scripts/dev/dev-restart.sh [OPTIONS]

Options:
  --release              Rebuild and restart using the release profile (default if no profile active)
  --debug                Rebuild and restart using the debug profile
  --skip-build           Skip `cargo build` and restart using existing binaries
  --control              Launch the development Radivine Control Center window after restart
  --page <NAME>          Launch Control Center directly to <NAME> after restart
  --enable-nemo          Enable/refresh development Nemo right-click integration
  --stop-installed       Stop active installed `radivine.service` if running
  --idle-timeout <SECS>  Worker idle timeout in seconds (default: 3600)
  -h, --help             Show this help message
EOF
}

PROFILE="$(get_active_profile)"
SKIP_BUILD=0
FORWARD_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            PROFILE="release"
            FORWARD_ARGS+=("--release")
            shift
            ;;
        --debug)
            PROFILE="debug"
            FORWARD_ARGS+=("--debug")
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            FORWARD_ARGS+=("--skip-build")
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            FORWARD_ARGS+=("$1")
            shift
            ;;
    esac
done

# 1. Stop existing development processes cleanly
dev_info "Stopping existing development instance before restart..."
"${DEV_SCRIPTS_DIR}/dev-stop.sh"

# 2. Rebuild workspace binaries (unless --skip-build was passed)
if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    dev_info "Rebuilding changed workspace binaries (${PROFILE} profile)..."
    (
        cd "${REPO_ROOT}"
        if [[ "${PROFILE}" == "release" ]]; then
            if ! cargo build --release \
                -p radivine \
                -p radivine-worker \
                -p radivine-control \
                -p radivine-viewer; then
                dev_err "Cargo build (--release) failed; development services were not restarted."
                exit 1
            fi
        else
            if ! cargo build \
                -p radivine \
                -p radivine-worker \
                -p radivine-control \
                -p radivine-viewer; then
                dev_err "Cargo build (--debug) failed; development services were not restarted."
                exit 1
            fi
        fi
    )
fi

# 3. Start freshly built binaries and verify readiness
dev_info "Starting updated development instance (${PROFILE} profile)..."
exec "${DEV_SCRIPTS_DIR}/dev-start.sh" "--${PROFILE}" --skip-build "${FORWARD_ARGS[@]}"
