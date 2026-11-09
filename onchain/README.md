# onchain: holdings claims and a bounty in a contract

Verifying `holdings` proofs on Ethereum: one item's `Holds` claim
([src/HoldsCheck.sol](src/HoldsCheck.sol)), and a bounty that pays its whole
token balance to whoever first proves they hold unspent gems worth at least a
threshold of BIT ([src/BountyEscrow.sol](src/BountyEscrow.sol)).

The proof is wrapped twice. pod2's MainPod proof is plonky2 over Goldilocks,
which is far too expensive to check in the EVM. pod2-onchain first proves,
in plonky2 with Poseidon over BN254, that the MainPod proof verifies; then a
gnark Groth16 circuit proves that this second proof verifies, and gnark
generates a Solidity verifier for it.

## The pipeline

1. `holdings onchain-pod --out pod.json ITEM.dobj` proves one item's
   `Holds(state_root, type, nullifier, tag)` as the pod's only public
   statement. A contract reads the pod's one statement, so this pod names no
   player.
2. `wrap pod.json plonky2-proof/` (pod2-onchain at `d3ab486`, branch
   `fixed-verifier-data`) wraps it. The wrapper circuit verifies the MainPod
   proof against the MainPod circuit (a constant), fixes the verifier-data set
   to pod2's default, rebuilds the statement from its public inputs and
   checks that it is the pod's statement. Its 24 public inputs are the
   predicate's hash and each argument's value, 4 Goldilocks limbs each.
3. `go run cli/main.go -t -i plonky2-proof -o groth` (once per wrapper
   circuit) compiles the Groth16 circuit, runs the setup and writes
   `Verifier.sol`. The wrapper circuit is the same for every pod with one
   public statement, since the predicate's hash is a public input, so this
   one setup serves `Holds` and `BountyClaim` alike. `go run cli/main.go -p -i plonky2-proof -o groth` proves,
   and writes `solidity_calldata.json`: the verifier's arguments.
4. [src/HoldsCheck.sol](src/HoldsCheck.sol) takes those arguments. It checks
   that the claim is a `Holds` claim, that its state root is one the owner
   posted, and that the Groth16 proof verifies, and returns the item's class,
   nullifier and tag.

A value packs into the 32 bytes the synchronizer and `holdings` print, limb 3
first, so the owner posts the synchronizer's `state_root` as a `bytes32`
unchanged.

## Measured (2026-11-09, Mac M3 Pro)

| Step | Time | Peak memory | Output |
|---|---:|---:|---|
| `holdings onchain-pod`, one Emerald | 14.2 s | 3.4 GB | 222 kB pod |
| `wrap` (wrapper circuit 2^13 rows) | 11.7 s, of which 7.7 s proving | 1.6 GB | 617 kB proof JSON |
| Groth16 circuit compile | 26 s | | 5,566,502 constraints, 24 public inputs, 322 MB R1CS |
| Groth16 setup (once per circuit) | 552 s | 13.7 GB | 1.36 GB proving key, 1.3 kB verifying key, 45 kB `Verifier.sol` |
| Groth16 proof | 16.7 s, after 107 s to load the proving key | 9.8 GB | 256-byte proof |
| gnark's own verification | 1.5 ms | | |

The Groth16 rows are from the fixed circuit (below), measured on an idle
machine. The first measurements, made while other proofs ran, were about 17
minutes for the setup and 36.6 s plus 236 s for the proof.

On-chain (Foundry, `forge test -vv`):

| Call | Gas |
|---|---:|
| `Verifier.verifyProof` | 543,110 |
| `HoldsCheck.check` (input checks, root lookup, verify) | 547,738 |

The 24 public inputs cost about a quarter of that: each is a scalar
multiplication in the verifier. A bounty statement with fewer or packed
arguments would cost less.

Loading the proving key is most of the prover's time. A prover that stays
running, as pod2-onchain's FFI `init` does, pays it once. gnark can also
read the key without re-checking its points, which should be much faster;
not tried.

The tests check that `check` returns the Emerald's class, and refuses a root
the owner did not post, a changed claim (one limb of the type hash), an input
at or above the Goldilocks modulus, and a root posted by anyone but the owner.

## The verifier was forgeable until 2026-11-09

The first `Verifier.sol` here (#2) came from pod2-onchain's Groth16 circuit as
it was, gnark-plonky2-verifier's `ExampleVerifierCircuit`. That circuit takes
the wrapper's verifier data (its constants, its permutation and their
digest) as a secret input, so its Groth16 key accepted a proof of any plonky2
circuit shaped like the wrapper. A look-alike wrapper that skips the MainPod
check could have proved any 24 public inputs: any class, any state root.

pod2-onchain's branch `fixed-verifier-data` compiles the verifier data into
the circuit as constants (11,301 secret inputs instead of 11,318). The setup
was re-run, and `src/Verifier.sol` and the fixture here come from the new one.
The new verifier rejects the proof made under the old setup
(`CommitmentInvalid`), and accepts the new proof for the same 543,110 gas.

## The bounty: `BountyEscrow`

A claim is a wrapped `BountyClaim(state_header, recipient, threshold)`
([../holdings/src/bounty.podlang](../holdings/src/bounty.podlang)): the
prover holds gems, each in the created array of `state_header` and unspent
there (its nullifier is absent from the nullifier set), each a different
object, worth at least `threshold` BIT together. The state header's hash is
the synchronizer's `state_root`. `holdings bounty-pod` proves it, and
`holdings bounty-info` prints what the contract is deployed with.

`claim(proof, commitments, commitmentPok, input)` checks, before the
verifier:

- no one has been paid, and the deadline has not passed;
- every input is below the Goldilocks modulus, so each claim has one
  encoding;
- input 0..3 is the `BountyClaim` predicate's hash (fixed at deployment; it
  commits to the gem table, so a claim with other values doesn't match);
- input 4..7 is a state root the poster posted;
- input 12..15 is the threshold, as a pod2 integer (fixed at deployment);
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

The tests (`test/BountyEscrow.t.sol`, 17, with a mock verifier and token)
cover a paid claim; the recipient named by the proof, not the sender, being
paid; one claim only; an invalid proof; a root not posted; another predicate;
a lower threshold; a non-zero unused argument; a non-canonical input; a
recipient limb over 40 bits; the zero recipient; nothing to pay; a claim after
the deadline; roots posted by anyone but the poster, the owner included;
changing the poster; and the refund, only to the owner, only after the
deadline, and not after a claim.

## Run the tests

```bash
cd onchain && forge test -vv
```

`test/fixtures/emerald_calldata.json` is a real Emerald's proof, and
`src/Verifier.sol` the verifier from the setup it was proved with.

## Deploy on Sepolia

The poster's address comes from `install-poster.sh` on the synchronizer's box
([../synchronizer/](../synchronizer/README.md#deploy)); the predicate and the
packed threshold from `holdings bounty-info --threshold 50000000`. `OWNER` is
a Foundry keystore account (`cast wallet import`), and `SEPOLIA` an RPC URL.

```bash
USDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238   # Circle's USDC on Sepolia, 6 decimals (read on-chain 2026-11-09)
PREDICATE=0xedb50b5197cf5b7b647a34e27ff456c056620794a465191de635a9695f315f7f
THRESHOLD=0xffffffff00000000000000000000000000000000000000000000000002faf080   # 50,000,000 BIT
DEADLINE=$(date -d '+30 days' +%s)   # macOS: date -v+30d +%s
forge create src/Verifier.sol:Verifier --rpc-url $SEPOLIA --account OWNER --broadcast
forge create src/BountyEscrow.sol:BountyEscrow --rpc-url $SEPOLIA --account OWNER --broadcast \
  --constructor-args $VERIFIER $USDC $PREDICATE $THRESHOLD $DEADLINE $POSTER
cast send $USDC 'transfer(address,uint256)' $BOUNTY 100000000 --rpc-url $SEPOLIA --account OWNER   # 100 USDC
```

Sizes (`forge build --sizes`): `Verifier` 9,355 bytes, `BountyEscrow`
4,182 bytes, both well under the 24,576-byte limit.

## Not for real money yet

- **The Groth16 setup here is one machine's.** Whoever ran it can forge
  proofs for this verifier. For a bounty worth more than test tokens, run a
  multi-party setup, or use gnark's PLONK, whose setup is universal.
- **The poster is trusted.** A made-up state root can commit to made-up gems,
  so whoever holds the poster's key can take the prize. Our synchronizer
  derives roots from the chain and checks them against the hosted one before
  posting, but the contract can't tell.
- **A claim proves gems held at one posted root**, up to an hour old, not
  now. Gems held at a posted root can be spent afterwards and still win.
