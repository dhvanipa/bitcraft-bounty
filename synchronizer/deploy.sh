#!/usr/bin/env bash
# Deploys the patched synchronizer and the archiver to a box from this machine:
#
#   RPC_URL_FILE=~/sepolia-rpc synchronizer/deploy.sh ubuntu@HOST KEY.pem /path/to/digital-objects-network
#
# Copies the game's source at COMMIT (default 616e0eb, v0.1.0-rc.50, what the
# hosted synchronizer runs) and this folder to the box, then runs install.sh
# there in the background, since the first build takes a long time. Follow it
# with: ssh -i KEY.pem ubuntu@HOST tail -f sync-build/install.log
#
# The services need a Sepolia RPC. RPC URLs usually carry an API token, so the
# URL is piped to the box's root-only env files and never printed:
#   RPC_URL_FILE  a local file holding the URL;
#   RPC_FROM      or another box of yours, reached with the same key, whose
#                 root-only env file RPC_FROM_ENV (default
#                 /etc/bitcraft-relayer.env) has a line RPC_URL=...; nothing
#                 else in that file is read.
# Without either the services keep the URL they have, or use the public node,
# which rate-limits a catch-up.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
host="${1:?usage: synchronizer/deploy.sh ubuntu@HOST KEY.pem /path/to/digital-objects-network}"
key="${2:?key}"
game="${3:?game source}"
commit="${COMMIT:-616e0eb}"
ssh=(ssh -i "$key" -o BatchMode=yes "$host")

"${ssh[@]}" 'rm -rf ~/sync-build/src ~/sync-build/deploy && mkdir -p ~/sync-build/deploy'
# run on the box: saves the URL from stdin, readable only by its owner, and
# prints only its size
save_rpc='umask 077 && cat > ~/sync-build/rpc_url && echo "RPC URL copied: $(wc -c < ~/sync-build/rpc_url) bytes"'
if [ -n "${RPC_URL_FILE:-}" ]; then
  tr -d '[:space:]' < "$RPC_URL_FILE" | "${ssh[@]}" "$save_rpc"
elif [ -n "${RPC_FROM:-}" ]; then
  ssh -i "$key" -o BatchMode=yes "$RPC_FROM" "sudo sed -n 's/^RPC_URL=//p' ${RPC_FROM_ENV:-/etc/bitcraft-relayer.env}" |
    "${ssh[@]}" "$save_rpc"
fi
git -C "$game" archive --format=tar --prefix=src/ "$commit" | "${ssh[@]}" 'tar x -C ~/sync-build'
scp -q -i "$key" -r "$here/install.sh" "$here/install-poster.sh" "$here/install-proxy.sh" "$here/post-root.sh" "$here/patches" "$host:sync-build/deploy/"
"${ssh[@]}" 'nohup bash ~/sync-build/deploy/install.sh > ~/sync-build/install.log 2>&1 < /dev/null &'
echo "installing on $host; follow it with: ssh -i $key $host tail -f sync-build/install.log"
