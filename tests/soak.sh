#!/usr/bin/env bash
# soak.sh N [flags] : (HOST0=<host0 folder>, a web server on 127.0.0.1:8975 with 10m.bin, its adler32 in $BENCH/www/10m.sum)
# N fresh PSPLink starts (soft reset), netup loads usbnet like an application,
# connects over the USB profile and does 4 downloads of 10 MB with reconnects. One line per round.
R=$(cd "$(dirname "$0")/.." && pwd); B=${BENCH:-$HOME/.cache/pspkit-bench}; H=${HOST0:?the folder usbhostfs_pc serves as host0:}; P=${PSPSH:-pspsh}
want=$(cat $B/www/10m.sum); F=${2:-kr}
pgrep -x pspkit-usbnetd >/dev/null || ( setsid nohup $R/gateway/target/release/pspkit-usbnetd > $B/daemon.log 2>&1 & )
up(){ timeout 6 $P -e ver 2>/dev/null | grep -q PSPLink; }
for n in $(seq $1); do
  up || { echo "round $n: PSPLink does not answer, stopping"; exit 1; }
  if timeout 10 $P -e modlist 2>/dev/null | tr -d '\r' | grep -q 'Name: usbnet$'; then timeout 15 $P -e "modstun @usbnet" >/dev/null 2>&1; sleep 4; fi
  timeout 15 $P -e reset >/dev/null 2>&1; sleep 5
  for i in $(seq 40); do up && break; sleep 1; done
  rm -f $H/netup.log
  [ -n "$PRE" ] && { timeout 15 $P -e "modstun @usbnet" >/dev/null 2>&1; sleep 4; for i in 1 2 3 4 5; do up && break; sleep 2; done; timeout 20 $P -e "ldstart host0:/usbnet.prx $PRE" >/dev/null; sleep 2; }
  timeout 15 $P -e "ldstart host0:/netup.prx 2 Hi-Speed_USB 10.77.0.1 8975 /10m.bin $F" >/dev/null
  for i in $(seq 120); do grep -qs 'netup: finished' $H/netup.log && break; sleep 1; done
  ok=$(grep -c "adler32 $want" $H/netup.log 2>/dev/null)
  [ "$ok" = 4 ] || cp $H/netup.log $B/fail-$n.log 2>/dev/null
  echo "round $n: $ok of 4 downloads right, ms: $(grep 'download ended' $H/netup.log 2>/dev/null | grep -o 'in [0-9]* ms' | awk '{print $2}' | tr '\n' ' ')"
done
