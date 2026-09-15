#!/bin/sh
# koboterm launcher for Kobo. Installed at /mnt/onboard/.adds/koboterm/koboterm.sh
# and started from a NickelMenu entry (cmd_spawn).
#
# Nickel (the Kobo UI) must not draw or take taps while koboterm runs.
#   pause   (default) the app freezes the UI stack with SIGSTOP once the screen
#           is stable and thaws it on quit. Instant resume, state untouched.
#   restart KOReader style: this script kills the UI stack and relaunches it
#           the way /etc/init.d/rcS does. Slower (~10 s) but a clean slate.
# Set KOBOTERM_NICKEL=restart in the environment (or edit below) to switch.
# Wi-Fi is left alone either way: wpa_supplicant, dhcpcd and the MediaTek wmt
# daemon are separate processes and keep the connection up without Nickel.
DIR=/mnt/onboard/.adds/koboterm
LOG=$DIR/log.txt
MODE="${KOBOTERM_NICKEL:-pause}"
cd "$DIR" || exit 1
exec >> "$LOG" 2>&1
echo "=== koboterm start $(date) mode=$MODE"

NPID="$(pidof -s nickel)"
if [ "$MODE" = restart ] && [ -n "$NPID" ]; then
  # shellcheck disable=SC2046
  export $(tr '\0' '\n' < "/proc/$NPID/environ" | grep -E '^(DBUS_SESSION_BUS_ADDRESS|NICKEL_HOME|WIFI_MODULE|LANG|INTERFACE|PLATFORM|PRODUCT|LD_LIBRARY_PATH|PATH|QT_GSTREAMER_PLAYBIN_AUDIOSINK|QT_GSTREAMER_PLAYBIN_AUDIOSINK_DEVICE_PARAMETER)=')
  sync
  killall -q -TERM nickel hindenburg sickel fickel strickel fontickel adobehost foxitpdf iink fmon
  i=0
  while pkill -0 nickel 2>/dev/null; do
    i=$((i + 1)); [ "$i" -ge 15 ] && break
    usleep 250000
  done
  rm -f /tmp/nickel-hardware-status
  echo "nickel stopped"
  ./koboterm app --nickel leave
  echo "koboterm exited with $?"
  rm -f /tmp/nickel-hardware-status
  mkfifo /tmp/nickel-hardware-status
  rm -f /mnt/onboard/.kobo/sickel_frozen
  sync
  cd /
  /usr/local/Kobo/hindenburg &
  LIBC_FATAL_STDERR_=1 /usr/local/Kobo/nickel -platform kobo -skipFontLoad &
  [ "$PLATFORM" != "freescale" ] && udevadm trigger &
  echo "nickel restarted"
else
  ./koboterm app --nickel pause
  echo "koboterm exited with $?"
fi
echo "=== koboterm end $(date)"
