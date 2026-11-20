#!/bin/sh
# Runs two instances of mdgitjournal on this machine, as "Device A" and "Device B", each with its
# own journal, settings and sign-in, to try syncing between devices with one computer.
#
#   scripts/two-devices.sh [--dev] [--reset]
#
# Folders, kept between runs (so sign-ins and journals survive restarts):
#   ~/mdgitjournal-test-devices/a/{journal,config,app.log}
#   ~/mdgitjournal-test-devices/b/{journal,config,app.log}
#
# MDGJ_JOURNAL_DIR and MDGJ_CONFIG_DIR are always set, so neither instance touches the real
# journal, settings or keychain (with MDGJ_CONFIG_DIR the sign-in is a file in that folder).
# Sign in on each device (Settings > Connect GitHub; a personal access token works) and connect
# both to the same test repository.
#
# Builds nothing. The binary is $MDGJ_BIN if set, else the release build:
#   macOS: target/release/bundle/macos/mdgitjournal.app/Contents/MacOS/mdgitjournal
#          (run directly, so each instance gets its own environment and a second one starts)
#   Linux: target/release/mdgitjournal
# --dev uses target/debug/mdgitjournal (`cargo build -p mdgitjournal`, or `pnpm -C app tauri dev`),
# which loads the UI from the dev server on localhost:1420. target/ is $CARGO_TARGET_DIR if set.
#
# Stays in the foreground: Ctrl-C (or closing the terminal, or both windows) ends both instances.

set -eu

usage() {
	cat <<'EOF'
Usage: scripts/two-devices.sh [--dev] [--reset]

Runs mdgitjournal twice on this machine, as "Device A" and "Device B", each with its own
journal, settings and sign-in under ~/mdgitjournal-test-devices/{a,b}. Never touches the
real journal, settings or keychain.

  --dev     use the debug build (target/debug/mdgitjournal), which loads the UI from
            `pnpm -C app dev` on localhost:1420
  --reset   delete ~/mdgitjournal-test-devices first (both test devices' journals,
            settings and sign-ins)
  -h, --help
            show this help

MDGJ_BIN overrides the binary. Ctrl-C, or closing both windows, ends both instances.
EOF
}

die() {
	printf 'two-devices: %s\n' "$*" >&2
	exit 1
}

dev=0
reset=0
for arg in "$@"; do
	case $arg in
	--dev) dev=1 ;;
	--reset) reset=1 ;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		exit 2
		;;
	esac
done

[ -n "${HOME:-}" ] || die "HOME is not set"
root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
devices=$HOME/mdgitjournal-test-devices

case $(uname -s) in
Darwin) os=macos ;;
Linux) os=linux ;;
*) die "only macOS and Linux are supported" ;;
esac

if [ -n "${MDGJ_BIN:-}" ]; then
	bin=$MDGJ_BIN
elif [ "$dev" = 1 ]; then
	bin=$target/debug/mdgitjournal
elif [ "$os" = macos ]; then
	bin=$target/release/bundle/macos/mdgitjournal.app/Contents/MacOS/mdgitjournal
else
	bin=$target/release/mdgitjournal
fi

if [ ! -x "$bin" ]; then
	printf 'two-devices: no mdgitjournal at %s\n' "$bin" >&2
	if [ -n "${MDGJ_BIN:-}" ]; then
		echo "Point MDGJ_BIN at an mdgitjournal binary." >&2
	elif [ "$dev" = 1 ]; then
		echo "Build it with: cargo build -p mdgitjournal   (and run pnpm -C app dev)" >&2
	elif [ "$os" = macos ]; then
		echo "Build it with: pnpm -C app tauri build --bundles app" >&2
	else
		echo "Build it with: pnpm -C app tauri build --no-bundle" >&2
	fi
	if [ "$dev" = 0 ] && [ -z "${MDGJ_BIN:-}" ] && [ -x "$target/debug/mdgitjournal" ]; then
		echo "Or use the debug build with --dev (needs pnpm -C app dev running)." >&2
	fi
	exit 1
fi

if [ "$dev" = 1 ] && [ -z "${MDGJ_BIN:-}" ]; then
	if command -v curl >/dev/null 2>&1; then
		curl -fsS -o /dev/null --max-time 3 http://localhost:1420/ ||
			die "nothing answers on localhost:1420; run pnpm -C app dev first"
	else
		echo "two-devices: curl not found; not checking that pnpm -C app dev is running" >&2
	fi
fi

if [ "$reset" = 1 ]; then
	echo "Deleting $devices (both test devices' journals, settings and sign-ins)."
	rm -rf -- "$devices"
fi

pids=
# shellcheck disable=SC2317 # only reached through the traps below
cleanup() {
	trap - HUP INT TERM EXIT
	# shellcheck disable=SC2086 # word splitting is wanted: a list of pids
	[ -z "$pids" ] || kill $pids 2>/dev/null || true
}
trap cleanup EXIT
trap 'cleanup; exit 129' HUP
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# start <letter> <name>
start() {
	dir=$devices/$1
	mkdir -p "$dir/journal" "$dir/config"
	printf '%s: journal %s\n          config  %s\n          log     %s\n' \
		"$2" "$dir/journal" "$dir/config" "$dir/app.log"
	MDGJ_JOURNAL_DIR=$dir/journal MDGJ_CONFIG_DIR=$dir/config MDGJ_DEVICE_NAME=$2 \
		"$bin" >>"$dir/app.log" 2>&1 &
	pids="$pids $!"
}

echo "Binary: $bin"
start a "Device A"
start b "Device B"
echo "Running. Ctrl-C (or closing both windows) ends both."

status=0
for pid in $pids; do
	wait "$pid" || status=$?
done
pids=
exit "$status"
