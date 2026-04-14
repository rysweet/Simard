#!/bin/bash
# check_vm.sh — Probe the remote Simard VM via azlin bastion SSH.
#
# Output: KEY=VALUE lines on stdout for parsing by the dashboard API.
# Must be run via `systemd-run --user --pipe` when called from the
# daemon, because azlin's bastion SSH produces empty stdout when
# running as a direct child of a systemd service cgroup.

set -euo pipefail

export PATH="/home/azureuser/.cargo/bin:/home/azureuser/.local/bin:/usr/local/bin:/usr/bin:/bin"
export HOME="/home/azureuser"
export SSH_AUTH_SOCK="/run/user/1000/openssh_agent"

exec azlin connect Simard --resource-group rysweet-linux-vm-pool --no-tmux -- \
  "echo HOSTNAME=\$(hostname) && echo UPTIME=\$(uptime -p) && echo DISK_ROOT=\$(df / --output=pcent | tail -1 | tr -d ' %') && echo DISK_DATA=\$(df /mnt/home-data --output=pcent 2>/dev/null | tail -1 | tr -d ' %' || echo N/A) && echo DISK_TMP=\$(df /mnt/tmp-data --output=pcent 2>/dev/null | tail -1 | tr -d ' %' || echo N/A) && echo SIMARD_PROCS=\$(pgrep -f simard -c 2>/dev/null || echo 0) && echo CARGO_PROCS=\$(pgrep -f cargo -c 2>/dev/null || echo 0) && echo LOAD=\$(cat /proc/loadavg | cut -d' ' -f1-3) && echo MEM_USED=\$(free -m | awk '/Mem/{printf \"%d/%d\", \$3, \$2}')"
