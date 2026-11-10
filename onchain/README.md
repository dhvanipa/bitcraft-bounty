# onchain: holdings claims and a bounty in a contract

Verifying `holdings` proofs on Ethereum: one item's `Holds` claim
([src/HoldsCheck.sol](src/HoldsCheck.sol)), and a bounty that pays its whole
token balance to whoever first proves they hold unspent Bitcraft items worth
at least a threshold of BIT, as the Bitcraft app values them, at a state root
posted in the last 24 hours ([src/BountyEscrow.sol](src/BountyEscrow.sol)).

The proof is wrapped twice. pod2's MainPod proof is plonky2 over Goldilocks,
which is far too expensive to check in the EVM. pod2-onchain first proves,
in plonky2 with Poseidon over BN254, that the MainPod proof verifies; then a
gnark Groth16 circuit proves that this second proof verifies, and gnark
generates a Solidity verifier for it.

## The pipeline

1. `holdings onchain-pod --sync URL --out pod.json ITEM.dobj` proves one
   item's `Holds(state_root, type, tag)`, live at the synchronizer's state, as
   the pod's only public statement. A contract reads the pod's one statement,
   so this pod names no player.
2. `wrap pod.json plonky2-proof/`
   ([dhvanipa/pod2-onchain](https://github.com/dhvanipa/pod2-onchain) at
   `de77b6a`, its `main`) wraps it. The wrapper circuit verifies the MainPod
   proof against the MainPod circuit (a constant), fixes the verifier-data set
   to pod2's default, rebuilds the statement from its public inputs and
   checks that it is the pod's statement. Its 24 public inputs are the
   predicate's hash and each argument's value, 4 Goldilocks limbs each.
3. `go run cli/main.go -t -i plonky2-proof -o groth` (once per wrapper
   circuit) compiles the Groth16 circuit, runs the setup and writes
   `Verifier.sol`. The wrapper circuit is the same for every pod with one
   public statement, since the predicate's hash is a public input, so this
   one setup serves `Holds` and `BountyClaim` alike.
   `go run cli/main.go -p -i plonky2-proof -o groth` proves, and writes
   `solidity_calldata.json`: the verifier's arguments. To prove for the
   verifier here, use [its setup](#the-groth16-setup), not a new one.
4. [src/HoldsCheck.sol](src/HoldsCheck.sol) takes those arguments. It checks
   that the claim is a `Holds` claim, that its state root is one the owner
   posted, that the unused arguments are zero, and that the Groth16 proof
   verifies, and returns the item's class and tag. The item was created and
   is unspent at that root; its nullifier stays private.

A value packs into the 32 bytes the synchronizer and `holdings` print, limb 3
first, so the owner posts the synchronizer's `state_root` as a `bytes32`
unchanged.

## Measured (2026-11-09, Mac M3 Pro)

| Step | Time | Peak memory | Output |
|---|---:|---:|---|
| `holdings onchain-pod`, one Emerald | 7.9 s | 3.4 GB | 223 kB pod |
| `holdings bounty-pod`, three Emeralds (the gem-only claim) | 7.6 s | 3.4 GB | 232 kB pod |
| `holdings bounty-pod`, 19 items of every kind | 102 s, 397 statements in 7 pods | 4.1 GB | |
| `wrap` (wrapper circuit 2^13 rows) | 14.3 to 14.5 s; 11.7 s on a quieter Mac, of which 7.7 s proving | 1.6 GB | 617 kB proof JSON |
| Groth16 circuit compile | 26 s | | 5,566,502 constraints, 24 public inputs, 322 MB R1CS |
| Groth16 setup (once per circuit) | 552 s | 13.7 GB | 1.36 GB proving key, 1.3 kB verifying key, 45 kB `Verifier.sol` |
| Groth16 proof | 16.5 to 16.7 s, after 105 to 122 s to load the proving key | 9.8 GB | 256-byte proof |
| gnark's own verification | 1.5 ms | | |

The Groth16 rows are from the fixed circuit (below). The setup was measured
on an idle machine; the `holdings` rows and the later proofs with a GPU
search for the game's geodes running alongside (load average about 15). The first
measurements, made while other proofs ran, were about 17 minutes for the
setup and 36.6 s plus 236 s for the proof.

On-chain (Foundry, `forge test -vv`):

| Call | Gas |
|---|---:|
| `Verifier.verifyProof` | 543,110 |
| `HoldsCheck.check` (input checks, root lookup, verify) | 548,057 |
| `BountyEscrow.claim`, real proof, paying a mock token | 597,422 |
| `BountyEscrow.claim` on Sepolia, paying Circle's USDC (2026-11-10, below) | 739,086 |

The 24 public inputs cost about a quarter of that: each is a scalar
multiplication in the verifier. A bounty statement with fewer or packed
arguments would cost less.

Loading the proving key is most of the prover's time. A prover that stays
running, as pod2-onchain's FFI `init` does, pays it once. gnark can also
read the key without re-checking its points, which should be much faster;
not tried.

The tests check that `check` returns the Emerald's class, and refuses a root
the owner did not post, a changed claim (one limb of the type hash), a
non-zero unused argument, an input at or above the Goldilocks modulus, and a
root posted by anyone but the owner.

## The verifier was forgeable until 2026-11-09

The first `Verifier.sol` here (#2) came from pod2-onchain's Groth16 circuit as
it was, gnark-plonky2-verifier's `ExampleVerifierCircuit`. That circuit takes
the wrapper's verifier data (its constants, its permutation and their
digest) as a secret input, so its Groth16 key accepted a proof of any plonky2
circuit shaped like the wrapper. A look-alike wrapper that skips the MainPod
check could have proved any 24 public inputs: any class, any state root.

pod2-onchain's `fixed-verifier-data` (merged into dhvanipa/pod2-onchain's
`main` as `de77b6a`) compiles the verifier data into
the circuit as constants (11,301 secret inputs instead of 11,318). The setup
was re-run, and `src/Verifier.sol` and the fixture here come from the new one.
The new verifier rejects the proof made under the old setup
(`CommitmentInvalid`), and accepts the new proof for the same 543,110 gas.

## The bounty: `BountyEscrow`

A claim is a wrapped `BountyClaim(state_header, recipient, threshold)`
([../holdings/src/bounty.podlang](../holdings/src/bounty.podlang)): the
prover holds items, each in the created array of `state_header` and unspent
there (its nullifier is absent from the nullifier set), each a different
object, worth at least `threshold` together by the Bitcraft app's rules
(machines by level, buckets by contents, axes by durability, everything else
by class; see [../holdings/](../holdings/README.md#a-bounty-claim)). Values
and the threshold are in centi-BIT (1/100 BIT), so 50,000,000 BIT is
5,000,000,000. The state header's hash is
the synchronizer's `state_root`. `holdings bounty-pod` proves it, and
`holdings bounty-info` prints what the contract is deployed with.

`claim(proof, commitments, commitmentPok, input)` checks, before the
verifier:

- no one has been paid, and the deadline has not passed;
- every input is below the Goldilocks modulus, so each claim has one
  encoding;
- input 0..3 is the `BountyClaim` predicate's hash (fixed at deployment; it
  commits to every class's value and to holdings.podlang, which it imports,
  so a claim with other values or another `LiveObject` doesn't match);
- input 4..7 is a state root the poster posted, at most 24 hours ago
  (`ROOT_LIFETIME`): a claim shows items held within the last day, not at
  any time since deployment. An item made a week ago still counts, as long
  as it is unspent at that recent root;
- input 12..15 is the threshold in centi-BIT, as a pod2 integer (fixed at
  deployment);
- input 16..23 (the unused arguments) is zero;
- input 8..11 is the recipient, 4 limbs of 40 bits, least significant first,
  and not zero;
- the contract holds something to pay.

Then it pays its whole balance to the recipient. The recipient is part of the
proved statement, so a proof copied from the mempool pays the same address,
whoever sends it.

Roles: the owner deploys, funds, can change the poster, and can take the
tokens back after the deadline if no one claimed. The poster posts state
roots with their block numbers (`postRoot(root, block)`); a prover asks the
synchronizer for its witness at that block (`rootBlock(root)`). The poster is
a key on the synchronizer's box
([../synchronizer/](../synchronizer/README.md)), which posts the root at
Sepolia's finalized block each hour.

`test/BountyEscrowProof.t.sol` runs a real claim through the real verifier:
19 items of every kind (3 Emeralds, 2 Sapphires, 2 Amethysts, 2 Quartz, Wood,
4 buckets, 2 MiningDrills, 3 IronAxes) live at Sepolia block 12098545, worth
exactly 2,487,006 BIT, proved on 2026-11-09 against our synchronizer at that
threshold. Its 6 tests check that the 24 inputs are the claim (predicate,
root, recipient, threshold, zero unused arguments), that the claim pays the
recipient whoever sends it, that it needs its root posted and fails once the
root is over 24 hours old, that a 50,000,000 BIT bounty refuses it, and that
changing the recipient fails the proof.

The tests (`test/BountyEscrow.t.sol`, 19, with a mock verifier and token)
cover a paid claim; the recipient named by the proof, not the sender, being
paid; one claim only; an invalid proof; a root not posted; a root exactly 24
hours old (taken) and one a second older (refused); another predicate;
a lower threshold; a non-zero unused argument; a non-canonical input; a
recipient limb over 40 bits; the zero recipient; nothing to pay; a claim after
the deadline; roots posted by anyone but the poster, the owner included;
changing the poster; and the refund, only to the owner, only after the
deadline, and not after a claim.

## The Groth16 setup

`src/Verifier.sol` holds the verifying key of one setup, run on 2026-11-09.
A proof made with any other proving key fails it, so every claim needs that
setup's proving key: the release
[groth16-setup-1](https://github.com/dhvanipa/bitcraft-leaderboard/releases/tag/groth16-setup-1)
has `r1cs` (322 MB), `proving.key` (1.36 GB) and `verifying.key`, which
pod2-onchain's prover reads from its output folder.
[groth16-setup.sha256](groth16-setup.sha256) has their SHA-256 sums:

```bash
mkdir groth && cd groth
for f in r1cs proving.key verifying.key; do
  curl -fLO https://github.com/dhvanipa/bitcraft-leaderboard/releases/download/groth16-setup-1/$f
done
shasum -a 256 -c ../onchain/groth16-setup.sha256
```

Running the setup again (`-t`) makes a new key pair, and a new
`Verifier.sol` to deploy with it: the fixtures here and any deployed
verifier would no longer accept its proofs.

## Run the tests

```bash
cd onchain && forge test -vv
```

`test/fixtures/emerald_calldata.json` is a real Emerald's `Holds` proof and
`test/fixtures/bounty_calldata.json` a real 19-item `BountyClaim` live at
block 12098545, and `src/Verifier.sol` the verifier from the setup they were
proved with. 32 tests pass on 2026-11-09.

## Deploy on Sepolia

The poster's address comes from `install-poster.sh` on the synchronizer's box
([../synchronizer/](../synchronizer/README.md#deploy)); the predicate and the
packed threshold from `holdings bounty-info --threshold 50000000`. `OWNER` is
a Foundry keystore account (`cast wallet import`), and `SEPOLIA` an RPC URL.

```bash
USDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238   # Circle's USDC on Sepolia, 6 decimals (read on-chain 2026-11-09)
PREDICATE=0xaae74e677f89c82dc523ce1850a26d075d457f85bedbdf35005f3918526a772a
THRESHOLD=0xffffffff0000000000000000000000000000000000000001000000002a05f200   # 50,000,000 BIT = 5,000,000,000 centi-BIT
DEADLINE=$(date -d '+30 days' +%s)   # macOS: date -v+30d +%s
forge create src/Verifier.sol:Verifier --rpc-url $SEPOLIA --account OWNER --broadcast
forge create src/BountyEscrow.sol:BountyEscrow --rpc-url $SEPOLIA --account OWNER --broadcast \
  --constructor-args $VERIFIER $USDC $PREDICATE $THRESHOLD $DEADLINE $POSTER
cast send $USDC 'transfer(address,uint256)' $BOUNTY 100000000 --rpc-url $SEPOLIA --account OWNER   # 100 USDC
```

Then start the poster with the bounty's address
([../synchronizer/](../synchronizer/README.md#deploy)) and set `bounty` in
[../site/config.js](../site/config.js), so the bounty page shows it.

Sizes (`forge build --sizes`): `Verifier` 9,355 bytes, `BountyEscrow`
4,389 bytes, both well under the 24,576-byte limit.

### Deployed (2026-11-10)

| | |
|---|---|
| `BountyEscrow` | [`0x91d3219a384ab57262287fFc9d11b96cE7d1d434`](https://sepolia.etherscan.io/address/0x91d3219a384ab57262287fFc9d11b96cE7d1d434) |
| `Verifier` | [`0xb5cA82616B29A68DacA8b475A40871b0aC0cFBDF`](https://sepolia.etherscan.io/address/0xb5cA82616B29A68DacA8b475A40871b0aC0cFBDF) |
| token | Circle's USDC, `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238` |
| threshold | 50,000,000 BIT (`0xffffffff…0001000000002a05f200`) |
| claim predicate | `0xaae74e677f89c82dc523ce1850a26d075d457f85bedbdf35005f3918526a772a` |
| deadline | 1799496000, 2027-01-09 12:00 UTC |
| poster | `0xfa9D2103fE14B4a64f90CF2272Fc079A6A44BcBc`, on the synchronizer's box |
| owner | `0xE0ae70caBb529336e25FA7a1f036b77ad0089d2a` |

The deployed verifier's runtime code is byte for byte this repository's
`Verifier.sol`, and every setting above was read back from the contract.
Deploying took far more gas than code deposit used to cost: 14,674,466 for
the verifier (block 12103537) and 7,108,724 for the escrow (block 12103539).
The poster's first `postRoot`, the state at block 12103484, took 240,084 gas,
against 47,855 on a local anvil chain on 2026-11-09, which suggests Sepolia
now charges much more for new state. At that day's gas price of 68 wei a post
cost about 1.6 × 10⁻¹¹ ETH; at 1 gwei, the poster's 1 ETH would last about
4,000 posts, some 170 days.

### The first claim paid on Sepolia (2026-11-10)

A test bounty, [`0x364a2F8aA95030d31CB0CB49496232a552D07ea4`](https://sepolia.etherscan.io/address/0x364a2F8aA95030d31CB0CB49496232a552D07ea4),
the same contract and verifier at a threshold of 2,000,000 BIT, funded with
1 USDC, with the root at block 12103547 posted to it from the box. Its owner
proved 3 Emeralds from a fresh clone, following the bounty page's steps:
built `holdings` and the wrapper, downloaded the published setup, ran the
dry run, proved, wrapped and made the Groth16 proof; then checked the claim
on the page and sent it from a browser wallet. The contract verified it and
paid 1 USDC to the recipient the proof named:
[`0x7829dfc2…e737e`](https://sepolia.etherscan.io/tx/0x7829dfc2fd636ace84540e8c78edc335ca92125392d4a1bd7323e8ba151e737e),
block 12103919, 739,086 gas, against 597,422 on a local chain.

## Not for real money yet

- **The Groth16 setup here is one machine's.** Whoever ran it can forge
  proofs for this verifier. For a bounty worth more than test tokens, run a
  multi-party setup, or use gnark's PLONK, whose setup is universal.
- **The poster is trusted.** A made-up state root can commit to made-up items,
  so whoever holds the poster's key can take the prize. Our synchronizer
  derives roots from the chain and checks them against the hosted one before
  posting, but the contract can't tell.
- **A claim proves items held at one posted root**, posted within the last
  24 hours, not now. Items held at that root can be spent afterwards and
  still win.
