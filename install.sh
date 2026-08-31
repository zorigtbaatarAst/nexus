#!/bin/sh
#
# BugHunter installer.
#
#   curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/bughunter/main/install.sh | sh
#
# Downloads the release binary for this machine, verifies its checksum, and
# installs it. Nothing is installed if the checksum does not match. With no
# matching release it falls back to building from source, if cargo is present.
#
# Environment:
#   BUGHUNTER_VERSION       version to install (default: the latest release)
#   BUGHUNTER_INSTALL_DIR   where to install (default: see "Choosing a directory")
#
# Flags:
#   --uninstall             remove an installed bughunter
#   --version <v>           same as BUGHUNTER_VERSION
#   --dir <path>            same as BUGHUNTER_INSTALL_DIR
#   --from-source           skip the download and build with cargo
#
# Choosing a directory, in order:
#   1. BUGHUNTER_INSTALL_DIR, if set
#   2. /usr/local/bin, when running as root or when it is writable
#   3. ~/.local/bin
#
# The script never invokes sudo on its own. Escalating privileges inside a
# piped shell script is a surprise the reader cannot see coming, so when
# /usr/local/bin is not writable it installs to your home directory and tells
# you how to install system-wide instead.

set -eu

REPO="zorigtbaatarAst/bughunter"
BINARY="bughunter"

VERSION="${BUGHUNTER_VERSION:-}"
INSTALL_DIR="${BUGHUNTER_INSTALL_DIR:-}"
UNINSTALL=0
FROM_SOURCE=0

# ---------------------------------------------------------------- output ----

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
	C_BOLD=$(printf '\033[1m')
	C_DIM=$(printf '\033[2m')
	C_RED=$(printf '\033[31m')
	C_GREEN=$(printf '\033[32m')
	C_YELLOW=$(printf '\033[33m')
	C_OFF=$(printf '\033[0m')
else
	C_BOLD='' C_DIM='' C_RED='' C_GREEN='' C_YELLOW='' C_OFF=''
fi

say()  { printf '%s\n' "$*" >&2; }
step() { printf '%s==>%s %s\n' "$C_BOLD" "$C_OFF" "$*" >&2; }
warn() { printf '%s!%s %s\n' "$C_YELLOW" "$C_OFF" "$*" >&2; }
dim()  { printf '%s%s%s\n' "$C_DIM" "$*" "$C_OFF" >&2; }
ok()   { printf '%s✓%s %s\n' "$C_GREEN" "$C_OFF" "$*" >&2; }

die() {
	printf '%serror:%s %s\n' "$C_RED" "$C_OFF" "$*" >&2
	exit 1
}

# ------------------------------------------------------------------ args ----

while [ $# -gt 0 ]; do
	case "$1" in
	--uninstall) UNINSTALL=1 ;;
	--from-source) FROM_SOURCE=1 ;;
	--version)
		[ $# -ge 2 ] || die "--version needs a value, e.g. --version v0.1.0"
		VERSION="$2"
		shift
		;;
	--version=*) VERSION="${1#--version=}" ;;
	--dir)
		[ $# -ge 2 ] || die "--dir needs a value"
		INSTALL_DIR="$2"
		shift
		;;
	--dir=*) INSTALL_DIR="${1#--dir=}" ;;
	-h | --help)
		sed -n '2,32p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown option: $1" ;;
	esac
	shift
done

have() { command -v "$1" >/dev/null 2>&1; }

# ------------------------------------------------------------- uninstall ----

if [ "$UNINSTALL" -eq 1 ]; then
	found=0
	for dir in "$INSTALL_DIR" /usr/local/bin "$HOME/.local/bin" "$HOME/bin"; do
		[ -n "$dir" ] || continue
		target="$dir/$BINARY"
		[ -f "$target" ] || continue
		found=1
		if rm -f "$target" 2>/dev/null; then
			ok "Removed $target"
		else
			warn "Cannot remove $target (needs root). Run:"
			dim "  sudo rm $target"
		fi
	done
	[ "$found" -eq 1 ] || say "No bughunter installation found."
	say ""
	dim "Project data lives in each repository's .bughunter/ directory and was left alone."
	exit 0
fi

# -------------------------------------------------------------- platform ----

detect_platform() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$os" in
	Linux) os="linux" ;;
	Darwin) os="macos" ;;
	*) return 1 ;;
	esac
	case "$arch" in
	x86_64 | amd64) arch="x86_64" ;;
	aarch64 | arm64) arch="aarch64" ;;
	*) return 1 ;;
	esac
	printf '%s-%s' "$BINARY" "$os-$arch"
}

fetch() {
	if have curl; then curl -fsSL "$1"
	elif have wget; then wget -qO- "$1"
	else die "neither curl nor wget is installed; one is needed to download BugHunter."
	fi
}

fetch_to() {
	if have curl; then curl -fsSL -o "$2" "$1"
	elif have wget; then wget -qO "$2" "$1"
	else die "neither curl nor wget is installed; one is needed to download BugHunter."
	fi
}

# ------------------------------------------------------------- directory ----

choose_dir() {
	if [ -n "$INSTALL_DIR" ]; then
		printf '%s' "$INSTALL_DIR"
		return
	fi
	if [ "$(id -u)" = "0" ] || [ -w /usr/local/bin ] 2>/dev/null; then
		printf '/usr/local/bin'
		return
	fi
	printf '%s/.local/bin' "$HOME"
}

install_binary() {
	# $1 = path to the built or downloaded binary
	dir=$(choose_dir)
	mkdir -p "$dir" || die "cannot create $dir"
	chmod +x "$1"
	if ! mv -f "$1" "$dir/$BINARY" 2>/dev/null; then
		die "cannot write to $dir.
Install somewhere else, or as root:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --dir \$HOME/.local/bin
  sudo sh -c 'curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh'"
	fi
	ok "Installed $dir/$BINARY"

	case ":$PATH:" in
	*":$dir:"*) ;;
	*)
		warn "$dir is not on your PATH. Add it:"
		dim "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.bashrc   # or ~/.zshrc"
		dim "  export PATH=\"$dir:\$PATH\""
		;;
	esac
}

# ----------------------------------------------------------- from source ----

build_from_source() {
	have cargo || die "no prebuilt binary for this machine, and cargo is not installed.
Install Rust first:
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"

	step "Building from source (this takes a few minutes the first time)"
	TMP=$(mktemp -d 2>/dev/null || mktemp -d -t bughunter)
	trap 'rm -rf "$TMP"' EXIT INT TERM

	have git || die "git is needed to build from source."
	git clone --depth 1 "https://github.com/$REPO.git" "$TMP/src" >/dev/null 2>&1 ||
		die "could not clone https://github.com/$REPO.git"

	( cd "$TMP/src" && cargo build --release --locked -q ) ||
		die "the build failed. A C compiler is needed for the bundled SQLite:
  Debian/Ubuntu:  sudo apt install build-essential
  Fedora:         sudo dnf install gcc
  macOS:          xcode-select --install"

	install_binary "$TMP/src/target/release/$BINARY"
}

if [ "$FROM_SOURCE" -eq 1 ]; then
	build_from_source
	say ""
	"$(choose_dir)/$BINARY" --version 2>/dev/null || true
	exit 0
fi

# --------------------------------------------------------------- version ----

# Follows the /releases/latest redirect rather than calling the API, because the
# API rate-limits unauthenticated requests to 60 an hour per IP and an installer
# must not fail for a reason the user cannot act on.
resolve_latest() {
	url=""
	if have curl; then
		url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
			"https://github.com/$REPO/releases/latest" 2>/dev/null || true)
	elif have wget; then
		url=$(wget -qS --max-redirect=10 -O /dev/null \
			"https://github.com/$REPO/releases/latest" 2>&1 |
			awk '/^  Location: /{print $2}' | tail -1 || true)
	fi
	case "$url" in
	*/releases/tag/*) printf '%s' "${url##*/tag/}" ;;
	*) return 1 ;;
	esac
}

ASSET=$(detect_platform) || {
	warn "No prebuilt binary for $(uname -s) $(uname -m)."
	build_from_source
	exit 0
}

if [ -z "$VERSION" ]; then
	step "Resolving the latest release"
	if ! VERSION=$(resolve_latest); then
		warn "No published release found yet."
		build_from_source
		say ""
		dim "Next: cd into a project and run  bughunter scan"
		exit 0
	fi
fi

case "$VERSION" in
v*) ;;
*) VERSION="v$VERSION" ;;
esac

BASE="https://github.com/$REPO/releases/download/$VERSION"
TMP=$(mktemp -d 2>/dev/null || mktemp -d -t bughunter)
trap 'rm -rf "$TMP"' EXIT INT TERM

step "Downloading $BINARY $VERSION ($ASSET)"
if ! fetch_to "$BASE/$ASSET" "$TMP/$BINARY" 2>/dev/null; then
	warn "No asset $ASSET in release $VERSION."
	build_from_source
	exit 0
fi

# -------------------------------------------------------------- checksum ----

# A download that cannot be verified is not installed. Silently accepting one
# would defeat the point of publishing checksums at all.
step "Verifying the checksum"
if ! fetch "$BASE/checksums.txt" >"$TMP/checksums.txt" 2>/dev/null; then
	die "release $VERSION publishes no checksums.txt; refusing to install an unverified binary.
Build from source instead:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --from-source"
fi

expected=$(awk -v a="$ASSET" '$2 == a || $2 == "*"a {print $1}' "$TMP/checksums.txt" | head -1)
[ -n "$expected" ] || die "checksums.txt has no entry for $ASSET."

if have sha256sum; then
	actual=$(sha256sum "$TMP/$BINARY" | awk '{print $1}')
elif have shasum; then
	actual=$(shasum -a 256 "$TMP/$BINARY" | awk '{print $1}')
else
	die "neither sha256sum nor shasum is available; cannot verify the download."
fi

[ "$actual" = "$expected" ] || die "checksum mismatch for $ASSET.
  expected $expected
  actual   $actual
Nothing was installed."
ok "Checksum verified"

install_binary "$TMP/$BINARY"

# ----------------------------------------------------------------- done ----

say ""
dim "Next:"
dim "  cd /path/to/your/project"
dim "  bughunter scan       # indexes the project and sets a baseline"
dim "  bughunter rescan     # what changed, and what it touches"
dim "  bughunter doctor     # if anything looks wrong"
