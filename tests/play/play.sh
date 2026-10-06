#!/usr/bin/env bash
# play.sh <iso on ms0:> <timetable> <seconds>: starts the game from PSPLink with pad.prx pressing the
# timetable's buttons, lets it run, power-cycles back to PSPLink and fetches the pictures into
# $BENCH/play/<time>/ (sheet.png shows them all). The gateway's log of that time is put beside them.
# Needs HOST0, PSPSH, RELAY as night.sh, and python with numpy and Pillow as PYTHON.
R=$(cd "$(dirname "$0")/../.." && pwd); B=${BENCH:-$HOME/.cache/pspkit-bench}; H=${HOST0:?}; P=${PSPSH:-pspsh}
iso=$1 table=$2 secs=${3:-120}; out=$B/play/$(date +%H%M%S); mkdir -p $out
up(){ timeout 6 $P -e ver 2>/dev/null | grep -q PSPLink; }
wait_up(){ for i in $(seq 60); do up && return 0; sleep 2; done; return 1; }
sh(){ timeout 30 $P -e "$1" >/dev/null 2>&1; }
pkill -x usbhostfs_pc; sleep 1; ( cd $H && setsid nohup ${HOSTFS:-usbhostfs_pc} > /dev/null 2>&1 < /dev/null & ); sleep 2
pgrep -x pspkit-usbnetd >/dev/null || ( setsid nohup $R/gateway/target/release/pspkit-usbnetd -v >> $B/daemon.log 2>&1 < /dev/null & )
up || { $RELAY off >/dev/null 2>&1; sleep 15; $RELAY on >/dev/null 2>&1; wait_up; } || { echo "no PSPLink"; exit 1; }
cp $R/tests/play/launch.prx $R/tests/play/pad.prx $H/; cp "$table" $H/pad.txt; echo "$iso" > $H/launch.txt
printf 'always, ms0:/seplugins/usbnet.prx, on\numd, ms0:/seplugins/pad.prx, on\n' > $H/PLUGINS.TXT
sh "cp host0:/pad.prx ms0:/seplugins/pad.prx"; sh "cp host0:/pad.txt ms0:/seplugins/pad.txt"; sh "cp host0:/PLUGINS.TXT ms0:/seplugins/PLUGINS.TXT"
for f in $(timeout 20 $P -e "ls ms0:/shots" 2>/dev/null | tr -d '\r' | grep -o '[0-9]*\.raw'); do sh "rm ms0:/shots/$f"; done
mark=$(wc -l < $B/daemon.log)
timeout 15 $P -e "ldstart host0:/launch.prx" >/dev/null 2>&1
sleep $secs
tail -n +$((mark + 1)) $B/daemon.log > $out/gateway.log
$RELAY off >/dev/null 2>&1; sleep 15; $RELAY on >/dev/null 2>&1; wait_up || { echo "no PSPLink after the run"; exit 1; }
printf 'always, ms0:/seplugins/usbnet.prx, on\n' > $H/PLUGINS.TXT; sh "cp host0:/PLUGINS.TXT ms0:/seplugins/PLUGINS.TXT"
mkdir -p $H/shots; rm -f $H/shots/*
for f in $(timeout 20 $P -e "ls ms0:/shots" 2>/dev/null | tr -d '\r' | grep -o '[0-9]*\.raw'); do sh "cp ms0:/shots/$f host0:/shots/$f"; done
mv $H/shots/*.raw $out/ 2>/dev/null
${PYTHON:-python3} $R/tests/play/shots.py $out
echo "$out"; grep -c "" $out/gateway.log
