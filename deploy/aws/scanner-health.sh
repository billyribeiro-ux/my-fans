#!/bin/sh
set -eu
test -s /scan/freshclam.pid
kill -0 "$(cat /scan/freshclam.pid)"
clamdscan --config-file=/etc/myfans-clamd.conf --ping=1:1 >/dev/null 2>&1
# Application scanning separately enforces the 48-hour signature freshness bound.
