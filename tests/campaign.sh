#!/usr/bin/env bash
# campaign.sh: 1000 connections in 20 batches of 50 (5 starts x 10 rounds), of every kind tests/night
# knows, and 100 downloads over HTTPS from the internet (tests/https) if asked for. Needs what night.sh needs. The totals end up in $BENCH/night/results.txt.
N=$(dirname "$0")/night.sh
b(){ $N "$1" 5 10 "$2" "${3:-plugin}"; }
b download-1 d;        b connect-1 -;          b tcp-udp-1 tu;       b scan-download sd
b wifi-usb-1 ad;       b noradio-download-1 d nowlan;                b noradio-connect - nowlan
b noradio-tcp-udp tu nowlan;                   b noradio-scan s nowlan
b reload d reload;     b to-stick dw;          b 333mhz dc;          b big-buffers dbP
b gateway-restart d gwrestart;                 b psp-reset d pspreset
b download-2 d;        b connect-2 -;          b tcp-udp-2 tu;       b wifi-usb-2 ad
b noradio-download-2 d nowlan
# and from the internet, if HTTPS_URL names a server with tests/https's file: "<url> <bytes> <adler32>"
if [ -n "$HTTPS_URL" ]; then
  for n in 1 2; do NIGHT_CMD="host0:/httpsnight.prx Hi-Speed_USB 10 $HTTPS_URL" $N https-$n 5 10 -; done
fi
echo "campaign done" >> ${BENCH:-$HOME/.cache/pspkit-bench}/night/results.txt
