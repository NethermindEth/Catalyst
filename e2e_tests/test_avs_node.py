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

l2_prefunded_priv_key_2 = os.getenv("TEST_L2_PREFUNDED_PRIVATE_KEY_2")
if not l2_prefunded_priv_key_2:
    raise Exception("Environment variable TEST_L2_PREFUNDED_PRIVATE_KEY_2 not set")

taiko_inbox_address = os.getenv("TAIKO_INBOX_ADDRESS")
if not taiko_inbox_address:
    raise Exception("Environment variable TAIKO_INBOX_ADDRESS not set")

preconf_whitelist_address = os.getenv("PRECONF_WHITELIST_ADDRESS")
if not preconf_whitelist_address:
    raise Exception("Environment variable PRECONF_WHITELIST_ADDRESS not set")

preconf_min_txs = os.getenv("PRECONF_MIN_TXS")
if preconf_min_txs is None:
    raise Exception("PRECONF_MIN_TXS is not set")
preconf_min_txs = int(preconf_min_txs)

preconf_heartbeat_ms = int(os.getenv("PRECONF_HEARTBEAT_MS"))
if not preconf_heartbeat_ms:
    raise Exception("Environment variable PRECONF_HEARTBEAT_MS not set")


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

def test_propose_batch_to_l1_after_reaching_max_blocks_per_batch(l2_client_node1, l1_client):
    current_block = l1_client.eth.block_number
    current_block_timestamp = l1_client.eth.get_block(current_block).timestamp
    spam_n_txs(l2_client_node1, l2_prefunded_priv_key, 11)

    event = wait_for_batch_proposed_event(l1_client, taiko_inbox_address, current_block)

    assert event['args']['meta']['proposer'] in [l1_client.eth.account.from_key(l2_prefunded_priv_key).address, l1_client.eth.account.from_key(l2_prefunded_priv_key_2).address], "Proposer should be L2 Node 1 or L2 Node 2"
    assert event['args']['meta']['proposedAt'] > current_block_timestamp, "Proposed at timestamp should be larger than current block timestamp"

def test_proposing_other_operator_blocks(l2_client_node1, l1_client, beacon_client, catalyst_node_teardown):
    catalyst_node_teardown

    # wait till 23 slot
    current_slot = get_slot_in_epoch(beacon_client)
    print(f"Current slot: {current_slot}")

    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, preconf_whitelist_address, 5)

    node_number = get_current_operator_number(l1_client, l2_prefunded_priv_key, preconf_whitelist_address)

    spam_txs_until_new_batch_is_proposed(l1_client, l2_client_node1, l2_prefunded_priv_key, taiko_inbox_address, beacon_client, preconf_min_txs)

    # should create new block in new batch
    tx_hash = spam_n_txs(l2_client_node1, l2_prefunded_priv_key, 1)
    assert wait_for_tx_to_be_included(l2_client_node1, tx_hash), "Transaction should be included in L2 Node 1"

    stop_catalyst_node(node_number)

    wait_for_slot_beginning(beacon_client, 0)
    wait_for_batch_proposed_event(l1_client, taiko_inbox_address, l1_client.eth.block_number)

    # sent tx should still be included, no reorg
    wait_for_tx_to_be_included(l2_client_node1, tx_hash)
    pass

def test_verification_of_unproposed_blocks(l1_client, l2_client_node1, catalyst_node_teardown, beacon_client):
    catalyst_node_teardown

    wait_for_slot_beginning(beacon_client, 5)

    spam_txs_until_new_batch_is_proposed(l1_client, l2_client_node1, l2_prefunded_priv_key, taiko_inbox_address, beacon_client, preconf_min_txs)
    current_block = l1_client.eth.block_number

    # spam additional block
    spam_n_blocks(l2_client_node1, l2_prefunded_priv_key, 1, preconf_min_txs)

    current_node = get_current_operator_number(l1_client, l2_prefunded_priv_key, preconf_whitelist_address)
    stop_catalyst_node(current_node)
    start_catalyst_node(current_node)

    wait_for_batch_proposed_event(l1_client, taiko_inbox_address, current_block)

def test_end_of_sequencing(l2_client_node1, beacon_client, l1_client):
    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, preconf_whitelist_address, 24) # handover window

    l2_block_number = l2_client_node1.eth.block_number
    send_n_txs_without_waiting(l2_client_node1, l2_prefunded_priv_key, preconf_min_txs)
    time.sleep(2 * preconf_heartbeat_ms / 1000)
    assert l2_client_node1.eth.block_number == l2_block_number+1, "L2 Node 1 should have a new block after sending transactions, even in handover buffer"