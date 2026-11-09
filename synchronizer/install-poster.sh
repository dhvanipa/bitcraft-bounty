#!/usr/bin/env bash
# Installs the hourly root poster (post-root.sh) on the synchronizer's box,
# after install.sh. Run on the box from the folder deploy.sh copied:
#
#   bash ~/sync-build/deploy/install-poster.sh            # 1. make the poster's key
#   bash ~/sync-build/deploy/install-poster.sh 0xBOUNTY   # 2. post to this contract
#
# Step 1 installs cast (Foundry) and jq, and makes the poster's key on the box,
# in an encrypted keystore with a random password, both root only; it prints
# only the address. Send that address a little Sepolia ETH and deploy the
# bounty with it as `poster`. Step 2 records the contract and starts the
# hourly timer, then posts once. Safe to re-run: the key is kept.
#
# The poster's Sepolia RPC is the synchronizer's RPC_URL, copied from its
# root-only env file without being printed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bounty="${1:-}"
foundry_version=v1.7.1
key_dir=/etc/holdings-root-poster
env_file=/etc/holdings-root-poster.env
sync_env=/etc/holdings-synchronizer.env

echo "== cast and jq"
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq jq >/dev/null
if ! /usr/local/bin/cast --version 2>/dev/null | grep -q "${foundry_version#v}"; then
  curl -fsSL https://foundry.paradigm.xyz | bash >/dev/null
  ~/.foundry/bin/foundryup --install "$foundry_version" >/dev/null
  sudo install -m 755 ~/.foundry/bin/cast /usr/local/bin/cast
fi
/usr/local/bin/cast --version | head -1
sudo install -m 755 "$here/post-root.sh" /usr/local/bin/holdings-post-root

echo "== the poster's key"
sudo install -d -m 700 "$key_dir"
if ! sudo test -f "$key_dir/poster"; then
  sudo sh -c "umask 077 && openssl rand -hex 32 > $key_dir/password"
  # the password goes through the environment, not the command line
  sudo bash -c "CAST_PASSWORD=\"\$(cat $key_dir/password)\" /usr/local/bin/cast wallet new $key_dir poster >/dev/null"
  sudo chmod 600 "$key_dir/poster"
fi
address="$(sudo env ETH_KEYSTORE="$key_dir/poster" ETH_PASSWORD="$key_dir/password" /usr/local/bin/cast wallet address)"
echo "poster address: $address"

echo "== environment"
sudo touch "$env_file"
sudo chmod 600 "$env_file"
sudo bash -c "sed -i '/^ETH_RPC_URL=/d' $env_file && sed -n 's/^RPC_URL=/ETH_RPC_URL=/p' $sync_env >> $env_file"
if [ -n "$bounty" ]; then
  sudo sed -i '/^BOUNTY=/d' "$env_file"
  echo "BOUNTY=$bounty" | sudo tee -a "$env_file" >/dev/null
fi

echo "== service and timer"
sudo tee /etc/systemd/system/holdings-root-poster.service >/dev/null <<EOF
[Unit]
Description=Post the game's finalized state root to the bounty contract
After=network-online.target holdings-synchronizer.service
Wants=network-online.target

[Service]
Type=oneshot
DynamicUser=yes
EnvironmentFile=$env_file
LoadCredential=keystore:$key_dir/poster
LoadCredential=password:$key_dir/password
Environment=ETH_KEYSTORE=%d/keystore ETH_PASSWORD=%d/password HOME=/tmp
ExecStart=/usr/local/bin/holdings-post-root
EOF
sudo tee /etc/systemd/system/holdings-root-poster.timer >/dev/null <<EOF
[Unit]
Description=Post the game's state root to the bounty contract hourly

[Timer]
OnCalendar=hourly
RandomizedDelaySec=120
Persistent=true

[Install]
WantedBy=timers.target
EOF
sudo systemctl daemon-reload

if sudo grep -q '^BOUNTY=' "$env_file"; then
  sudo systemctl enable -q --now holdings-root-poster.timer
  echo "posting once now"
  sudo systemctl start holdings-root-poster.service || true
  journalctl -u holdings-root-poster.service -n 5 --no-pager -o cat
  systemctl list-timers holdings-root-poster.timer --no-pager | head -2
else
  echo "no contract yet: fund $address with Sepolia ETH, deploy the bounty with it as poster, then re-run with the contract's address"
fi
