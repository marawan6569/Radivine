#!/usr/bin/env bash
# Build `target/debian/radivine_0.2.0_amd64.deb` and `target/debian/SHA256SUMS.txt` (`IMPLEMENTATION_PLAN.md` §3, `AC-C5.5`).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PKG_NAME="radivine"
PKG_VERSION="0.2.0"
PKG_ARCH="amd64"
DEB_BASENAME="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}"
OUT_DIR="${REPO_ROOT}/target/debian"
STAGE_DIR="${OUT_DIR}/${DEB_BASENAME}"
DEB_PATH="${OUT_DIR}/${DEB_BASENAME}.deb"

echo "==> Building release binaries (radivine, radivine-viewer, radivine-control, radivine-worker)..."
DISPLAY="" cargo build --release -p radivine -p radivine-viewer -p radivine-control -p radivine-worker

rm -rf "$STAGE_DIR"
mkdir -p \
    "${STAGE_DIR}/DEBIAN" \
    "${STAGE_DIR}/usr/bin" \
    "${STAGE_DIR}/usr/lib/systemd/user" \
    "${STAGE_DIR}/usr/share/applications" \
    "${STAGE_DIR}/usr/share/nemo/actions" \
    "${STAGE_DIR}/usr/share/icons/hicolor/256x256/apps" \
    "${STAGE_DIR}/usr/share/icons/hicolor/512x512/apps" \
    "${STAGE_DIR}/usr/share/doc/radivine/examples" \
    "${STAGE_DIR}/usr/share/man/man1"

echo "==> Installing and stripping binaries..."
install -m 0755 "${REPO_ROOT}/target/release/radivine" "${STAGE_DIR}/usr/bin/radivine"
install -m 0755 "${REPO_ROOT}/target/release/radivine-viewer" "${STAGE_DIR}/usr/bin/radivine-viewer"
install -m 0755 "${REPO_ROOT}/target/release/radivine-control" "${STAGE_DIR}/usr/bin/radivine-control"
install -m 0755 "${REPO_ROOT}/target/release/radivine-worker" "${STAGE_DIR}/usr/bin/radivine-worker"
strip --strip-unneeded \
    "${STAGE_DIR}/usr/bin/radivine" \
    "${STAGE_DIR}/usr/bin/radivine-viewer" \
    "${STAGE_DIR}/usr/bin/radivine-control" \
    "${STAGE_DIR}/usr/bin/radivine-worker"

echo "==> Installing desktop entry, systemd user units, and Nemo action..."
install -m 0644 "${REPO_ROOT}/packaging/radivine.desktop" "${STAGE_DIR}/usr/share/applications/radivine.desktop"
install -m 0644 "${REPO_ROOT}/packaging/radivine.service" "${STAGE_DIR}/usr/lib/systemd/user/radivine.service"
install -m 0644 "${REPO_ROOT}/packaging/radivine-worker.service" "${STAGE_DIR}/usr/lib/systemd/user/radivine-worker.service"
install -m 0644 "${REPO_ROOT}/packaging/radivine.nemo_action" "${STAGE_DIR}/usr/share/nemo/actions/radivine.nemo_action"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "${STAGE_DIR}/usr/share/applications/radivine.desktop"
fi

echo "==> Generating 256x256 and 512x512 hicolor icons from assets/icons/app-icon.png (read-only)..."
python3 - "${REPO_ROOT}/assets/icons/app-icon.png" \
    "${STAGE_DIR}/usr/share/icons/hicolor/256x256/apps/radivine.png" \
    "${STAGE_DIR}/usr/share/icons/hicolor/512x512/apps/radivine.png" <<'PY'
import sys
from PIL import Image

src, out256, out512 = sys.argv[1], sys.argv[2], sys.argv[3]
with Image.open(src) as im:
    im.resize((256, 256), Image.Resampling.LANCZOS).save(out256, format="PNG")
    im.resize((512, 512), Image.Resampling.LANCZOS).save(out512, format="PNG")
PY
chmod 0644 \
    "${STAGE_DIR}/usr/share/icons/hicolor/256x256/apps/radivine.png" \
    "${STAGE_DIR}/usr/share/icons/hicolor/512x512/apps/radivine.png"

echo "==> Installing documentation, copyright, changelog, and opt-in examples..."
install -m 0644 "${REPO_ROOT}/packaging/debian/copyright" "${STAGE_DIR}/usr/share/doc/radivine/copyright"
gzip -9nc "${REPO_ROOT}/packaging/debian/changelog" > "${STAGE_DIR}/usr/share/doc/radivine/changelog.gz"
chmod 0644 "${STAGE_DIR}/usr/share/doc/radivine/changelog.gz"

install -m 0644 "${REPO_ROOT}/packaging/config.example.toml" "${STAGE_DIR}/usr/share/doc/radivine/examples/config.example.toml"
install -m 0644 "${REPO_ROOT}/packaging/convert.example.toml" "${STAGE_DIR}/usr/share/doc/radivine/examples/convert.example.toml"
install -m 0644 "${REPO_ROOT}/packaging/actions.example.toml" "${STAGE_DIR}/usr/share/doc/radivine/examples/actions.example.toml"
install -m 0644 "${REPO_ROOT}/packaging/radivine-daemon.desktop" "${STAGE_DIR}/usr/share/doc/radivine/examples/radivine-daemon.desktop"
install -m 0644 "${REPO_ROOT}/packaging/radivine.service" "${STAGE_DIR}/usr/share/doc/radivine/examples/radivine.service"
install -m 0644 "${REPO_ROOT}/packaging/radivine-worker.service" "${STAGE_DIR}/usr/share/doc/radivine/examples/radivine-worker.service"
if [ -f "${REPO_ROOT}/README.md" ]; then
    install -m 0644 "${REPO_ROOT}/README.md" "${STAGE_DIR}/usr/share/doc/radivine/README.md"
fi
if [ -f "${REPO_ROOT}/THIRD_PARTY_NOTICES.md" ]; then
    install -m 0644 "${REPO_ROOT}/THIRD_PARTY_NOTICES.md" "${STAGE_DIR}/usr/share/doc/radivine/THIRD_PARTY_NOTICES.md"
fi

cat <<'MAN1' | gzip -9nc > "${STAGE_DIR}/usr/share/man/man1/radivine.1.gz"
.TH RADIVINE 1 "October 2026" "Radivine 0.2.0" "User Commands"
.SH NAME
radivine \- context-aware radial action wheel daemon and CLI client
.SH SYNOPSIS
.B radivine
[\fB\-\-config\fR \fIPATH\fR] [\fB\-\-socket\fR \fIPATH\fR] \fICOMMAND\fR [\fIARGS\fR...]
.SH DESCRIPTION
Radivine ("Branch into action.") is a context-aware hierarchical radial action
wheel for X11 desktops. Press Shift during a file drag in Nemo or run
\fBradivine invoke\fR to summon the radial wheel at the pointer.
.SH COMMANDS
.TP
\fBdaemon\fR
Start the background Radivine daemon.
.TP
\fBinvoke\fR [\fB\-\-at\fR \fIX,Y\fR] [\fIFILES\fR...]
Summon the radial wheel in Invoke mode for the specified files or URIs.
.TP
\fBstatus\fR [\fB\-\-json\fR]
Query the running daemon status and telemetry.
.TP
\fBreload\fR
Reload \fB~/.config/radivine/config.toml\fR, \fB~/.config/radivine/actions.toml\fR,
and \fB~/.config/radivine/convert.toml\fR.
.TP
\fBquit\fR
Gracefully stop the running daemon.
.TP
\fBdoctor\fR [\fB\-\-json\fR]
Inspect X11 extensions, compositor status, monitors, and desktop environment.
MAN1
chmod 0644 "${STAGE_DIR}/usr/share/man/man1/radivine.1.gz"

cat <<'MAN2' | gzip -9nc > "${STAGE_DIR}/usr/share/man/man1/radivine-viewer.1.gz"
.TH RADIVINE-VIEWER 1 "October 2026" "Radivine 0.2.0" "User Commands"
.SH NAME
radivine-viewer \- read-only file preview window for Radivine
.SH SYNOPSIS
.B radivine-viewer
\fB\-\-mode\fR \fIfull|head|tail\fR [\fB\-\-lines\fR \fIN\fR] [\fB\-\-theme\fR \fIdark|light|auto\fR] \fB\-\-\fR \fIPATH\fR
.SH DESCRIPTION
Opens a fast, read-only desktop preview window for text files in Full, Head,
or Tail mode with incremental search and multi-gigabyte file support.
MAN2
chmod 0644 "${STAGE_DIR}/usr/share/man/man1/radivine-viewer.1.gz"

cat <<'MAN3' | gzip -9nc > "${STAGE_DIR}/usr/share/man/man1/radivine-control.1.gz"
.TH RADIVINE-CONTROL 1 "October 2026" "Radivine 0.2.0" "User Commands"
.SH NAME
radivine-control \- graphical Control Center and Advanced Convert window for Radivine
.SH SYNOPSIS
.B radivine-control
[\fB\-\-page\fR \fIdashboard|general|wheel|actions|conversion|jobs|diagnostics|about\fR] [\fB\-\-config\fR \fIPATH\fR] [\fB\-\-socket\fR \fIPATH\fR]
.br
.B radivine-control convert
[\fB\-\-preset\fR \fIID\fR] [\fB\-\-target\fR \fIpng|jpeg|webp\fR] \fB\-\-\fR \fIPATH\fR...
.SH DESCRIPTION
Opens the native Radivine Control Center desktop application to manage the
background Radivine daemon, login autostart, radial wheel geometry, shell-free
custom actions, conversion settings, background jobs, and diagnostics, or
launches the standalone Advanced Convert window.
MAN3
chmod 0644 "${STAGE_DIR}/usr/share/man/man1/radivine-control.1.gz"

cat <<'MAN4' | gzip -9nc > "${STAGE_DIR}/usr/share/man/man1/radivine-worker.1.gz"
.TH RADIVINE-WORKER 1 "October 2026" "Radivine 0.2.0" "User Commands"
.SH NAME
radivine-worker \- on-demand background conversion and job worker for Radivine
.SH SYNOPSIS
.B radivine-worker
[\fB\-\-socket\fR \fIPATH\fR] [\fB\-\-db\fR \fIPATH\fR] [\fB\-\-lock\fR \fIPATH\fR] \fBserve\fR [\fB\-\-idle\-timeout\-ms\fR \fIMS\fR]
.SH DESCRIPTION
Executes durable background conversion jobs submitted by \fBradivine\fR or
\fBradivine-control\fR, persisting job history in \fB~/.local/state/radivine/jobs.sqlite3\fR
with startup crash recovery, atomic no-overwrite file commits, and automatic
idle exit after 60 seconds of inactivity.
MAN4
chmod 0644 "${STAGE_DIR}/usr/share/man/man1/radivine-worker.1.gz"

INSTALLED_SIZE="$(du -sk "${STAGE_DIR}/usr" | awk '{print $1}')"
sed "s/__INSTALLED_SIZE__/${INSTALLED_SIZE}/g" \
    "${REPO_ROOT}/packaging/debian/control" > "${STAGE_DIR}/DEBIAN/control"
chmod 0644 "${STAGE_DIR}/DEBIAN/control"

install -m 0755 "${REPO_ROOT}/packaging/debian/prerm" "${STAGE_DIR}/DEBIAN/prerm"
install -m 0755 "${REPO_ROOT}/packaging/debian/postrm" "${STAGE_DIR}/DEBIAN/postrm"

find "$STAGE_DIR" -type d -exec chmod 0755 {} +

echo "==> Building ${DEB_PATH}..."
dpkg-deb --root-owner-group --build "$STAGE_DIR" "$DEB_PATH"

(
    cd "$OUT_DIR"
    sha256sum "${DEB_BASENAME}.deb" > SHA256SUMS.txt
)

echo "==> Package metadata (dpkg-deb -I):"
dpkg-deb -I "$DEB_PATH"
echo "==> Package contents (dpkg-deb -c):"
dpkg-deb -c "$DEB_PATH"
echo "==> SHA-256 checksum:"
cat "${OUT_DIR}/SHA256SUMS.txt"
