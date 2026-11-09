// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {BountyEscrow, IERC20} from "../src/BountyEscrow.sol";
import {IGroth16Verifier} from "../src/HoldsCheck.sol";

/// The Foundry cheatcodes these tests use.
interface Vm {
    function expectRevert(bytes calldata message) external;
    function prank(address sender) external;
    function warp(uint256 timestamp) external;
}

/// Accepts every proof unless told to reject, so these tests check the
/// escrow's own rules. BountyEscrowProof.t.sol runs a real proof.
contract MockVerifier is IGroth16Verifier {
    bool public reject;

    function setReject(bool reject_) external {
        reject = reject_;
    }

    function verifyProof(uint256[8] calldata, uint256[2] calldata, uint256[2] calldata, uint256[24] calldata)
        external
        view
    {
        require(!reject, "invalid proof");
    }
}

contract MockToken is IERC20 {
    mapping(address => uint256) public balanceOf;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

contract BountyEscrowTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    // Each 64-bit limb of these is below the Goldilocks modulus.
    bytes32 internal constant PREDICATE = 0x0123456789abcdef0011223344556677_8899aabbccddeeff0f1e2d3c4b5a6978;
    bytes32 internal constant ROOT = 0x1111111111111111222222222222222233333333333333334444444444444444;
    /// 50,000,000 as a pod2 Int; any 4 canonical limbs serve here.
    bytes32 internal constant THRESHOLD = 0x0000000000000005000000000000000000000000000000000000000002faf080;
    address internal constant RECIPIENT = 0xa1B2c3d4E5f60718293a4B5C6d7E8F9012345678;
    uint256 internal constant DEADLINE = 1_800_000_000;
    uint256 internal constant PRIZE = 100e6;
    address internal constant POSTER = address(0x9057);

    MockVerifier internal verifier;
    MockToken internal token;
    BountyEscrow internal escrow;

    uint256[8] internal proof;
    uint256[2] internal commitments;
    uint256[2] internal pok;
    uint256[24] internal input;

    function setUp() public {
        verifier = new MockVerifier();
        token = new MockToken();
        escrow = new BountyEscrow(IGroth16Verifier(address(verifier)), token, PREDICATE, THRESHOLD, DEADLINE, POSTER);
        token.mint(address(escrow), PRIZE);
        vm.warp(DEADLINE - 30 days);

        unpack(PREDICATE, 0);
        unpack(ROOT, 4);
        uint256 r = uint160(RECIPIENT);
        for (uint256 i = 0; i < 4; i++) input[8 + i] = (r >> (40 * i)) & ((1 << 40) - 1);
        unpack(THRESHOLD, 12);
    }

    function unpack(bytes32 value, uint256 at) internal {
        for (uint256 i = 0; i < 4; i++) input[at + i] = (uint256(value) >> (64 * i)) & type(uint64).max;
    }

    function claim() internal {
        escrow.claim(proof, commitments, pok, input);
    }

    function test_a_valid_claim_pays_the_recipient() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 12_345_678);
        claim();
        require(token.balanceOf(RECIPIENT) == PRIZE, "recipient not paid");
        require(token.balanceOf(address(escrow)) == 0, "escrow kept tokens");
        require(escrow.winner() == RECIPIENT, "winner not recorded");
    }

    function test_the_recipient_is_whoever_the_proof_names() public {
        // anyone may send the transaction; the proof decides who is paid
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        vm.prank(address(0xCAFE));
        claim();
        require(token.balanceOf(RECIPIENT) == PRIZE, "recipient not paid");
        require(token.balanceOf(address(0xCAFE)) == 0, "sender paid");
    }

    function test_only_one_claim_pays() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        claim();
        token.mint(address(escrow), 1);
        vm.expectRevert(bytes("already claimed"));
        claim();
    }

    function test_rejects_an_invalid_proof() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        verifier.setReject(true);
        vm.expectRevert(bytes("invalid proof"));
        claim();
    }

    function test_accepts_a_root_up_to_24_hours_old() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        vm.warp(block.timestamp + 24 hours);
        claim();
        require(token.balanceOf(RECIPIENT) == PRIZE, "recipient not paid");
    }

    function test_rejects_a_root_older_than_24_hours() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        vm.warp(block.timestamp + 24 hours + 1);
        vm.expectRevert(bytes("state root older than 24 hours"));
        claim();
    }

    function test_rejects_a_root_not_posted() public {
        vm.expectRevert(bytes("state root not posted"));
        claim();
    }

    function test_rejects_another_predicate() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        input[1] ^= 1;
        vm.expectRevert(bytes("not a bounty claim"));
        claim();
    }

    function test_rejects_a_lower_threshold() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        input[12] -= 1;
        vm.expectRevert(bytes("wrong threshold"));
        claim();
    }

    function test_rejects_an_extra_argument() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        input[20] = 1;
        vm.expectRevert(bytes("unexpected argument"));
        claim();
    }

    function test_rejects_a_non_canonical_input() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        input[5] += 0xFFFFFFFF00000001;
        vm.expectRevert(bytes("input is not a Goldilocks element"));
        claim();
    }

    function test_rejects_a_recipient_limb_over_40_bits() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        input[11] |= 1 << 40;
        vm.expectRevert(bytes("recipient limb out of range"));
        claim();
    }

    function test_rejects_the_zero_recipient() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        for (uint256 i = 8; i < 12; i++) input[i] = 0;
        vm.expectRevert(bytes("no recipient"));
        claim();
    }

    function test_rejects_a_claim_with_nothing_to_pay() public {
        BountyEscrow empty =
            new BountyEscrow(IGroth16Verifier(address(verifier)), token, PREDICATE, THRESHOLD, DEADLINE, POSTER);
        vm.prank(POSTER);
        empty.postRoot(ROOT, 1);
        vm.expectRevert(bytes("nothing to pay"));
        empty.claim(proof, commitments, pok, input);
        require(empty.winner() == address(0), "an empty claim ended the bounty");
    }

    function test_rejects_a_claim_after_the_deadline() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        vm.warp(DEADLINE);
        vm.expectRevert(bytes("the bounty has ended"));
        claim();
    }

    function test_only_the_poster_posts_roots() public {
        // not even the owner
        vm.expectRevert(bytes("only the poster posts roots"));
        escrow.postRoot(ROOT, 1);
        vm.prank(address(0xBEEF));
        vm.expectRevert(bytes("only the poster posts roots"));
        escrow.postRoot(ROOT, 1);
    }

    function test_the_owner_changes_the_poster() public {
        vm.prank(POSTER);
        vm.expectRevert(bytes("only the owner"));
        escrow.setPoster(address(0xBEEF));
        escrow.setPoster(address(0xBEEF));
        vm.prank(POSTER);
        vm.expectRevert(bytes("only the poster posts roots"));
        escrow.postRoot(ROOT, 1);
        vm.prank(address(0xBEEF));
        escrow.postRoot(ROOT, 1);
        require(escrow.rootBlock(ROOT) == 1, "root not posted");
    }

    function test_the_owner_takes_back_an_unclaimed_prize_after_the_deadline() public {
        vm.expectRevert(bytes("the bounty is still open"));
        escrow.refund();
        vm.warp(DEADLINE);
        vm.prank(address(0xBEEF));
        vm.expectRevert(bytes("only the owner"));
        escrow.refund();
        escrow.refund();
        require(token.balanceOf(address(this)) == PRIZE, "owner not refunded");
    }

    function test_no_refund_after_a_claim() public {
        vm.prank(POSTER);
        escrow.postRoot(ROOT, 1);
        claim();
        vm.warp(DEADLINE);
        vm.expectRevert(bytes("already claimed"));
        escrow.refund();
    }
}
