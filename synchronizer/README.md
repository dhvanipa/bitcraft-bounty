# synchronizer: state roots and spend proofs for the bounty

The bounty contract ([../onchain/src/BountyEscrow.sol](../onchain/src/BountyEscrow.sol))
takes a claim against a game state root that its poster put on Sepolia. The
claim proves each item unspent at that root: its nullifier is absent from the
root's nullifier set. Proving that needs a Merkle proof of absence at the
posted root.

The hosted synchronizer (`synchronizer.digiobjects.net`) can't give one. Its
`/v1/state/membership` answers only yes or no for a nullifier, and its
`/v1/txlib/grounding-witness` proves created objects, not nullifiers, and
only at its head, which moves every 12 s slot. So we run the game's own
synchronizer with one patch, and its archiver, on a box of our own, and
serve both to anyone:

- **synchronizer:** `https://synchronizer.don.pateldhvani.com`, the hosted
  synchronizer's API plus `POST /v1/holdings/witness`. `holdings` uses it by
  default.
- **archiver:** `https://archiver.don.pateldhvani.com`, the network's blobs
  since the genesis slot, kept after the beacon drops them (18 days), for
  anyone running a synchronizer (`ARCHIVER_URL`).

## What runs

| Service | What | Listens |
|---|---|---|
| `holdings-archiver` | the game's archiver: keeps the network's blobs (sent to `0x4343…43`) on disk, since the beacon drops blobs after 18 days | 127.0.0.1:3001 |
| `holdings-synchronizer` | the game's synchronizer at `616e0eb` (v0.1.0-rc.50, what the hosted one reports at `/info`), with [patches/holdings-witness.patch](patches/holdings-witness.patch); reads its blobs from our archiver | 127.0.0.1:3000 |
| `holdings-root-poster.timer` | hourly: posts the state root at Sepolia's finalized block to the bounty ([post-root.sh](post-root.sh)) | |

Both start at slot 11491968 (2026-11-02 16:33 UTC), the genesis slot the
hosted synchronizer reports, and derive every state from the blobs: the
synchronizer verifies each transaction's `TxFinalized` proof itself, so its
roots depend only on the chain, not on the hosted one. The genesis blobs leave
the beacon on 2026-11-20 at 21:28 UTC; after that a new synchronizer can only
start from an archiver that kept them.

The synchronizer's Postgres keeps every slot's roots, and its RocksDB keeps
tree nodes by hash and never deletes them (only a reorg rolls back slot
rows). So any past root it derived can still be proved against, which is what
lets a prover answer for the root the contract holds rather than the head.

## The patch: `POST /v1/holdings/witness`

```json
{ "blockNumber": 12096897, "objectCommitments": ["…"], "nullifiers": ["…"] }
```

returns, at the state of execution block `blockNumber` (the head if left out),
the state header (`stateRoot`, `blockNumber`, `blockTimestamp`, `blockHash`,
`createdRoot`, `nullifiersRoot`, `priorStateHistoryRoot`), a `createdProofs`
entry per commitment (`present`, `index` and the Merkle proof, as the
grounding witness gives them), and a `nullifierProofs` entry per nullifier:
`spent`, and a Merkle proof of presence when spent or of absence when not.
`holdings bounty-pod --sync URL --block N` calls it.

The patch also adds `ContainerDb::prove_nullifiers_for` and a test that proves
presence and absence at one root; `cargo test -p synchronizer
nullifier_proofs` passes on 2026-11-09.

## Deploy

On a fresh Ubuntu box (we use an m7i.large: 2 vCPUs, 8 GB, 96 GB disk), from
a Mac with the game's source:

```bash
RPC_FROM=ubuntu@GAME-SERVER synchronizer/deploy.sh ubuntu@HOST KEY.pem /path/to/digital-objects-network
```

copies the game's source at `616e0eb` and this folder to the box and runs
[install.sh](install.sh) there in the background. It adds 8 GB of swap (the
build needs more than 8 GB), installs Postgres and Rust, applies the patches,
builds both services, makes a `holdsync` user and database, writes root-only
env files (`/etc/holdings-synchronizer.env`, `/etc/holdings-archiver.env`) and
starts both services. Re-running rebuilds and keeps the data.

`RPC_FROM` names a box running bitcraft-agent's relayer. Its Sepolia
`RPC_URL`, which carries an API token, is piped from that box's root-only env
file to the new box's, never printed or written elsewhere. Without it the
services use the public node, which rate-limits a catch-up.

Then the poster, once the synchronizer has caught up:

```bash
bash ~/sync-build/deploy/install-poster.sh            # on the box: make the poster's key, print its address
bash ~/sync-build/deploy/install-poster.sh 0xBOUNTY   # after deploying the bounty with that address as poster
```

[install-poster.sh](install-poster.sh) installs `cast` (Foundry v1.7.1) and
`jq`, makes the poster's key on the box in an encrypted keystore with a random
password, both in root-only `/etc/holdings-root-poster/`, and prints only its
address. The service runs as a throwaway user (`DynamicUser`) and gets the
keystore and password through systemd credentials. The poster needs a little
Sepolia ETH: `postRoot` writes one storage slot.

## The poster

Each hour [post-root.sh](post-root.sh):

1. stops if the bounty has a winner;
2. takes Sepolia's finalized block (about 16 minutes behind the head on
   2026-11-09) and our synchronizer's state at it, so a posted root can't be
   reorged out;
3. compares that root with the hosted synchronizer's at the same block, and
   does not post if they differ. If the hosted one doesn't answer, ours alone
   decides;
4. skips a root already posted, and otherwise sends
   `postRoot(root, block)`.

A failed run (our synchronizer behind, roots differing, a reverted
transaction) exits non-zero and shows in
`systemctl status holdings-root-poster` and its journal.

**The poster key is worth the prize.** The contract trusts the roots it is
given: a made-up root could commit to made-up items. Keep the box's SSH key as
safe as the prize, and use the contract's `setPoster` to move the role if the
box is lost.

## Check it

```bash
ssh -i KEY.pem ubuntu@HOST
systemctl status holdings-archiver holdings-synchronizer
journalctl -u holdings-synchronizer -n 50
curl -s localhost:3000/sync-progress            # compare with the hosted one's
curl -s localhost:3000/v1/state/by_el/BLOCK     # same state_root as the hosted one's
systemctl list-timers holdings-root-poster.timer
journalctl -u holdings-root-poster -n 20
```

Both services listen on loopback only; Caddy serves them to the public
(below).

## Public endpoints

| Name | Serves |
|---|---|
| `https://synchronizer.don.pateldhvani.com` | the synchronizer, 127.0.0.1:3000 |
| `https://archiver.don.pateldhvani.com` | the archiver, 127.0.0.1:3001 |

[install-proxy.sh](install-proxy.sh) installs Caddy from Ubuntu's archive
(2.6.2) and proxies each name to its loopback port; Caddy gets a Let's
Encrypt certificate for each name, renews it, and redirects HTTP to HTTPS.
Before running it:

1. give the box an Elastic IP: an instance's own public IP changes when it
   stops (ours went from 3.148.104.216 to the Elastic IP 3.135.44.34);
2. point both names' A records at it, **DNS only** in Cloudflare: Cloudflare's
   free proxy certificate covers `*.pateldhvani.com` but not second-level
   names like `synchronizer.don.pateldhvani.com`;
3. open TCP 80 and 443 to everyone in the security group.

```bash
bash ~/sync-build/deploy/install-proxy.sh synchronizer.don.pateldhvani.com archiver.don.pateldhvani.com [EMAIL]
```

Both APIs only read, and both send permissive CORS headers themselves, so
browsers can call them. Access logs go to `/var/log/caddy/`.

On 2026-11-09, from a Mac: both certificates were issued within 5 s of
Caddy's start (Let's Encrypt YE2, valid to 2027-02-07); the synchronizer's
root at the finalized block equalled the hosted one's; `holdings prove`
against the public URL fetched witnesses for 6 Emeralds in 1.1 s; and the
archiver returned a slot's game blob (131,072 bytes) in 1.8 s.

Two things to watch: the box has 2 vCPUs and Caddy's stock build has no rate
limits, so heavy public use of the witness endpoint (up to 128 items per
request) would slow the synchronizer the poster reads; and whoever runs the
synchronizer sees which items a prover asks about.

## Measured (2026-11-09, m7i.large)

| What | Measured |
|---|---|
| first build of both services (`install.sh`) | 20 min 16 s |
| catch-up from the genesis slot to the hosted head | 3 h 35 min (10:45:43 to 14:20:21 UTC), 49,733 slots, 3.9 slots/s |
| CPU during the catch-up | load average 0.3 to 0.4 of 2 vCPUs: the archiver fetches one slot at a time, so request latency sets the pace |
| disk after the catch-up (7 days of chain) | archived blobs 7.2 GB (about 1 GB per day), synchronizer 2.7 GB, Postgres 81 MB; 72 GB free |
| state roots against the hosted synchronizer | equal at all 16 blocks compared, from the first state (block 12049833) to the finalized block 12098006 |
| the relayer sharing the RPC | no errors in its journal during the catch-up |

The synchronizer runs 2 slots behind the archiver while catching up and logs
"archived blobs ... not available yet" each time it waits for it; that
warning is expected.

## Shared RPC

The synchronizer and the archiver use the same Sepolia RPC as the game
server's relayer. While catching up they make many more requests than the
relayer does, so if the RPC plan has a rate limit, the relayer can see
errors during the catch-up.
