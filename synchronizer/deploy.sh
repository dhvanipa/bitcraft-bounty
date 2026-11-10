#!/usr/bin/env bash
# Deploys the patched synchronizer and the archiver to a box from this Mac:
#
#   RPC_FROM=ubuntu@GAME-SERVER synchronizer/deploy.sh ubuntu@HOST KEY.pem /path/to/digital-objects-network
#
# Copies the game's source at COMMIT (default 616e0eb, v0.1.0-rc.50, what the
# hosted synchronizer runs) and this folder to the box, then runs install.sh
# there in the background, since the first build takes a long time. Follow it
# with: ssh -i KEY.pem ubuntu@HOST tail -f sync-build/install.log
#
# RPC_FROM (optional): a box running bitcraft-agent's relayer, reached with the
# same key. Its Sepolia RPC_URL, which may carry an API token, is copied from
# its root-only /etc/bitcraft-relayer.env straight to the new box, never
# printed; nothing else in that file is read. Without it the services keep the
# URL they have, or use the public node.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
host="${1:?usage: synchronizer/deploy.sh ubuntu@HOST KEY.pem /path/to/digital-objects-network}"
key="${2:?key}"
game="${3:?game source}"
commit="${COMMIT:-616e0eb}"
ssh=(ssh -i "$key" -o BatchMode=yes "$host")

"${ssh[@]}" 'rm -rf ~/sync-build/src ~/sync-build/deploy && mkdir -p ~/sync-build/deploy'
if [ -n "${RPC_FROM:-}" ]; then
  ssh -i "$key" -o BatchMode=yes "$RPC_FROM" "sudo sed -n 's/^RPC_URL=//p' /etc/bitcraft-relayer.env" |
    "${ssh[@]}" 'umask 077 && cat > ~/sync-build/rpc_url && echo "RPC URL copied: $(wc -c < ~/sync-build/rpc_url) bytes"'
fi
git -C "$game" archive --format=tar --prefix=src/ "$commit" | "${ssh[@]}" 'tar x -C ~/sync-build'
scp -q -i "$key" -r "$here/install.sh" "$here/install-poster.sh" "$here/install-proxy.sh" "$here/post-root.sh" "$here/patches" "$host:sync-build/deploy/"
"${ssh[@]}" 'nohup bash ~/sync-build/deploy/install.sh > ~/sync-build/install.log 2>&1 < /dev/null &'
echo "installing on $host; follow it with: ssh -i $key $host tail -f sync-build/install.log"
