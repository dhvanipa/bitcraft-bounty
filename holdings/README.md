# holdings: prove what you hold

A prototype of a Bitcraft leaderboard whose scores are proofs.

Today's leaderboard is the game's Discord bot. It reads the Total Inventory
Value that the Bitcraft app shares as a Discord presence, and its own README
says the scores are not verified: anyone can share a presence as the Bitcraft
application, since its ID is public. A player can't prove a score by uploading
item files instead, because whoever has an item's file can spend the item.

`holdings` proves, for each item, that the prover holds it, unspent, at one
recent state of the chain, without showing it. Anyone can check the proof
against the chain.

## What a proof says

For each item, `Holds(state_root, type, tag)`
([src/holdings.podlang](src/holdings.podlang)): the prover knows an object
state such that

- it is live at the state root, and `tag` is its tag (`LiveObject`):
  - its commitment is in the state's created array. The synchronizer only
    adds commitments from valid `TxFinalized` proofs, so a valid transaction
    made it under the rules of its class;
  - its nullifier, `H(H(obj, obj.key), "txlib-nullifier-v1")`, the spend tag
    the network records when the item is consumed or re-keyed, is not in the
    state's nullifier set, by a Merkle proof of absence. Only a holder of the
    item's key can compute the nullifier, and it stays private;
  - `tag` is `H(obj.stable_identifier, "dobj-object-tag-v1")`. Every state
    of an object keeps its `stable_identifier`, so the tag stays the same
    when the item is re-keyed, and copies of one file give the same tag;
- `type` is its class: the hash of the plugin's `Is<Class>` predicate.

holdings.podlang is not specific to Bitcraft: `type` is whatever class an
object's plugin gave it, and the tag's domain names Digital Objects, not an
app. The Bitcraft parts are the verifier's class names and values and the
bounty's gem table. The bounty's bounty.podlang imports holdings.podlang by
its hash (`use module ... as holdings`; live.rs fills the hash in), and a
predicate's hash covers the modules it uses, so `BountyClaim`'s hash, which a
deployed contract pins, pins holdings.podlang too: change it only as a new
version. `LiveObject` uses all 5 of pod2's statements per predicate; `Holds`
adds the class check.

Every item in a proof is claimed at the same state root. Each pod also makes
the player's name public as `Equal(name, name)`, which ties the proof to that
name: a renamed proof fails to verify.

Until 2026-11-09 `Holds` proved each item at the root of its own creation and
made its nullifier public, and the verifier asked the network whether each
nullifier had been spent. Proving liveness in the circuit keeps nullifiers
private, so publishing a proof no longer lets anyone watch for the items being
spent, and makes `verify` one lookup. The cost: a proof is about one state,
so an item can be spent after it, and proving needs a synchronizer that can
prove a nullifier absent ([../synchronizer/](../synchronizer/README.md)).

## What `verify` checks

1. Each pod is a real proof: not a mock pod, a MainPod, pod2's default
   parameters and verifier-data set, and plonky2 verification passes.
2. Each public statement is this `Holds` (same predicate batch id and index)
   about the file's state root, or the player's name, and every pod names the
   same player.
3. `type` maps to a Bitcraft class. The verifier gets the mapping by compiling
   the plugin, which must compile to the network's module,
   `0x54548dac8ef19568c4ccbbb7912c18c6a70941eba3953ce5c95acbbd3eade36e`.
4. The state root is the synchronizer's at the block the file names
   (`/v1/state/by_el/{block}`); the hosted synchronizer serves this. It
   prints how many minutes before the check that state was. How old a state
   a leaderboard accepts is its choice.
5. No tag is claimed twice in the file.
6. Values come from the Bitcraft app's `inventory-values.json`. `Holds` does not
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
holdings prove --player NAME --sync http://127.0.0.1:3000 --out proof.json ~/.bitcraft/objects/bitcraft__emerald_*.dobj
```

proves every item that is live at the synchronizer's head (or at execution
block `--block N`), and skips, with a note, those spent or not yet created
there. `--sync` must be a synchronizer with the holdings witness endpoint: ours
runs on a box ([../synchronizer/](../synchronizer/README.md)), reached through
`ssh -L 3000:127.0.0.1:3000`. The hosted one has no such endpoint.

```bash
holdings verify --pexe ~/.bitcraft/actions/bitcraft.pexe proof.json
```

`holdings classes --pexe ...` prints the module hash, the `Holds` predicate's
hash and each class's type hash. `prove --chunk N` sets the items per pod
(default 9). `holdings onchain-pod --sync URL --out pod.json ITEM.dobj` proves
one item's `Holds` as a pod's only public statement, for a contract to check
(see [../onchain/](../onchain/README.md)).

Item files are in `~/.bitcraft/objects/` for the Bitcraft app and in
`~/.dobj/objects/` for the `dobj` CLI. Proving only reads them: it runs no
action and spends nothing. It sends the synchronizer each item's commitment
and nullifier, so whoever runs the synchronizer learns which items were
proved; use one you trust or run your own. Proving uses every core and about
3.4 GB, so on a machine that is also proving the game's actions, it slows
them down while it runs.

## Measured (2026-11-09, Mac M3 Pro, 11 cores)

At Sepolia block 12098387, against our synchronizer through an SSH tunnel,
with the Mac helper's GPU search running on its medium setting (load average
about 15):

| Proof | Wall time | CPU time | Peak memory | File size |
|---|---:|---:|---:|---:|
| 1 Emerald (`onchain-pod`) | 7.9 s | 45 s | 3.4 GB | 223 kB |
| 50 items (3 Emeralds, 47 Sapphires), 6 pods | 74.9 s | 469 s | 3.9 GB | 1.5 MB |

That is about 1.5 s of wall time per item; each pod of 9 items took 13.1 to
13.8 s. Fetching the witness for 50 items is one request.

The same 50 items took 153 s (569 s of CPU) with the earlier `Holds`, which
did not prove nullifiers absent, under a load on the Mac that was not
recorded, so the two runs don't measure a speed-up. The load matters: 11
items took 21 s in one run and 43 s in another while the helper's search used
950% CPU.

Checking the 6 pods took 0.02 s, and the whole `verify` 1.24 s: one
synchronizer request for the one state root. The earlier `verify` took 9.7 s,
one request per item's creation root plus a nullifier query.

## Tested

On 2026-11-09, proved against our synchronizer and verified against the
hosted one:

| Attempt | Result |
|---|---|
| 50 real items at block 12098387 | verified: every item live there, 4,280,000 BIT |
| 11 early items (Quartz, a Pocketknife) at block 12059155 | verified; our synchronizer's root there equals the hosted one's |
| the player's name changed inside the pods | plonky2 verification fails |
| one character of a pod's proof changed | plonky2 verification fails (invalid Merkle proof) |
| a mock pod | refused as a mock |
| an Emerald's file relabelled as a Diamond | skipped: its commitment is not in the created array ("not created at this state"); the circuit refuses it too (the `an_item_not_in_the_state` test) |
| 100 items from an old copy of the server's inventory | 91 skipped as spent at block 12098387, 9 proved |
| the same pod twice in one file | refused: an object claimed twice |
| the file's block or state root changed | refused: the root is not the network's at that block, or the claims are about another root |
| pods from two players, at one state, merged into one file | refused: different players |
| the same Emerald proved by a second player | verifies, with the same tag (`0x82572f50dd…`), so a leaderboard can count it once |

The mock-prover tests (`cargo test -p holdings`) check the circuit itself: a
live item is claimed and its nullifier is not public; a spent item, or an
item not in the state, can't be claimed; a re-keyed item is claimed only in
its new state, with the same tag.

## A bounty claim

`BountyClaim(state_header, recipient, threshold)`
([src/bounty.podlang](src/bounty.podlang)) is the statement the bounty
contract ([../onchain/](../onchain/README.md#the-bounty-bountyescrow)) pays
for. The prover knows object states, each with a different tag, such that
each one

- is live at `state_header`, as in `Holds` (`LiveObject`): in its created
  array, and unspent there, its nullifier absent from its nullifier set by a
  Merkle proof of absence;
- is a gem: `H(type, value)` is in a set with one entry per gem class and its
  value in BIT, so the value is the gem's (`LiveGem`);

and their values add up to at least `threshold` (`GemsTotal`, one step per
gem; `SetInsert` of each object's tag, which stays private here, requires it
to be new, so no object counts twice). `state_header` is the state's header record, whose hash is the
synchronizer's `state_root`. `recipient` is the address the bounty pays, as 4
limbs of 40 bits, made public so a copied proof pays the same address.

The gem table ([src/bounty.rs](src/bounty.rs), `GEMS`):

| Gem | BIT |
|---|---:|
| Quartz | 100 |
| Amethyst | 2,000 |
| Sapphire | 40,000 |
| Emerald | 800,000 |
| Ruby | 16,000,000 |
| Diamond | 320,000,000 |

```bash
holdings bounty-info --threshold 50000000
```

prints the `BountyClaim` predicate's hash (it commits to the gem table) and
the threshold packed as the contract reads them (and a recipient, with `--recipient`).

```bash
holdings bounty-pod --sync http://127.0.0.1:3000 --block BLOCK \
  --recipient 0xADDRESS --threshold 50000000 --out claim.json ITEM.dobj...
```

proves the claim as a pod's only public statement, against the state at
execution block `BLOCK` (a block whose root the contract holds:
`rootBlock(root)`). It needs a synchronizer with the witness endpoint
([../synchronizer/](../synchronizer/README.md)), which gives each item's
creation proof and nullifier absence proof at that block; the hosted
synchronizer has none. It refuses a spent item, an item not created by then,
a non-gem, the same object twice, and a total below the threshold.

## What a leaderboard still needs

- **Count each tag once across players.** Copies of one file are equally held
  until someone re-keys the item, so two players can prove the same item at
  a state before the re-key. The tag shows the collision, and the
  leaderboard decides who gets the item (first claim, or neither).
- **Rank on recent states, not peaks.** A proof is about one state, so a
  peak can be gamed: prove, pass the items on with a re-key, and let the next
  player prove them. Accept only proofs at a recent state (the verifier
  prints its age) and have players re-prove; an old copy of a re-keyed item
  can't prove at any state after the re-key.
- **Tie the name to an account.** The proof binds a name but doesn't
  authenticate it. The site should require the name to match the signed-in
  account (Discord, say).
- **Run its own synchronizer** if it doesn't want to trust the hosted one for
  state roots, and offer players a witness endpoint, since the hosted
  synchronizer has none.
- **Prove level, fill and durability** for drills, buckets and axes, in a
  Bitcraft predicate built on `LiveObject` (`DictContains(obj, "level",
  level)` and the like), so that holdings.podlang stays generic.
- **Privacy.** Nullifiers stay private, but the tags link a player's claims
  over time, and the synchronizer that serves the witness sees which items it
  was asked about.
