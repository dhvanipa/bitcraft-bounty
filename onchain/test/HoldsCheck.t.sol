// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {HoldsCheck, IGroth16Verifier} from "../src/HoldsCheck.sol";
import {Verifier} from "../src/Verifier.sol";

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
}

/// A real Emerald's `Holds` claim, proved by `holdings onchain-pod`, wrapped by
/// pod2-onchain's `wrap` and proved in Groth16 by its Go prover
/// (test/fixtures/emerald_calldata.json).
contract HoldsCheckTest {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    event log_named_uint(string key, uint256 val);
    event log_named_bytes32(string key, bytes32 val);

    /// `holdings classes` prints these.
    bytes32 internal constant HOLDS = 0x374b70ce2c1ee0a79be6b06d76593599befd53170b430ca2637e7bfe1942df40;
    bytes32 internal constant EMERALD = 0x87c2c758e8d81ccb603ed25ad7b4c6d4cdea3e4ff014e9765240d9ec945fe227;
    /// The state root of the Emerald's creation proof (block 12078843), as
    /// the synchronizer reports it.
    bytes32 internal constant ROOT = 0x93c55b3783ad59842f0c9e6377bc1f417250dd1211c98111acbd0e7d530a5ddf;

    Verifier internal verifier;
    HoldsCheck internal holds;
    uint256[8] internal proof;
    uint256[2] internal commitments;
    uint256[2] internal pok;
    uint256[24] internal input;

    function setUp() public {
        string memory json = vm.readFile("test/fixtures/emerald_calldata.json");
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
        holds = new HoldsCheck(IGroth16Verifier(address(verifier)), HOLDS);
    }

    function test_verifier_accepts_the_proof() public {
        uint256 before = gasleft();
        verifier.verifyProof(proof, commitments, pok, input);
        emit log_named_uint("verifyProof gas", before - gasleft());
    }

    function test_check_reads_an_emerald() public {
        holds.postRoot(ROOT);
        uint256 before = gasleft();
        (bytes32 class, bytes32 nullifier, bytes32 tag) = holds.check(proof, commitments, pok, input);
        emit log_named_uint("check gas", before - gasleft());
        require(class == EMERALD, "not an Emerald");
        emit log_named_bytes32("nullifier", nullifier);
        emit log_named_bytes32("tag", tag);
    }

    function test_rejects_a_root_not_posted() public {
        vm.expectRevert(bytes("state root not posted"));
        holds.check(proof, commitments, pok, input);
    }

    function test_rejects_a_changed_claim() public {
        holds.postRoot(ROOT);
        // claim a different class: one limb of the type hash
        input[8] = input[8] ^ 1;
        vm.expectRevert();
        holds.check(proof, commitments, pok, input);
    }

    function test_rejects_a_non_canonical_input() public {
        holds.postRoot(ROOT);
        input[12] = input[12] + 0xFFFFFFFF00000001;
        vm.expectRevert(bytes("input is not a Goldilocks element"));
        holds.check(proof, commitments, pok, input);
    }

    function test_only_the_owner_posts_roots() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert(bytes("only the owner posts roots"));
        holds.postRoot(ROOT);
    }
}
