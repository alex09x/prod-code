#!/bin/sh
set -eu
if [ -x notes/readme.txt ]; then exit 21; fi
if [ -x notes/created.txt ]; then exit 22; fi
if [ -e scripts/obsolete.sh ]; then exit 23; fi
printf 'shadow-script-ok:%s:%s\n' "$(cat notes/readme.txt)" "$(cat notes/created.txt)"
