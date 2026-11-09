# onchain: checking a holdings claim in a contract

A first try at verifying a `holdings` proof on Ethereum, toward a bounty
contract that pays whoever first proves they hold a given amount of BIT.

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
2. `wrap pod.json plonky2-proof/` (pod2-onchain, branch
   `port-pod2-331a1145`) wraps it. The wrapper circuit verifies the MainPod
   proof against the MainPod circuit (a constant), fixes the verifier-data set
   to pod2's default, rebuilds the statement from its public inputs and
   checks that it is the pod's statement. Its 24 public inputs are the
   predicate's hash and each argument's value, 4 Goldilocks limbs each.
3. `go run cli/main.go -t -i plonky2-proof -o groth` (once per wrapper
   circuit) compiles the Groth16 circuit, runs the setup and writes
   `Verifier.sol`. `go run cli/main.go -p -i plonky2-proof -o groth` proves,
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
| Groth16 circuit compile | 50 s | | 5,567,234 constraints, 24 public inputs, 322 MB R1CS |
| Groth16 setup (once per circuit) | about 17 min | about 9.3 GB (sampled) | 1.36 GB proving key, 1.3 kB verifying key, 45 kB `Verifier.sol` |
| Groth16 proof | 36.6 s, after 236 s to load the proving key | 11.2 GB | 256-byte proof |
| gnark's own verification | 3.5 ms | | |

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

## Run the test

```bash
cd onchain && forge test -vv
```

`test/fixtures/emerald_calldata.json` is a real Emerald's proof, and
`src/Verifier.sol` the verifier from the setup it was proved with.

## Not for real money yet

- **The Groth16 setup here is one machine's.** Whoever ran it can forge
  proofs for this verifier. For a bounty, run a multi-party setup, or use
  gnark's PLONK, whose setup is universal.
- **`Holds` alone is not a bounty claim.** A bounty needs one statement that
  sums the values of several items in-circuit, checks the total against the
  threshold, proves each nullifier is absent from the state root's nullifier
  set (so the contract needs nothing but the root), and names the recipient's
  address, so a copied proof pays the same person.
