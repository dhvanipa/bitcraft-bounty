#!/usr/bin/env bash
# Posts the game's state root at Sepolia's finalized block to the bounty
# contract (onchain/src/BountyEscrow.sol). The holdings-root-poster timer
# runs it hourly on the synchronizer's box; install-poster.sh installs both.
#
# The root comes from our own synchronizer, which verifies every transaction's
# proof itself. The finalized block is about 16 minutes behind the head on
# Sepolia, and a root there cannot be reorged out. Before posting, the root is
# checked against a second synchronizer (CHECK_URL, the hosted one by
# default): a disagreement stops the post. If that one is down, ours alone
# decides.
#
# Environment (/etc/holdings-root-poster.env, root only):
#   BOUNTY        the BountyEscrow's address
#   ETH_RPC_URL   a Sepolia RPC
#   SYNC_URL      our synchronizer (default http://127.0.0.1:3000)
#   CHECK_URL     a second synchronizer, or empty for none
#                 (default https://synchronizer.digiobjects.net)
# and the poster's key as a keystore: ETH_KEYSTORE and ETH_PASSWORD (a file),
# which the service sets from its systemd credentials.
set -euo pipefail

: "${BOUNTY:?BOUNTY is not set}" "${ETH_RPC_URL:?ETH_RPC_URL is not set}"
: "${ETH_KEYSTORE:?ETH_KEYSTORE is not set}" "${ETH_PASSWORD:?ETH_PASSWORD is not set}"
export ETH_RPC_URL ETH_KEYSTORE ETH_PASSWORD
sync="${SYNC_URL:-http://127.0.0.1:3000}"
check="${CHECK_URL-https://synchronizer.digiobjects.net}"

winner="$(cast call "$BOUNTY" 'winner()(address)')"
if [ "$winner" != 0x0000000000000000000000000000000000000000 ]; then
  echo "the bounty was claimed by $winner; nothing to post"
  exit 0
fi

block="$(cast block finalized --field number)"
if ! state="$(curl -fsS "$sync/v1/state/by_el/$block")"; then
  echo "our synchronizer has no state at finalized block $block yet; $sync/sync-progress says $(curl -fsS "$sync/sync-progress" || echo nothing)" >&2
  exit 1
fi
root="0x$(jq -r .state_root <<<"$state")"

if [ -n "$check" ]; then
  if theirs="$(curl -fsS --max-time 20 "$check/v1/state/by_el/$block")"; then
    theirs="0x$(jq -r .state_root <<<"$theirs")"
    if [ "$theirs" != "$root" ]; then
      echo "state roots differ at block $block: ours $root, $check's $theirs; not posting" >&2
      exit 1
    fi
  else
    echo "warning: $check did not answer for block $block; posting our root alone" >&2
  fi
fi

if [ "$(cast call "$BOUNTY" 'rootBlock(bytes32)(uint64)' "$root")" != 0 ]; then
  echo "root $root (block $block) is already posted"
  exit 0
fi

receipt="$(cast send --json "$BOUNTY" 'postRoot(bytes32,uint64)' "$root" "$block")"
jq -r '"root '"$root"' (block '"$block"'): \(.transactionHash), status \(.status), gas \(.gasUsed)"' <<<"$receipt"
[ "$(jq -r .status <<<"$receipt")" = 0x1 ]
