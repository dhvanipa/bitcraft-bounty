// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// The Groth16 verifier gnark generates for the pod2-onchain wrapper circuit
/// (src/Verifier.sol). It reverts unless the proof is valid for `input`.
interface IGroth16Verifier {
    function verifyProof(
        uint256[8] calldata proof,
        uint256[2] calldata commitments,
        uint256[2] calldata commitmentPok,
        uint256[24] calldata input
    ) external view;
}

/// Checks a wrapped `Holds(state_root, type, tag)` claim (bitcraft-bounty's
/// holdings.podlang) against the game state roots its owner posts: the
/// prover holds an item of class `type`, created and unspent at that root.
/// The item's nullifier stays private; `tag` is the same for every state of
/// one object, so a caller can count each object once.
///
/// The wrapper's 24 public inputs are Goldilocks field elements: the
/// statement's predicate hash, then each argument's value, 4 limbs each. A
/// value packs into the 32 bytes the synchronizer and `holdings` print, limb 3
/// first: (l3 << 192) | (l2 << 128) | (l1 << 64) | l0.
contract HoldsCheck {
    /// The Goldilocks field's modulus: each input must be below it, so each
    /// claim has one encoding.
    uint256 internal constant GOLDILOCKS = 0xFFFFFFFF00000001;

    IGroth16Verifier public immutable verifier;
    /// Hash of the `Holds` predicate, packed.
    bytes32 public immutable holdsPredicate;
    address public immutable owner;

    /// Game state roots the owner has vouched for.
    mapping(bytes32 => bool) public isRoot;

    event RootPosted(bytes32 root);

    constructor(IGroth16Verifier verifier_, bytes32 holdsPredicate_) {
        verifier = verifier_;
        holdsPredicate = holdsPredicate_;
        owner = msg.sender;
    }

    function postRoot(bytes32 root) external {
        require(msg.sender == owner, "only the owner posts roots");
        isRoot[root] = true;
        emit RootPosted(root);
    }

    /// Verifies a wrapped `Holds` proof and returns its claim: the item's
    /// class (its `Is<Class>` predicate hash) and its tag.
    function check(
        uint256[8] calldata proof,
        uint256[2] calldata commitments,
        uint256[2] calldata commitmentPok,
        uint256[24] calldata input
    ) external view returns (bytes32 class, bytes32 tag) {
        for (uint256 i = 0; i < 24; i++) {
            require(input[i] < GOLDILOCKS, "input is not a Goldilocks element");
        }
        require(pack(input, 0) == holdsPredicate, "not a Holds claim");
        require(isRoot[pack(input, 4)], "state root not posted");
        for (uint256 i = 16; i < 24; i++) {
            require(input[i] == 0, "unexpected argument");
        }
        verifier.verifyProof(proof, commitments, commitmentPok, input);
        return (pack(input, 8), pack(input, 12));
    }

    function pack(uint256[24] calldata input, uint256 at) internal pure returns (bytes32) {
        return bytes32(
            (input[at + 3] << 192) | (input[at + 2] << 128) | (input[at + 1] << 64) | input[at]
        );
    }
}
