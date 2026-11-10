# bitcraft-leaderboard

Zero-knowledge proofs of what a player holds in
[Bitcraft](https://github.com/0xPARC/digital-objects-network), the crafting
game on the Digital Objects Network, and a Sepolia bounty that pays for one.

A player proves which items they hold, unspent, at a recent state of the
game, without showing the items or anything that would let someone watch for
them being spent. Anyone can check the proof against the chain. Two things
use it:

- **A leaderboard whose scores are proofs.** `holdings prove` proves a
  player's items one by one; `holdings verify` checks them against the chain.
- **A bounty.** `BountyEscrow`, on Sepolia, pays its USDC balance to the first
  proof of unspent items worth at least 50,000,000 BIT, counted as the
  Bitcraft app counts them, at a state root posted in the last 24 hours.
  [The bounty page](https://dhvanipa.github.io/bitcraft-leaderboard/)
  ([site/](site/)) shows whether it is open, which roots a claim can use, how
  to claim, and checks a claim before it is sent.

Status: a working prototype on Sepolia testnet, tested on real items on
2026-11-09 and 2026-11-10. The bounty is deployed at
[`0x91d3219a384ab57262287fFc9d11b96cE7d1d434`](https://sepolia.etherscan.io/address/0x91d3219a384ab57262287fFc9d11b96cE7d1d434),
open until 2027-01-09. The first real claim was paid on Sepolia on
2026-11-10, against a 2,000,000 BIT test bounty
([onchain/README.md](onchain/README.md#the-first-claim-paid-on-sepolia-2026-11-10)). It is not ready for real money: see
[what you have to trust](onchain/README.md#not-for-real-money-yet).

## Why

Bitcraft's current leaderboard is a Discord bot. It reads the Total Inventory
Value that the Bitcraft app shares as a Discord presence, and keeps each
player's highest. Its README says the scores are not verified: anyone can share
a presence as the Bitcraft application, since its ID is public. Players can't
prove a score by uploading their item files instead, because whoever has an
item's file can spend the item.

Here a player proves which items they hold with a zero-knowledge proof that
shows nothing else: that each item was made by a valid transaction, which
class it is, and that it has not been spent at a given state of the chain.

## Layout

| Folder | What |
|---|---|
| [holdings/](holdings/README.md) | the prover and verifier (`holdings prove`, `verify`, `bounty-pod`), and the pod2 predicates they prove: [holdings.podlang](holdings/src/holdings.podlang), generic to any Digital Objects app, and the bounty's [bounty.podlang](holdings/src/bounty.podlang) |
| [onchain/](onchain/README.md) | the contracts: proofs wrapped in Groth16 with pod2-onchain and verified in Solidity for about 545,000 gas; `HoldsCheck` for one item, and `BountyEscrow` |
| [synchronizer/](synchronizer/README.md) | our run of the game's synchronizer and archiver, patched to prove an item unspent at a past state, public at `synchronizer.don.pateldhvani.com` and `archiver.don.pateldhvani.com`; and the hourly poster of finalized state roots to the bounty |
| [site/](site/README.md) | the bounty page: static HTML and JavaScript with no build step, reading the contract from a public Sepolia node |

## How a claim gets paid

```
 your item files ──► holdings bounty-pod ──► a pod2 MainPod: BountyClaim(root, you, 50M BIT)
                           ▲                          │
            each item's creation proof                ▼
            and proof of not being spent     wrap (pod2-onchain): plonky2, BN254-friendly
            at the root's block                       │
                           │                          ▼
   synchronizer ◄── archiver ◄── Sepolia blobs   Groth16 proof (the published setup)
        │                                             │
        └── poster: postRoot(root, block), hourly ──► BountyEscrow.claim(...) ──► USDC to you
```

The contract checks that the claim is this bounty's `BountyClaim`, that its
state root was posted in the last 24 hours, its threshold and recipient, and
the Groth16 proof; then it pays its whole balance once. The recipient is part
of the proved statement, so a proof copied from the mempool pays the same
address.

## Build and test

| Part | Needs | Check |
|---|---|---|
| `holdings/` | Rust nightly, a checkout of [digital-objects-network](https://github.com/0xPARC/digital-objects-network) | `holdings/build.sh /path/to/digital-objects-network` builds it in a clone of the game's workspace at `616e0eb` and prints the binary's path; then `cd ~/.cache/bitcraft-holdings/src && CARGO_TARGET_DIR=../target cargo test --release -p holdings` (12 tests, pod2's mock prover) |
| `onchain/` | [Foundry](https://getfoundry.sh) | `cd onchain && forge test` (32 tests: 19 of the escrow's rules with a mock verifier, 13 with real proofs through the real verifier: an Emerald's `Holds` and a 19-item claim) |
| `site/` | Node 22 | `node --test site/test.mjs` (9 tests); `python3 -m http.server -d site` to view it |
| `synchronizer/` | the game's workspace | the patch's test, `cargo test -p synchronizer nullifier_proofs`, with [the patch](synchronizer/patches/holdings-witness.patch) applied at `616e0eb` |

`holdings` builds inside the game's workspace because it uses the game's own
libraries, at the release the network runs (v0.1.0-rc.50), so it hashes items
exactly as the network does. CI runs the Foundry and Node tests, and a push
to `main` that changes `site/` publishes it to GitHub Pages.

## Contributing

Issues and pull requests are welcome. [AGENTS.md](AGENTS.md) lists what must
change together (a predicate's hash is pinned by a deployed contract, the
verifier by its setup) and the conventions the docs follow; it is written for
coding agents and reads as well for people.

To report a security problem privately, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE), like the game's own code.
