#!/bin/sh
# koboterm launcher for Kobo. Installed at /mnt/onboard/.adds/koboterm/koboterm.sh
# and started from a NickelMenu entry (cmd_spawn). Modeled on KOReader's
# koreader.sh / nickel.sh: stop the Kobo UI stack while koboterm owns the
# screen and the touch panel, then start it again exactly like /etc/init.d/rcS.
#
# Wi-Fi is left alone: wpa_supplicant, dhcpcd and the MediaTek wmt daemon are
# separate processes and keep the connection up without Nickel.
DIR=/mnt/onboard/.adds/koboterm
LOG=$DIR/log.txt
cd "$DIR" || exit 1
exec >> "$LOG" 2>&1
echo "=== koboterm start $(date)"

NPID="$(pidof -s nickel)"
if [ -n "$NPID" ]; then
  # Keep the bits of Nickel's environment that rcS exports, for the restart.
  # shellcheck disable=SC2046
  export $(tr '\0' '\n' < "/proc/$NPID/environ" | grep -E '^(DBUS_SESSION_BUS_ADDRESS|NICKEL_HOME|WIFI_MODULE|LANG|INTERFACE|PLATFORM|PRODUCT|LD_LIBRARY_PATH|PATH|QT_GSTREAMER_PLAYBIN_AUDIOSINK|QT_GSTREAMER_PLAYBIN_AUDIOSINK_DEVICE_PARAMETER)=')
  sync
  killall -q -TERM nickel hindenburg sickel fickel strickel fontickel adobehost foxitpdf iink fmon
  i=0
  while pkill -0 nickel 2>/dev/null; do
    i=$((i + 1)); [ "$i" -ge 15 ] && break
    usleep 250000
  done
  # udev/udhcpc scripts would block on open() of this FIFO with nobody reading it.
  rm -f /tmp/nickel-hardware-status
  echo "nickel stopped"
fi

./koboterm app
echo "koboterm exited with $?"

if [ -n "$NPID" ]; then
  rm -f /tmp/nickel-hardware-status
  mkfifo /tmp/nickel-hardware-status
  rm -f /mnt/onboard/.kobo/sickel_frozen
  sync
  cd /
  /usr/local/Kobo/hindenburg &
  LIBC_FATAL_STDERR_=1 /usr/local/Kobo/nickel -platform kobo -skipFontLoad &
  [ "$PLATFORM" != "freescale" ] && udevadm trigger &
  echo "nickel restarted"
fi
echo "=== koboterm end $(date)"
