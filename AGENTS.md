# AGENTS.md

Notes for coding agents working in this repo. `CLAUDE.md` imports this file.
Read [README.md](README.md) first: it has the layout and how a claim flows.
Each folder's README explains its choices with the measurements behind them.

## Parts of this are live

- **The bounty contract on Sepolia** is immutable. It pins the `BountyClaim`
  predicate's hash, the threshold and the Groth16 verifier at deployment, so
  a change to any of them is a new deployment, not an edit.
- **The synchronizer's box** serves the public synchronizer and archiver
  (`synchronizer.don.pateldhvani.com`, `archiver.don.pateldhvani.com`) and
  posts state roots to the bounty every hour. Ask the owner before running
  `deploy.sh`, `install*.sh` or `systemctl restart|stop` there: people prove
  against it. Reading it (`systemctl status`, `journalctl`, `curl
  localhost:3000/...`) is fine.
- **The poster's key is worth the prize.** It lives on the box in root-only
  `/etc/holdings-root-poster/`. Never print, copy or commit it.
- **RPC URLs carry API tokens.** Never print or commit one (the box keeps its
  own in root-only `/etc/holdings-*.env`). The scripts pipe them without
  printing; keep it that way.
- **Proving is heavy:** `holdings bounty-pod` uses every core and about 4 GB,
  and the Groth16 prover about 10 GB. On a shared machine, ask before
  running a real proof; `--mock` is a cheap dry run.

## Build and test

| Part | Check |
|---|---|
| `holdings/` | `holdings/build.sh /path/to/digital-objects-network` copies `holdings/` into a clone of the game's workspace at `616e0eb` (in `~/.cache/bitcraft-holdings`, or `BUILD_DIR`) and builds it there. It copies again on every run, so run it after each edit; then `cd ~/.cache/bitcraft-holdings/src && CARGO_TARGET_DIR=../target cargo test --release -p holdings` (12 tests) and `cargo clippy --release -p holdings` in the same place |
| `onchain/` | `forge test -vv` (32 tests; two use real proofs from `test/fixtures/`) |
| `site/` | `node --test site/test.mjs` (9 tests). To see it against a local chain: deploy the contracts on `anvil`, post a root, and open `site/index.html` through any static server with `?bounty=0x…&rpc=http://127.0.0.1:8545` |
| `synchronizer/` | shell scripts: `bash -n` and `shellcheck`. The patch is tested in the game's workspace (`cargo test -p synchronizer nullifier_proofs`) |

## Things that must stay in sync

- **The game's revision.** `616e0eb` (v0.1.0-rc.50) in `holdings/build.sh`,
  `synchronizer/deploy.sh` and the READMEs. pod2 (`331a1145`) and plonky2
  (`60e32369`) come from that workspace. Another revision can hash items
  differently from the network.
- **Predicate hashes.** A predicate's hash covers its module and every module
  it imports, so editing `holdings.podlang` or `bounty.podlang`, a class
  hash in `bounty.rs`'s `CLASSES`, or a value in the game's
  `inventory-values.json` changes it. `BountyClaim`'s hash
  (`holdings bounty-info`) is pinned in `onchain/README.md` (deploy),
  `onchain/test/BountyEscrowProof.t.sol` (`PREDICATE`) and
  `site/config.js` (`planned.claimPredicate`); `Holds`'s in
  `onchain/test/HoldsCheck.t.sol`. The fixtures were proved with the old hash,
  so a change needs new fixtures too.
- **The Groth16 setup.** `onchain/src/Verifier.sol`, the proofs in
  `onchain/test/fixtures/`, `onchain/groth16-setup.sha256`, `site/config.js`
  (`setup.files`) and the `groth16-setup-1` release all belong to one setup.
  Never re-run the setup for this verifier: a new one invalidates all of them
  and every deployed verifier.
- **The contract's ABI and the page.** `site/chain.js` hard-codes selectors
  (`cast sig`) and event topics (`cast keccak`). Recompute them after changing
  a function or event the page uses; `site/test.mjs` checks the claim's
  calldata against the real fixture.
- **The witness endpoint and its client.** The patch takes at most 256 hashes
  a request; `holdings/src/live.rs` sends 128 items at a time (a commitment and
  a nullifier each), as plain hex without `0x`.
- **Domains.** `NULLIFIER_VERSION` (`txlib-nullifier-v1`, the game's) and
  `TAG_VERSION` (`dobj-object-tag-v1`) in `live.rs` are part of the proved
  statements.

## Conventions

- **Docs explain why, with measurements.** The READMEs record each choice with
  the number that set it and an absolute date (UTC), for example "on
  2026-11-09 a dry run of 300 items planned 92 pods in 52 s". Plain sentences,
  numbers with units, no marketing. Say what was measured and what is only
  estimated, and update a number when a change makes it stale.
- **Comments** follow the same style and carry the reasoning; read them
  before changing a constant.
- **Commit messages**: the subject is one plain sentence about the outcome
  ("Count every item at the app's value, against roots from the last 24
  hours"). The body says what was wrong, what changed, and what was measured
  after.
- **The page has no build step and no dependencies.** Keep it to static
  files: `index.html`, `style.css`, `app.js`, the pure `chain.js` and
  `config.js`.
- Changes land on `main` through pull requests.
