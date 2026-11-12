#!/bin/sh
set -eu
umask 077
mkdir -p /scan/database /scan/tmp
# Preloaded signatures avoid repeatedly downloading the entire public database.
if [ ! -e /scan/database/main.cvd ] && [ ! -e /scan/database/main.cld ]; then
  cp -R /opt/signatures/. /scan/database/
fi
scanner=''
updater=''
stop() {
  [ -z "$scanner" ] || kill -TERM "$scanner" 2>/dev/null || true
  [ -z "$updater" ] || kill -TERM "$updater" 2>/dev/null || true
  [ -z "$scanner" ] || wait "$scanner" 2>/dev/null || true
  [ -z "$updater" ] || wait "$updater" 2>/dev/null || true
}
trap 'stop; exit 0' TERM INT
# Finish the initial update before clamd reads signatures. Starting both at once
# can miss NotifyClamd while the socket is not yet ready and serve stale data.
freshclam --config-file=/etc/myfans-freshclam.conf &
updater=$!
wait "$updater"
updater=''
clamd --config-file=/etc/myfans-clamd.conf &
scanner=$!
freshclam --config-file=/etc/myfans-freshclam.conf --daemon &
updater=$!
while kill -0 "$scanner" 2>/dev/null && kill -0 "$updater" 2>/dev/null; do
  sleep 2 &
  wait "$!" || true
done
stop
exit 1
