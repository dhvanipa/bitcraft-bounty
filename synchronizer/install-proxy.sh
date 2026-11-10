#!/usr/bin/env bash
# Serves the synchronizer and the archiver to the public over HTTPS, with
# Caddy in front of their loopback ports. Run on the box, after install.sh:
#
#   bash ~/sync-build/deploy/install-proxy.sh SYNC_HOST ARCHIVER_HOST [EMAIL]
#
# e.g. synchronizer.don.pateldhvani.com archiver.don.pateldhvani.com. Before
# running it, point both names' A records at the box's Elastic IP (DNS only:
# Cloudflare's free proxy certificate doesn't cover second-level names like
# these) and open ports 80 and 443 in its security group. Caddy gets a Let's
# Encrypt certificate for each name on first start (HTTP-01 challenge on
# port 80), renews them, and redirects HTTP to HTTPS. EMAIL, optional, is
# where Let's Encrypt sends expiry notices.
#
# Both APIs only read: the synchronizer answers state, membership, witness and
# transaction queries, the archiver serves blobs. Both send permissive CORS
# headers themselves, and keep listening on 127.0.0.1 only. Safe to re-run.
set -euo pipefail

sync_host="${1:?usage: install-proxy.sh SYNC_HOST ARCHIVER_HOST [EMAIL]}"
archive_host="${2:?usage: install-proxy.sh SYNC_HOST ARCHIVER_HOST [EMAIL]}"
email="${3:-}"

echo "== caddy"
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy >/dev/null
caddy version

global=""
[ -n "$email" ] && global="{
	email $email
}
"
sudo tee /etc/caddy/Caddyfile >/dev/null <<CADDY
${global}# install-proxy.sh writes this file; edit the script, not this.
$sync_host {
	encode zstd gzip
	reverse_proxy 127.0.0.1:3000
	log {
		output file /var/log/caddy/synchronizer.log {
			roll_size 50MiB
			roll_keep 5
		}
	}
}

# Blobs are random-looking bytes: compressing them costs CPU for nothing.
$archive_host {
	reverse_proxy 127.0.0.1:3001
	log {
		output file /var/log/caddy/archiver.log {
			roll_size 50MiB
			roll_keep 5
		}
	}
}
CADDY
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
sudo systemctl enable -q caddy
sudo systemctl reload-or-restart caddy

echo "== certificates"
for host in "$sync_host" "$archive_host"; do
  for _ in $(seq 1 30); do
    curl -fsS -m 10 "https://$host/healthz" >/dev/null 2>&1 && break
    sleep 2
  done
  printf '%s: ' "$host"
  curl -fsS -m 10 "https://$host/healthz" && echo || echo "no answer over HTTPS yet; see journalctl -u caddy"
done
