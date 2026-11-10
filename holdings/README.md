# holdings: prove what you hold

The prover and verifier: proofs of which Bitcraft items a player holds,
unspent, without showing them. The bounty's claims are made with it
([A bounty claim](#a-bounty-claim)), and so could the scores of a leaderboard
whose scores are proofs, the use it was first written for.

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
holdings prove --player NAME --out proof.json ~/.bitcraft/objects/bitcraft__emerald_*.dobj
```

proves every item that is live at the synchronizer's head (or at execution
block `--block N`), and skips, with a note, those spent or not yet created
there. A whole objects folder can be given: files that aren't Bitcraft item
files are skipped with one line. On 2026-11-10 a Mac app's folder held 89
files from before the current network started on 2026-11-02, saved in an
older layout with no `metadata`; they can't be live now. The proofs come from a synchronizer with the holdings witness
endpoint, by default ours at `https://synchronizer.don.pateldhvani.com`
([../synchronizer/](../synchronizer/README.md#public-endpoints)); `--sync URL`
names another. The hosted synchronizer has no such endpoint.

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
with a GPU search for the game's geodes running alongside (load average
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
items took 21 s in one run and 43 s in another while the GPU search used
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
| 100 items from an old copy of our test player's inventory | 91 skipped as spent at block 12098387, 9 proved |
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
- is worth `value` by the Bitcraft app's own rules (`ItemValue`):

  | Kind | Classes | Value |
  |---|---|---|
  | machine (`LeveledValue`) | Quarry, MiningDrill, Pumpjack | its table's value at its `level`, 1 to 10 |
  | bucket (`BucketValue`) | the six buckets, BarrelOfOil | base + per-unit value × `n` |
  | axe (`AxeValue`) | WoodAxe, StoneAxe, IronAxe | full value × `durability` / 100 |
  | anything else (`FixedValue`) | gems, materials, Stick, Coal, Bit | its table's value |

and their values add up to at least `threshold` (`ItemsTotal`, one step per
item; `SetInsert` of each object's tag, which stays private here, requires
it to be new, so no object counts twice). `state_header` is the state's
header record, whose hash is the synchronizer's `state_root`. `recipient` is
the address the bounty pays, as 4 limbs of 40 bits, made public so a copied
proof pays the same address.

The rules are those of the app's Total Inventory Value
(`apps/bitcraft/src-tauri/src/metrics.rs`, `object_value`), and bounty.rs
reads the values from the app's own `inventory-values.json` at the game
commit holdings is built from. Values count in **centi-BIT** (1/100 BIT): a
Stick is 0.5 BIT, a MiningDrill at level 1 67.5 BIT and an IronAxe at
durability 37 3.33 BIT, all whole numbers of centi-BIT. Each class is in
exactly one of four tables, sets of hashes of the class with its values,
written into the predicate, so each item has one value and the predicate's
hash commits to every value. Pocketknife and Geode are worth nothing and
can't be claimed.

```bash
holdings bounty-info --threshold 50000000 --pexe ~/.bitcraft/actions/bitcraft.pexe
```

prints the `BountyClaim` predicate's hash, the threshold in centi-BIT packed
as the contract reads it (and a recipient's, with `--recipient`), and each
class's rule; with `--pexe` it checks the 27 class hashes against the plugin.

```bash
holdings bounty-pod --block BLOCK \
  --recipient 0xADDRESS --threshold 50000000 --out claim.json ITEM.dobj...
```

proves the claim as a pod's only public statement, against the state at
execution block `BLOCK` (a block whose root the contract holds:
`rootBlock(root)`). It needs a synchronizer with the witness endpoint
([../synchronizer/](../synchronizer/README.md)), which gives each item's
creation proof and nullifier absence proof at that block; the hosted
synchronizer has none. It skips files that aren't Bitcraft item files, and,
with a note, items spent or not yet created there and items worth nothing, keeps one file per object (copies share a
tag), and then claims only the fewest items that reach the threshold, most
valuable first; `--all` claims every item instead. It prints the claim by
class, and refuses a total below the threshold.

`--mock` checks every operation of the same claim with pod2's mock prover,
without proving it: a dry run that shows what a claim would count. It writes
no pod, since a mock pod can't be wrapped, so it takes no `--out`.

A claim's cost grows with its item count, not its value, which is why
`bounty-pod` takes the fewest, most valuable items that reach the
threshold. Where the time goes, timed on 2026-11-10 (M3 Pro) in dry runs at
block 12103547 over a Mac's 3,560 item files:

| Items | building the statements | pod2's planner | mock proving | writing the mock pod |
|---:|---:|---:|---:|---:|
| 150 | 1.5 s | 0.1 s | 0.2 s | 5.5 s, 66 MB |
| 300 | 8.4 s | 2.7 s | 0.5 s | 29.5 s, 203 MB |
| 500 | 27.9 s | 4.4 s | 1.2 s | 118.5 s, 556 MB |

- **The mock pod** records every statement, each `SetInsert` with the whole
  set of tags so far, so its JSON grows with the square of the items. No one
  can use it, so since 2026-11-10 a dry run doesn't write it: the 500-item
  dry run went from 195 s to 95 s, of which about 45 s is reading all the
  files and fetching their witness.
- **Building** is quadratic too, in pod2: `MultiPodBuilder` finds each new
  statement's index by scanning every earlier one (`stmt_index`), and a claim
  makes about 20 statements per item. An index there would make it linear
  without changing any proof; at 2,000 items it would save several minutes.
- **The planner** (`MultiPodBuilder::solve`, a dynamic program over a few
  orderings) is the smallest part. The 2026-11-09 figures (100 items planned
  in 6 s, 300 in 52 s) counted the mock pod's writing as planning.
- **Proving for real** is linear: about 3 items per pod and 14 s per pod, so
  about 4.5 s per item. 61 items took 264 s (below); 2,000 cheap items would
  take about 2.5 hours (an extrapolation). Only a predicate with fewer
  statements per item, and a new deployment, would change that.

On 2026-11-09 at 17:18 UTC, given all 4,643 of our test player's item
files, it found 4,407 live at block 12098857, worth 9,367,903.69 BIT, and for
a threshold of 7,000,000 BIT claimed 61 of them, 6 Emeralds and 55
Sapphires: 1,165 statements in 19 pods, proved in 264 s (315 s with reading
the files and fetching their witness; 4.1 GB; the GPU search running
alongside).

The app's rules, reimplemented in Python over our player's 5,036 live item
files, gave exactly the 9,447,022.73 BIT its dashboard showed (the app's
Total Inventory Value). A dry run of the
claim at the finalized block 12098606, against the same files, counted 4,831
items worth 9,356,867.23 BIT, the same values class by class, and skipped
169 whose current state was created after that block: a MiningDrill
upgraded to level 9 and a BucketOfStone filled since, among others. A claim
counts the item states that exist at its root, so an inventory that keeps
changing needs its files from that root's time, or a pause until the next
posted root.

On 2026-11-09 (M3 Pro, the GPU search running alongside) 19 real items of every
kind, live at block 12098545, proved at a threshold of exactly their value,
2,487,006 BIT, in 102 s and 4.1 GB: 397 statements in 7 pods. A Python
reimplementation of the app's rules, run over the same item files, gave the
same total. The mock-prover tests check the app's own cases (a Quarry at
level 3 is 140 BIT, a BucketOfIron with 10 units 55 BIT, an IronAxe at
durability 50 4.5 BIT), a claim with every kind, and that a gem at another
gem's value, a machine at another level's value, a bucket with more
contents, an unworn axe, or a class valued by another table's rule is
refused.

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
