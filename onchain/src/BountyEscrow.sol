// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IGroth16Verifier} from "./HoldsCheck.sol";

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// Pays its whole token balance to the first proof that someone holds unspent
/// Bitcraft gems worth at least a threshold of BIT, at a game state root its
/// owner posted.
///
/// A claim is a wrapped `BountyClaim(state_root, recipient, threshold)`
/// (bitcraft-leaderboard's bounty.podlang): pod2-onchain's wrapper exposes the
/// predicate's hash and each argument's value as 24 Goldilocks limbs, 4 per
/// value. The recipient is part of the proved statement, so a copied proof
/// pays the same address.
contract BountyEscrow {
    uint256 internal constant GOLDILOCKS = 0xFFFFFFFF00000001;
    uint256 internal constant LIMB_40 = 1 << 40;

    IGroth16Verifier public immutable verifier;
    IERC20 public immutable token;
    /// Hash of the `BountyClaim` predicate, packed (`holdings bounty-info`).
    bytes32 public immutable claimPredicate;
    /// The threshold as a pod2 value, packed (`holdings bounty-info`).
    bytes32 public immutable threshold;
    /// After this time no claim is taken, and the owner can take the tokens back.
    uint256 public immutable deadline;
    /// Deploys, funds and refunds; can change the poster.
    address public immutable owner;
    /// Posts state roots, hourly, from the synchronizer's box (a key that
    /// lives there, so the owner's need not). Whoever posts roots decides
    /// which states a claim may prove gems in: a made-up root could hold
    /// made-up gems. So this key is worth the prize.
    address public poster;

    /// The execution block of each posted state root; 0 for a root not posted.
    /// A prover asks the synchronizer for its witness at this block.
    mapping(bytes32 => uint64) public rootBlock;
    /// Who was paid, once someone was.
    address public winner;

    event RootPosted(bytes32 indexed root, uint64 blockNumber);
    event Claimed(address indexed recipient, bytes32 indexed root, uint256 amount);
    event Refunded(uint256 amount);
    event PosterSet(address poster);

    constructor(
        IGroth16Verifier verifier_,
        IERC20 token_,
        bytes32 claimPredicate_,
        bytes32 threshold_,
        uint256 deadline_,
        address poster_
    ) {
        verifier = verifier_;
        token = token_;
        claimPredicate = claimPredicate_;
        threshold = threshold_;
        deadline = deadline_;
        owner = msg.sender;
        poster = poster_;
        emit PosterSet(poster_);
    }

    function setPoster(address poster_) external {
        require(msg.sender == owner, "only the owner");
        poster = poster_;
        emit PosterSet(poster_);
    }

    function postRoot(bytes32 root, uint64 blockNumber) external {
        require(msg.sender == poster, "only the poster posts roots");
        require(blockNumber != 0, "no block number");
        rootBlock[root] = blockNumber;
        emit RootPosted(root, blockNumber);
    }

    function claim(
        uint256[8] calldata proof,
        uint256[2] calldata commitments,
        uint256[2] calldata commitmentPok,
        uint256[24] calldata input
    ) external {
        require(winner == address(0), "already claimed");
        require(block.timestamp < deadline, "the bounty has ended");
        for (uint256 i = 0; i < 24; i++) {
            require(input[i] < GOLDILOCKS, "input is not a Goldilocks element");
        }
        require(pack(input, 0) == claimPredicate, "not a bounty claim");
        bytes32 root = pack(input, 4);
        require(rootBlock[root] != 0, "state root not posted");
        require(pack(input, 12) == threshold, "wrong threshold");
        for (uint256 i = 16; i < 24; i++) {
            require(input[i] == 0, "unexpected argument");
        }
        address recipient = toAddress(input, 8);
        require(recipient != address(0), "no recipient");

        // A claim before the owner funds the bounty would end it unpaid.
        uint256 amount = token.balanceOf(address(this));
        require(amount != 0, "nothing to pay");

        verifier.verifyProof(proof, commitments, commitmentPok, input);

        winner = recipient;
        require(token.transfer(recipient, amount), "transfer failed");
        emit Claimed(recipient, root, amount);
    }

    function refund() external {
        require(msg.sender == owner, "only the owner");
        require(winner == address(0), "already claimed");
        require(block.timestamp >= deadline, "the bounty is still open");
        uint256 amount = token.balanceOf(address(this));
        require(token.transfer(owner, amount), "transfer failed");
        emit Refunded(amount);
    }

    /// 4 limbs from `at`, packed limb 3 first, as the synchronizer prints a hash.
    function pack(uint256[24] calldata input, uint256 at) internal pure returns (bytes32) {
        return bytes32(
            (input[at + 3] << 192) | (input[at + 2] << 128) | (input[at + 1] << 64) | input[at]
        );
    }

    /// An address from its 4 limbs of 40 bits, least significant first.
    function toAddress(uint256[24] calldata input, uint256 at) internal pure returns (address) {
        for (uint256 i = 0; i < 4; i++) {
            require(input[at + i] < LIMB_40, "recipient limb out of range");
        }
        return address(
            uint160(input[at] | (input[at + 1] << 40) | (input[at + 2] << 80) | (input[at + 3] << 120))
        );
    }
}
