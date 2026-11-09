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

Not built yet: the website that takes proofs, re-checks the claimed items for
spends, counts each item once across players, and ranks players.
[holdings/README.md](holdings/README.md#what-a-leaderboard-still-needs) lists
what it needs.
