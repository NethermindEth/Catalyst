import pytest
import requests
from web3 import Web3
import os
from dotenv import load_dotenv
import sys
from utils import *
import subprocess
import re

load_dotenv()

l2_prefunded_priv_key = os.getenv("TEST_L2_PREFUNDED_PRIVATE_KEY")
if not l2_prefunded_priv_key:
    raise Exception("Environment variable TEST_L2_PREFUNDED_PRIVATE_KEY not set")


def test_rpcs(l1_client, l2_client_node1, l2_client_node2, beacon_client):
    """Test to verify the chain IDs of L1 and L2 networks"""
    l1_chain_id = l1_client.eth.chain_id
    l2_chain_id_node1 = l2_client_node1.eth.chain_id
    l2_chain_id_node2 = l2_client_node2.eth.chain_id

    print(f"L1 Chain ID: {l1_chain_id}")
    print(f"L2 Chain ID Node 1: {l2_chain_id_node1}")
    print(f"L2 Chain ID Node 2: {l2_chain_id_node2}")

    assert l1_chain_id > 0, "L1 chain ID should be greater than 0"
    assert l2_chain_id_node1 > 0, "L2 chain ID should be greater than 0"

    assert l1_chain_id != l2_chain_id_node1, "L1 and L2 should have different chain IDs"
    assert l2_chain_id_node1 == l2_chain_id_node2, "L2 nodes should have the same chain IDs"

    spec = beacon_client.get_spec()
    slots_per_epoch = int(spec['data']['SLOTS_PER_EPOCH'])
    slot_duration = int(spec['data']['SECONDS_PER_SLOT'])
    print(f"Slot Duration: {slot_duration}")
    print(f"Slots Per Epoch: {slots_per_epoch}")
    assert slot_duration > 0, "Slot duration should be greater than 0"
    assert slots_per_epoch > 0, "Slots per epoch should be greater than 0"

def test_preconfirm_transaction(l1_client, l2_client_node1):
    account = l2_client_node1.eth.account.from_key(l2_prefunded_priv_key)
    nonce = l2_client_node1.eth.get_transaction_count(account.address)
    l2_block_number = l2_client_node1.eth.block_number

    tx_hash = send_transaction(nonce, account, '0.00005', l2_client_node1, l2_prefunded_priv_key)
    assert wait_for_tx_to_be_included(l2_client_node1, tx_hash), "Transaction should be included in L2 Node 1"

def test_p2p_preconfirmation(l2_client_node1, l2_client_node2):
    account = l2_client_node1.eth.account.from_key(l2_prefunded_priv_key)
    nonce = l2_client_node1.eth.get_transaction_count(account.address)
    l2_node_2_block_number = l2_client_node2.eth.block_number

    send_transaction(nonce, account, '0.00006', l2_client_node1, l2_prefunded_priv_key)

    assert wait_for_new_block(l2_client_node2, l2_node_2_block_number), "L2 Node 2 should have a new block after sending a transaction"

    l2_node_2_block_number_after = l2_client_node2.eth.block_number
    node_1_block_hash = l2_client_node1.eth.get_block(l2_node_2_block_number_after).hash
    node_2_block_hash = l2_client_node2.eth.get_block(l2_node_2_block_number_after).hash

    print(f"L2 Node 2 Block Number: {l2_node_2_block_number}")
    print(f"L2 Node 2 Block Number After: {l2_node_2_block_number_after}")

    assert node_2_block_hash == node_1_block_hash, "L2 Node 1 and L2 Node 2 should have the same block hash after sending a transaction"

def test_handover_transaction(l2_client_node1, l2_client_node2, beacon_client):
    wait_for_handover_window(beacon_client)

    account = l2_client_node1.eth.account.from_key(l2_prefunded_priv_key)
    nonce = l2_client_node1.eth.get_transaction_count(account.address)
    l2_node_2_block_number = l2_client_node2.eth.block_number
    print(f"L2 Node 2 Block Number: {l2_node_2_block_number}")

    tx_hash = send_transaction(nonce, account, '0.00007', l2_client_node1, l2_prefunded_priv_key)
    assert wait_for_tx_to_be_included(l2_client_node1, tx_hash), "Transaction should be included in L2 Node 1"
    tx_hash = send_transaction(nonce+1, account, '0.00008', l2_client_node2, l2_prefunded_priv_key)
    assert wait_for_tx_to_be_included(l2_client_node2, tx_hash), "Transaction should be included in L2 Node 2"

def test_forced_inclusion(l2_client_node1):
    """
    This test runs the forced inclusion toolbox docker command and prints its output.
    """
    cmd = [
        "docker", "run", "--network", "host", "--env-file", ".env", "--rm", "-it",
        "nethswitchboard/taiko-forced-inclusion-toolbox", "send"
    ]
    print("Running forced inclusion toolbox docker command...")
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        print("Forced inclusion toolbox output:")
        print(result.stdout)
        if result.stderr:
            print("Forced inclusion toolbox error output:")
            print(result.stderr)

        regex = r"hash=(0x[a-fA-F0-9]{64})"
        match = re.search(regex, result.stdout)
        assert match, "Could not find tx hash in forced inclusion toolbox output"
        forced_inclusion_tx_hash = match.group(1)
        print(f"Extracted forced inclusion tx hash: {forced_inclusion_tx_hash}")

        # Spam 41 transactions to L2 Node to at least one batch which will include the forced inclusion tx
        spam_n_txs(l2_client_node1, l2_prefunded_priv_key, 41)

        assert wait_for_tx_to_be_included(l2_client_node1, forced_inclusion_tx_hash), "Forced inclusion tx should be included in L2 Node 1"

    except subprocess.CalledProcessError as e:
        print("Error running forced inclusion toolbox docker command:")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "Forced inclusion toolbox docker command failed"

