#!/usr/bin/env bash
# campaign.sh : connect loops, download rounds as plugin and without radio. One summary line per block.
B=$(dirname "$0"); H=${HOST0:?the folder usbhostfs_pc serves as host0:}
for i in 1 2 3 4 5; do
  $B/soak.sh 1 kR >/dev/null; echo "connect loop $i: $(grep -c 'state 4 after' $H/netup.log) of 30 connects"
done
echo "plugin downloads: $($B/soak.sh 10 kr | grep -c '4 of 4') of 10 rounds"
echo "no radio downloads: $(PRE=nowlan $B/soak.sh 10 r | grep -c '4 of 4') of 10 rounds"
echo "no radio connect loop: $(PRE=nowlan $B/soak.sh 1 R >/dev/null; grep -c 'state 4 after' $H/netup.log) of 30 connects"
echo campaign done
