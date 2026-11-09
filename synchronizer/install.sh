#!/usr/bin/env bash
# Builds the game's synchronizer (with patches/ applied) and archiver, and
# runs them as the systemd services `holdings-synchronizer` and
# `holdings-archiver`. Runs on the box (Ubuntu, as a sudo user);
# synchronizer/deploy.sh copies the game's source to ~/sync-build/src and this
# folder to ~/sync-build/deploy, then runs it. Safe to re-run: it rebuilds,
# keeps the data, and restarts both services.
#
# Both start from INIT_START_SLOT 11491968 (2026-11-02 16:33 UTC), the genesis
# slot the hosted synchronizer reports at /info, so the synchronizer derives
# the same state roots. The archiver keeps the network's blobs (those sent to
# 0x4343...43) on disk, beyond the beacon's 18 days, and the synchronizer
# reads its blobs from it. APIs: synchronizer 127.0.0.1:3000, archiver
# 127.0.0.1:3001.
#
# RPC: ~/sync-build/rpc_url, when deploy.sh wrote one (it may hold an API
# token: it is moved into the root-only env files and deleted), else the URL
# already in the env files, else the public node.
set -euo pipefail

build=~/sync-build
here="$(cd "$(dirname "$0")" && pwd)"
sync_data=/var/lib/holdings-synchronizer
archive_data=/var/lib/holdings-archiver
sync_env=/etc/holdings-synchronizer.env
archive_env=/etc/holdings-archiver.env
beacon=https://ethereum-sepolia-beacon-api.publicnode.com
genesis_slot=11491968
to_address=0x4343434343434343434343434343434343434343

echo "== packages, swap and Rust"
if ! swapon --show | grep -q /swapfile; then
  # building pod2, plonky2, RocksDB and alloy needs more than the box's 8 GB
  sudo fallocate -l 8G /swapfile
  sudo chmod 600 /swapfile
  sudo mkswap -q /swapfile
  sudo swapon /swapfile
  echo "/swapfile none swap sw 0 0" | sudo tee -a /etc/fstab >/dev/null
fi
sudo apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  build-essential pkg-config libssl-dev clang libclang-dev cmake postgresql openssl >/dev/null
if [ ! -x ~/.cargo/bin/rustup ]; then
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y -q --profile minimal --default-toolchain none
fi

echo "== patches"
cd "$build/src"
for p in "$here"/patches/*.patch; do
  echo "applying $(basename "$p")"
  patch -p1 < "$p"
done

echo "== build (the first one takes a while on 2 cores)"
# the toolchain comes from the source tree's rust-toolchain.toml
DOBJ_RELEASE_TAG="v0.1.0-rc.50+holdings-witness" CARGO_TARGET_DIR="$build/target" \
  nice ~/.cargo/bin/cargo build --release -p synchronizer -p archiver
sudo install -m 755 "$build/target/release/synchronizer" /usr/local/bin/holdings-synchronizer
sudo install -m 755 "$build/target/release/archiver" /usr/local/bin/holdings-archiver

echo "== user, database and environment"
if ! id holdsync >/dev/null 2>&1; then
  sudo useradd --system --home-dir "$sync_data" --create-home --shell /usr/sbin/nologin holdsync
fi
sudo install -d -o holdsync -g holdsync -m 750 "$archive_data"

# set KEY=VALUE in a root-only env file, without printing VALUE
set_env() {
  local file="$1" key="$2" value="$3"
  sudo touch "$file"
  sudo chmod 600 "$file"
  sudo sed -i "/^$key=/d" "$file"
  printf '%s=%s\n' "$key" "$value" | sudo tee -a "$file" >/dev/null
}

rpc_url=""
if [ -s "$build/rpc_url" ]; then
  rpc_url="$(cat "$build/rpc_url")"
  rm -f "$build/rpc_url"
elif sudo test -f "$sync_env"; then
  rpc_url="$(sudo sed -n 's/^RPC_URL=//p' "$sync_env")"
fi
[ -n "$rpc_url" ] || rpc_url=https://ethereum-sepolia-rpc.publicnode.com

if ! sudo grep -q '^STATE_LOG_DB_URL=' "$sync_env" 2>/dev/null; then
  password="$(openssl rand -hex 24)"
  sudo -u postgres psql -qc "CREATE ROLE holdsync LOGIN PASSWORD '$password'"
  sudo -u postgres createdb -O holdsync synchronizer
  set_env "$sync_env" STATE_LOG_DB_URL "postgres://holdsync:$password@127.0.0.1:5432/synchronizer"
fi
set_env "$sync_env" RPC_URL "$rpc_url"
set_env "$sync_env" BEACON_URL "$beacon"
set_env "$sync_env" ARCHIVER_URL "http://127.0.0.1:3001"
set_env "$sync_env" TO_ADDRESS "$to_address"
set_env "$sync_env" INIT_START_SLOT "$genesis_slot"
set_env "$sync_env" CONTAINER_DB_PATH "$sync_data/db"
set_env "$sync_env" HTTP_BIND "127.0.0.1:3000"
set_env "$sync_env" RUST_LOG "info"

set_env "$archive_env" RPC_URL "$rpc_url"
set_env "$archive_env" BEACON_URL "$beacon"
set_env "$archive_env" FILTER_ADDRESS "$to_address"
set_env "$archive_env" INIT_START_SLOT "$genesis_slot"
set_env "$archive_env" BLOBS_PATH "$archive_data/blobs/"
set_env "$archive_env" HTTP_BIND "127.0.0.1:3001"
set_env "$archive_env" RUST_LOG "info"

echo "== services"
sudo tee /etc/systemd/system/holdings-archiver.service >/dev/null <<EOF
[Unit]
Description=Digital Objects blob archiver for the holdings synchronizer
After=network-online.target
Wants=network-online.target

[Service]
User=holdsync
EnvironmentFile=$archive_env
WorkingDirectory=$archive_data
ExecStart=/usr/local/bin/holdings-archiver
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
sudo tee /etc/systemd/system/holdings-synchronizer.service >/dev/null <<EOF
[Unit]
Description=Digital Objects synchronizer with the holdings witness endpoint
After=network-online.target postgresql.service holdings-archiver.service
Wants=network-online.target holdings-archiver.service
Requires=postgresql.service

[Service]
User=holdsync
EnvironmentFile=$sync_env
WorkingDirectory=$sync_data
ExecStart=/usr/local/bin/holdings-synchronizer
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable -q holdings-archiver holdings-synchronizer
sudo systemctl restart holdings-archiver
sudo systemctl restart holdings-synchronizer
sleep 5
systemctl --no-pager status holdings-archiver holdings-synchronizer | grep -E '●|Active:'
echo "== done"
