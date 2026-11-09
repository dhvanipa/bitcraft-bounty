# holdings: prove what you hold

A prototype of a Bitcraft leaderboard whose scores are proofs.

Today's leaderboard is the game's Discord bot. It reads the Total Inventory
Value that the Bitcraft app shares as a Discord presence, and its own README
says the scores are not verified: anyone can share a presence as the Bitcraft
application, since its ID is public. A player can't prove a score by uploading
item files instead, because whoever has an item's file can spend the item.

`holdings` proves, for each item, that the prover holds it without showing
it. Anyone can check the proof against the chain.

## What a proof says

For each item, `Holds(state_root, type, nullifier, tag)`
([src/holdings.podlang](src/holdings.podlang)): the prover knows an object
state such that

- its commitment is in the created array of the state root. The synchronizer
  only adds commitments from valid `TxFinalized` proofs, so a valid
  transaction made it under the rules of its class;
- `type` is its class: the hash of the plugin's `Is<Class>` predicate;
- `nullifier` is the spend tag the network records when the item is consumed,
  `H(H(obj, obj.key), "txlib-nullifier-v1")`. Only a holder of the item's key
  can compute it;
- `tag` is `H(obj.stable_identifier, "bitcraft-holdings-v1")`. Every state of
  an object keeps its `stable_identifier`, so the tag stays the same when the
  item is re-keyed, and copies of one file give the same tag.

Each pod also makes the player's name public as `Equal(name, name)`, which ties
the proof to that name: a renamed proof fails to verify.

## What `verify` checks

1. Each pod is a real proof: not a mock pod, a MainPod, pod2's default
   parameters and verifier-data set, and plonky2 verification passes.
2. Each public statement is this `Holds` (same predicate batch id and index)
   or the player's name, and every pod names the same player.
3. `type` maps to a Bitcraft class. The verifier gets the mapping by compiling
   the plugin, which must compile to the network's module,
   `0x54548dac8ef19568c4ccbbb7912c18c6a70941eba3953ce5c95acbbd3eade36e`.
4. Each state root is the synchronizer's at the block the file names
   (`/v1/state/by_el/{block}`).
5. No nullifier is in the network's nullifier set (`/v1/state/membership`). A
   spent item counts 0.
6. No item or tag is claimed twice in the file.
7. Values come from the Bitcraft app's `inventory-values.json`. `Holds` does not
   prove level, fill or durability, so classes whose value depends on those
   (drills, buckets, axes) count 0 for now. Gems and plain resources count.

## Build and run

```bash
holdings/build.sh /path/to/digital-objects-network
```

builds it inside a clone of the game's workspace at `616e0eb`
(v0.1.0-rc.50, which the hosted synchronizer reports at `/info`), in
`~/.cache/bitcraft-holdings`, and prints the binary's path. The checkout itself
is not changed.

```bash
holdings prove --player NAME --out proof.json ~/.bitcraft/objects/bitcraft__emerald_*.dobj
```

```bash
holdings verify --pexe ~/.bitcraft/actions/bitcraft.pexe proof.json
```

`holdings classes --pexe ...` prints the module hash, the `Holds` predicate's
hash and each class's type hash. `prove --chunk N` sets the items per pod
(default 9). `holdings onchain-pod --out pod.json ITEM.dobj` proves one item's
`Holds` as a pod's only public statement, for a contract to check (see
[../onchain/](../onchain/README.md)).

Item files are in `~/.bitcraft/objects/` for the Bitcraft app and in
`~/.dobj/objects/` for the `dobj` CLI. Proving only reads them: it runs no
action and spends nothing. Each item file already carries the Merkle proof of
its creation (`creationProof`), so proving needs no network. An item whose
transaction has not landed has no creation proof yet and can't be proved.
Proving uses every core and about 3.4 GB, so on a machine that is also proving
the game's actions, it slows them down while it runs.

## Measured (2026-11-09, Mac M3 Pro, 11 cores)

| Proof | Wall time | CPU time | Peak memory | File size |
|---|---:|---:|---:|---:|
| 1 Emerald | 14.2 s | 52 s | 3.4 GB | 222 kB |
| 50 items (3 Emeralds, 47 Sapphires), 6 pods | 153 s | 569 s | 3.4 GB | 1.5 MB |

That is about 3 s of wall time per item. The limit per pod is pod2's 64
statements: an item takes about 10, so 5 items fit one pod, and 9 items need
two (28 s).

Checking the 6 pods took 0.04 s. The whole `verify` took 9.7 s, nearly all of
it one synchronizer request per state root, because each item's root is the
one from when the item was created.

The inventory these items came from held 861 Amethysts, 48 Sapphires and 4
Emeralds on 2026-11-09.
Proving all 913 gems would take about 46 minutes and 27 MB at the measured rate.
That is an estimate, not a measurement. Proving only the 52 Sapphires and
Emeralds would take about 2.7 minutes and cover 72% of the score.

## Tested

On 2026-11-09, against the hosted synchronizer:

| Attempt | Result |
|---|---|
| a real Emerald | verified, live, 800,000 BIT |
| the player's name changed inside the proof | plonky2 verification fails |
| one byte of the proof changed | plonky2 verification fails |
| a mock pod | refused as a mock |
| an Emerald file relabelled as a Diamond | its commitment is not on the record: the prover's check, and with that off pod2's Merkle check, refuse it |
| a spent Stone (an archived copy of a consumed item's file) | proves, since it was created, but its nullifier is on-chain: "SPENT, not counted" |
| the same pod twice in one file | refused: an item claimed twice |
| pods from two players merged into one file | refused: different players |
| the same Emerald proved by a second player | verifies, with the same tag (`0xf44df94e3a…`), so a leaderboard can count it once |

## What a leaderboard still needs

- **Count each tag once across players.** Copies of one file are equally held
  until someone re-keys the item, so two players can prove the same item. The
  tag shows the collision, and the leaderboard decides who gets the item
  (first claim, or neither).
- **Rank on current holdings, not peaks.** A peak can be gamed: prove, pass
  the items on with a re-key, and let the next player prove them. Re-check the
  nullifiers now and then; a spent or re-keyed item drops off.
- **Tie the name to an account.** The proof binds a name but doesn't
  authenticate it. The site should require the name to match the signed-in
  account (Discord, say).
- **Run its own synchronizer** if it doesn't want to trust the hosted one for
  state roots and spends.
- **Prove level, fill and durability** for drills, buckets and axes: a second
  predicate with `DictContains(obj, "level", level)` and the like, since `Holds`
  already uses pod2's limit of 5 statements per predicate.
- **One fresh state root per proof.** Proving against a recent root from
  `/v1/txlib/grounding-witness`, instead of each item's creation root, would
  make `verify` one lookup instead of one per item.
- **Privacy.** Publishing the nullifiers lets anyone see when a claimed item is
  spent, and the tags link a player's claims over time.
