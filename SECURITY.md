# Security

The bounty runs on Sepolia, for test USDC. Its proofs, contracts and
synchronizer patch are meant to be sound all the same, and a hole in them
matters to anyone building on this code.

## Reporting a problem

Report privately through GitHub: the repository's **Security** tab, then
**Report a vulnerability**. Please include what you found, how to reproduce
it, and which part it is in:

- the predicates (`holdings/src/*.podlang`) or the prover and verifier
  (`holdings/`);
- the contracts (`onchain/src/`);
- the synchronizer's witness endpoint (`synchronizer/patches/`);
- the wrapper and its Groth16 circuit, which live in
  [pod2-onchain](https://github.com/dhvanipa/pod2-onchain);
- the bounty page (`site/`).

## Known limits

These are part of the design today, documented in
[onchain/README.md](onchain/README.md#not-for-real-money-yet), and not
vulnerabilities:

- **The Groth16 setup was run by one machine.** Whoever ran it could forge
  proofs for `onchain/src/Verifier.sol`.
- **The root poster is trusted.** The contract accepts the state roots its
  poster posts, and a made-up root can commit to made-up items.
- **A claim is about one posted state.** Items held at that state can be
  spent afterwards and the claim still pays.
- **The synchronizer that serves a prover's witness learns which items it was
  asked about.** Nullifiers stay private in the proof, not from the server
  that is asked for them.
