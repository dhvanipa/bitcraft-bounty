// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {BountyEscrow} from "../src/BountyEscrow.sol";
import {IGroth16Verifier} from "../src/HoldsCheck.sol";
import {Verifier} from "../src/Verifier.sol";
import {MockToken} from "./BountyEscrow.t.sol";

/// The Foundry cheatcodes these tests use.
interface Vm {
    function readFile(string calldata path) external view returns (string memory);
    function parseJsonUintArray(string calldata json, string calldata key)
        external
        pure
        returns (uint256[] memory);
    function expectRevert(bytes calldata message) external;
    function expectRevert() external;
    function prank(address sender) external;
    function warp(uint256 timestamp) external;
}

/// A real bounty claim: 19 items of every kind the Bitcraft app values (3
/// Emeralds, 2 Sapphires, 2 Amethysts, 2 Quartz, Wood, 4 buckets with their
/// contents, 2 MiningDrills by level, 3 IronAxes by durability), worth
/// 2,487,006 BIT by the app's rules, unspent at state root ROOT, for
/// RECIPIENT, proved by `holdings bounty-pod` with that exact threshold,
/// wrapped by pod2-onchain's `wrap` and proved in Groth16 by its Go prover
/// (test/fixtures/bounty_calldata.json), checked by the real verifier.
contract BountyEscrowProofTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    event log_named_uint(string key, uint256 val);

    /// `holdings bounty-info --threshold 2487006` prints these: thresholds are
    /// in centi-BIT.
    bytes32 internal constant PREDICATE = 0xaae74e677f89c82dc523ce1850a26d075d457f85bedbdf35005f3918526a772a;
    bytes32 internal constant THRESHOLD = 0xffffffff0000000000000000000000000000000000000000000000000ed2deb8;
    bytes32 internal constant THRESHOLD_50M = 0xffffffff0000000000000000000000000000000000000001000000002a05f200;
    /// The synchronizer's state root at Sepolia block 12098545, where the claim
    /// was proved.
    bytes32 internal constant ROOT = 0x5c6b1fe2caf14ed7746c657c67a3844d99bbbd94472faf445ffeb0521874f706;
    uint64 internal constant ROOT_BLOCK = 12_098_545;
    address internal constant RECIPIENT = 0xa1B2c3d4E5f60718293a4B5C6d7E8F9012345678;
    address internal constant POSTER = address(0x9057);
    uint256 internal constant DEADLINE = 1_800_000_000;
    uint256 internal constant PRIZE = 100e6;

    Verifier internal verifier;
    MockToken internal token;
    uint256[8] internal proof;
    uint256[2] internal commitments;
    uint256[2] internal pok;
    uint256[24] internal input;

    function setUp() public {
        string memory json = vm.readFile("test/fixtures/bounty_calldata.json");
        uint256[] memory p = vm.parseJsonUintArray(json, ".proof");
        for (uint256 i = 0; i < 8; i++) proof[i] = p[i];
        uint256[] memory c = vm.parseJsonUintArray(json, ".commitments");
        for (uint256 i = 0; i < 2; i++) commitments[i] = c[i];
        uint256[] memory k = vm.parseJsonUintArray(json, ".commitmentPok");
        for (uint256 i = 0; i < 2; i++) pok[i] = k[i];
        uint256[] memory x = vm.parseJsonUintArray(json, ".input");
        require(x.length == 24, "expected 24 public inputs");
        for (uint256 i = 0; i < 24; i++) input[i] = x[i];

        verifier = new Verifier();
        token = new MockToken();
        vm.warp(DEADLINE - 30 days);
    }

    function escrow(bytes32 threshold) internal returns (BountyEscrow e) {
        e = new BountyEscrow(IGroth16Verifier(address(verifier)), token, PREDICATE, threshold, DEADLINE, POSTER);
        token.mint(address(e), PRIZE);
        vm.prank(POSTER);
        e.postRoot(ROOT, ROOT_BLOCK);
    }

    function pack(uint256 at) internal view returns (bytes32) {
        return bytes32((input[at + 3] << 192) | (input[at + 2] << 128) | (input[at + 1] << 64) | input[at]);
    }

    function test_the_inputs_are_the_claim() public view {
        require(pack(0) == PREDICATE, "predicate");
        require(pack(4) == ROOT, "state root");
        require(pack(12) == THRESHOLD, "threshold");
        uint256 recipient = input[8] | (input[9] << 40) | (input[10] << 80) | (input[11] << 120);
        require(address(uint160(recipient)) == RECIPIENT, "recipient");
        for (uint256 i = 16; i < 24; i++) require(input[i] == 0, "unused argument");
    }

    function test_a_real_claim_pays_the_recipient_whoever_sends_it() public {
        BountyEscrow e = escrow(THRESHOLD);
        vm.prank(address(0xCAFE));
        uint256 before = gasleft();
        e.claim(proof, commitments, pok, input);
        emit log_named_uint("claim gas", before - gasleft());
        require(token.balanceOf(RECIPIENT) == PRIZE, "recipient not paid");
        require(e.winner() == RECIPIENT, "winner not recorded");
    }

    function test_a_real_claim_needs_its_root_posted() public {
        BountyEscrow e =
            new BountyEscrow(IGroth16Verifier(address(verifier)), token, PREDICATE, THRESHOLD, DEADLINE, POSTER);
        token.mint(address(e), PRIZE);
        vm.expectRevert(bytes("state root not posted"));
        e.claim(proof, commitments, pok, input);
    }

    function test_a_real_claim_fails_once_its_root_is_24_hours_old() public {
        BountyEscrow e = escrow(THRESHOLD);
        vm.warp(block.timestamp + 24 hours + 1);
        vm.expectRevert(bytes("state root older than 24 hours"));
        e.claim(proof, commitments, pok, input);
    }

    function test_a_50m_bounty_refuses_a_2_5m_claim() public {
        BountyEscrow e = escrow(THRESHOLD_50M);
        vm.expectRevert(bytes("wrong threshold"));
        e.claim(proof, commitments, pok, input);
    }

    function test_another_recipient_fails_the_proof() public {
        BountyEscrow e = escrow(THRESHOLD);
        input[8] ^= 1; // still a 40-bit limb, so only the verifier can refuse it
        vm.expectRevert();
        e.claim(proof, commitments, pok, input);
        require(token.balanceOf(address(e)) == PRIZE, "paid on a changed claim");
    }
}
