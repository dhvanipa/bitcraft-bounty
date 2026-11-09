# bitcraft-leaderboard

A leaderboard for [Bitcraft](https://github.com/dobjlabs/digital-objects-network)
whose scores are proofs.

Bitcraft's current leaderboard is a Discord bot. It reads the Total Inventory
Value that the Bitcraft app shares as a Discord presence, and keeps each
player's highest. Its README says the scores are not verified: anyone can share
a presence as the Bitcraft application, since its ID is public. Players can't
prove a score by uploading their item files instead, because whoever has an
item's file can spend the item.

Here a player proves which items they hold with a zero-knowledge proof that
shows nothing else, and anyone can check it against the chain: that each item
was made by a valid transaction, which class it is, and that it has not been
spent.

## Layout

| Folder | What |
|---|---|
| [holdings/](holdings/README.md) | the prover and verifier, `holdings prove` and `holdings verify`, with the pod2 predicate they use. A working prototype, measured and tested on real items on 2026-11-09 |
| [onchain/](onchain/README.md) | holdings claims in a contract: proofs wrapped in Groth16 with pod2-onchain and verified in Solidity for about 545,000 gas, and `BountyEscrow`, which pays its balance to whoever first proves they hold unspent items worth a threshold of BIT, as the Bitcraft app values them, at a state root posted in the last 24 hours |
| [synchronizer/](synchronizer/README.md) | our own run of the game's synchronizer and archiver, patched to prove a nullifier absent at a past root, and the hourly poster of finalized state roots to the bounty |

Not built yet: the website that takes proofs, re-checks the claimed items for
spends, counts each item once across players, and ranks players.
[holdings/README.md](holdings/README.md#what-a-leaderboard-still-needs) lists
what it needs.
