#!/usr/bin/env bash
# night.sh <name> <starts> <rounds> <flags> [mode]: one batch of starts x rounds connections on a
# real PSP over PSPLink, with tests/night. One line per start, the batch's total in $BENCH/night/results.txt,
# the logs of every start with a failure in $BENCH/night/fail-<name>-<start>/.
#
# mode: plugin (default: the copy ARK loaded), noplugin (unloaded: the firmware alone, to compare),
# nowlan (as if there were no radio), reload (unloaded and loaded again beside PSPLink), gwrestart (the gateway is ended and started during the rounds),
# pspreset (the PSP is reset during the rounds; three rounds after it are what counts).
# NIGHT_CMD: another program and its arguments instead of night.prx (tests/https).
# What PSPLink prints (an exception's registers) is kept in $BENCH/night/console.log.
#
# Needs: HOST0 (the folder usbhostfs_pc serves; this script starts it, HOSTFS names the program), PSPSH, a web server on 127.0.0.1:8975 with 10m.bin
# and 1k.bin, $BENCH/www/10m.sum, tests/udpecho.py running, RELAY (a command taking on|off) to
# power-cycle a PSP that no longer answers.
R=$(cd "$(dirname "$0")/.." && pwd); B=${BENCH:-$HOME/.cache/pspkit-bench}; H=${HOST0:?}; P=${PSPSH:-pspsh}
name=$1 starts=$2 rounds=$3 flags=$4 mode=${5:-plugin}; sum=$(cat $B/www/10m.sum)
G="$R/gateway/target/release/pspkit-usbnetd"
up(){ timeout 6 $P -e ver 2>/dev/null | grep -q PSPLink; }
wait_up(){ for i in $(seq ${1:-40}); do up && return 0; sleep 2; done; return 1; }
gateway(){ pgrep -x pspkit-usbnetd >/dev/null || ( setsid nohup $G -v >> $B/daemon.log 2>&1 < /dev/null & ); }
console(){ pgrep -f "night/[c]onsole.sh" >/dev/null && return
  printf '#!/usr/bin/env bash\nwhile :; do exec 3<>/dev/tcp/127.0.0.1/$1 && cat <&3; sleep 2; done\n' > $B/night/console.sh; chmod +x $B/night/console.sh
  for p in 10000 10001 10002; do ( setsid nohup $B/night/console.sh $p >> $B/night/console.log 2>/dev/null < /dev/null & ); done; }
power_cycle(){ echo "$name: PSPLink does not answer, power cycle" | tee -a $B/night/results.txt; $RELAY off >/dev/null 2>&1; sleep 8; $RELAY on >/dev/null 2>&1; wait_up 60; }
# usbhostfs_pc runs out of file handles after some thousand opened files: a fresh one for each batch
pkill -x usbhostfs_pc; sleep 1; ( cd $H && setsid nohup ${HOSTFS:-usbhostfs_pc} > $B/night/usbhostfs.log 2>&1 < /dev/null & ); sleep 2
total=0 good=0
for s in $(seq $starts); do
  gateway; console
  up || power_cycle || { echo "$name: no PSPLink after a power cycle, giving up"; exit 1; }
  timeout 15 $P -e reset >/dev/null 2>&1; sleep 5; wait_up || power_cycle
  if [ $mode = nowlan ] || [ $mode = reload ] || [ $mode = noplugin ]; then
    timeout 15 $P -e "modstun @usbnet" >/dev/null 2>&1; sleep 3; wait_up 10
    [ $mode = noplugin ] || timeout 20 $P -e "ldstart host0:/usbnet.prx $([ $mode = nowlan ] && echo nowlan)" >/dev/null 2>&1; sleep 2
  fi
  rm -f $H/night.log; mark=$(wc -l < $B/daemon.log); cmark=$(wc -l < $B/night/console.log 2>/dev/null || echo 0)
  cmd=${NIGHT_CMD:-"host0:/night.prx Hi-Speed_USB $rounds $flags $sum"}; n=$rounds
  timeout 15 $P -e "ldstart $cmd" >/dev/null 2>&1
  for i in $(seq 25); do [ -s $H/night.log ] && break; sleep 1; done
  [ -s $H/night.log ] || [ $mode = pspreset ] || timeout 15 $P -e "ldstart $cmd" >/dev/null 2>&1 # the command got lost
  if [ $mode = gwrestart ]; then sleep 12; kill $(pgrep -x pspkit-usbnetd); sleep 3; gateway; fi
  if [ $mode = pspreset ]; then
    sleep 12; timeout 15 $P -e reset >/dev/null 2>&1; sleep 5; wait_up || power_cycle
    rm -f $H/night.log; n=3; timeout 15 $P -e "ldstart host0:/night.prx Hi-Speed_USB 3 $flags $sum" >/dev/null 2>&1
  fi
  for i in $(seq $((n * 45 + 60))); do grep -qs 'night: finished' $H/night.log && break; sleep 1; done
  ok=$(grep -c 'night: round .* ok' $H/night.log 2>/dev/null); ok=${ok:-0}
  total=$((total + n)); good=$((good + ok))
  echo "$name start $s: $ok of $n"
  if [ "$ok" != "$n" ]; then
    d=$B/night/fail-$name-$s; mkdir -p $d; cp $H/night.log $d/ 2>/dev/null; tail -n +$((mark + 1)) $B/daemon.log > $d/gateway.log
    (timeout 8 $P -e modlist 2>/dev/null || echo "PSPLink does not answer") > $d/modlist.txt
    tail -n +$((cmark + 1)) $B/night/console.log > $d/console.log 2>/dev/null
  fi
done
echo "$(date +%H:%M) $name ($mode, flags $flags): $good of $total" | tee -a $B/night/results.txt
