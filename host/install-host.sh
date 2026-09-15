#!/bin/sh
# koboterm host setup. Served by the Kobo itself at http://<kobo>:8080/install.sh
# with this device's public key baked in. Run on the machine you want to reach
# from the Kobo:   curl -fsSL http://<kobo-ip>:8080/install.sh | sh
#
# What it does (all idempotent):
#   1. authorizes the Kobo's SSH key for this user
#   2. makes sure an SSH server is running (macOS: Remote Login)
#   3. installs tmux and, on macOS, a LaunchAgent that starts a "kobo" tmux
#      session at login. That server runs inside your GUI session, so tools
#      that need the macOS Keychain (Claude Code's login) work inside it.
#   4. registers this machine on the Kobo so it shows up on the home screen
set -eu

KOBO_KEY='__KOBO_KEY__'
KOBO_URL='__KOBO_URL__'
SESSION=kobo

say() { printf '\033[1m[koboterm]\033[0m %s\n' "$*"; }

# 1. authorized_keys
mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh"
touch "$HOME/.ssh/authorized_keys" && chmod 600 "$HOME/.ssh/authorized_keys"
if grep -qF "$KOBO_KEY" "$HOME/.ssh/authorized_keys"; then
  say "Kobo key already authorized"
else
  printf '%s\n' "$KOBO_KEY" >> "$HOME/.ssh/authorized_keys"
  say "authorized the Kobo's key for $USER"
fi

OS="$(uname -s)"

# 2. ssh server
if [ "$OS" = Darwin ]; then
  if nc -z -G 2 127.0.0.1 22 >/dev/null 2>&1; then
    say "Remote Login is on"
  else
    say "turning on Remote Login (needs your password)"
    sudo systemsetup -setremotelogin on
  fi
else
  if ! (command -v sshd >/dev/null 2>&1 || [ -x /usr/sbin/sshd ]); then
    say "no sshd found; install openssh-server (e.g. sudo apt install openssh-server)"
  fi
fi

# 3. tmux
if ! command -v tmux >/dev/null 2>&1; then
  if [ "$OS" = Darwin ] && command -v brew >/dev/null 2>&1; then
    say "installing tmux with Homebrew"
    brew install tmux
  elif command -v apt-get >/dev/null 2>&1; then
    say "installing tmux with apt"
    sudo apt-get install -y tmux
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y tmux
  else
    say "please install tmux, then re-run this script"
  fi
fi
TMUX="$(command -v tmux || true)"

if [ "$OS" = Darwin ] && [ -n "$TMUX" ]; then
  PLIST="$HOME/Library/LaunchAgents/co.wust.koboterm.tmux.plist"
  mkdir -p "$HOME/Library/LaunchAgents"
  cat > "$PLIST" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>co.wust.koboterm.tmux</string>
  <key>ProgramArguments</key><array>
    <string>$TMUX</string><string>new-session</string><string>-d</string><string>-s</string><string>$SESSION</string>
  </array>
  <key>RunAtLoad</key><true/>
</dict></plist>
PL
  launchctl bootout "gui/$(id -u)" "$PLIST" >/dev/null 2>&1 || true
  launchctl bootstrap "gui/$(id -u)" "$PLIST" >/dev/null 2>&1 || launchctl load "$PLIST" >/dev/null 2>&1 || true
  say "tmux session '$SESSION' starts at login (LaunchAgent installed)"
elif [ -n "$TMUX" ]; then
  "$TMUX" has-session -t "$SESSION" >/dev/null 2>&1 || "$TMUX" new-session -d -s "$SESSION"
fi

# 4. register with the Kobo
if [ "$OS" = Darwin ]; then
  NAME="$(scutil --get ComputerName 2>/dev/null || hostname)"
  IFACE="$(route -n get "$(printf '%s' "$KOBO_URL" | sed -E 's#https?://([^:/]+).*#\1#')" 2>/dev/null | awk '/interface:/{print $2}')"
  IP="$(ipconfig getifaddr "${IFACE:-en0}" 2>/dev/null || true)"
else
  NAME="$(hostname)"
  IP="$(ip route get "$(printf '%s' "$KOBO_URL" | sed -E 's#https?://([^:/]+).*#\1#')" 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src") print $(i+1)}' | head -1)"
fi
[ -n "$IP" ] || IP="$(hostname)"
# Absolute path: ssh runs commands in a non-login shell whose PATH may lack Homebrew.
CMD=""
# -u: the Kobo is a UTF-8 terminal. mouse on: swipes on the Kobo scroll tmux history.
[ -n "$TMUX" ] && CMD="$TMUX -u new -A -s $SESSION \\; set -g mouse on \\; set -g focus-events on"
urlenc() { printf '%s' "$1" | od -An -tx1 -v | tr ' ' '\n' | grep -v '^$' | awk '{printf "%%%s", $1}'; }
BODY="name=$(urlenc "$NAME")&spec=$(urlenc "$USER@$IP")&cmd=$(urlenc "$CMD")"
if curl -fsS -X POST --data "$BODY" "$KOBO_URL/register" >/dev/null; then
  say "registered '$NAME' ($USER@$IP) on the Kobo. Tap it there to connect."
else
  say "could not reach the Kobo at $KOBO_URL; add $USER@$IP on the Kobo by hand."
fi
if [ -n "$CMD" ]; then
  say "sessions attach to tmux '$SESSION'. Start your tools inside it: tmux attach -t $SESSION"
fi
