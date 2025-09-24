import pytest
import requests
from web3 import Web3
import os
import sys
from utils import *
import subprocess
import re
import time
from eth_account import Account
from taiko_inbox import get_last_batch_id
from forced_inclusion_store import forced_inclusion_store_is_empty, get_forced_inclusion_store_head


def send_forced_inclusion(nonce_delta):
    cmd = [
        "docker", "run", "--network", "host", "--env-file", ".env", "--rm",
        "nethswitchboard/taiko-forced-inclusion-toolbox", "send",
        "--nonce-delta", str(nonce_delta)
    ]
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
    return forced_inclusion_tx_hash

def test_forced_inclusion(l2_client_node1, env_vars):
    """
    This test runs the forced inclusion toolbox docker command and prints its output.
    """
    try:
        #send forced inclusion
        forced_inclusion_tx_hash = send_forced_inclusion(0)
        print(f"Extracted forced inclusion tx hash: {forced_inclusion_tx_hash}")

        # Spam 41 transactions to L2 Node to at least one batch which will include the forced inclusion tx
        delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
        print("spam 41 transactions with delay", delay)
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 41, delay)

        assert wait_for_tx_to_be_included(l2_client_node1, forced_inclusion_tx_hash), "Forced inclusion tx should be included in L2 Node 1"

    except subprocess.CalledProcessError as e:
        print("Error running forced inclusion toolbox docker command:")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "Forced inclusion toolbox docker command failed"


def test_three_consecutive_forced_inclusion(l1_client, beacon_client, l2_client_node1, env_vars):
    """
    Send three consecutive forced inclusions. And include them in the chain
    """
    assert env_vars.max_blocks_per_batch <= 10, "max_blocks_per_batch should be <= 10"
    assert env_vars.preconf_min_txs == 1, "preconf_min_txs should be 1"
    assert env_vars.l2_private_key != env_vars.l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    slot_duration_sec = get_slot_duration_sec(beacon_client)
    # wait for block 30 in epoch
    wait_for_slot_beginning(beacon_client, 30)
    slot = get_slot_in_epoch(beacon_client)
    print("Slot: ", slot)
    try:
        # get current nonce of FI sender
        fi_account = Account.from_key(env_vars.l2_private_key)
        fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("FI sender nonce:", fi_sender_nonce)
        # send 3 forced inclusion
        send_forced_inclusion(0)
        send_forced_inclusion(1)
        send_forced_inclusion(2)
        # get chain info
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)
        batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        # send transactions to create 4 batches
        delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
        print("delay", delay)
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
        # Sleep due to a node bug: the first gas history retrieval after restart takes too long
        # https://github.com/NethermindEth/Catalyst/issues/611
        time.sleep(slot_duration_sec)
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        assert fi_sender_nonce + 1 == new_fi_sender_nonce, "First fi transaction not included"
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 3 * env_vars.max_blocks_per_batch, delay)
        # wait 2 l1 slots to include all propose batch transactions
        time.sleep(slot_duration_sec * 2)
        # verify
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("New FI sender nonce:", new_fi_sender_nonce)
        assert block_number + 4 * env_vars.max_blocks_per_batch + 3 == new_block_number, "Invalid block number"
        assert fi_sender_nonce + 3 == new_fi_sender_nonce, "Transaction not included"
        # 4 batches for blocks and 3 batches for forced inclusion
        assert batch_id + 7 == new_batch_id, "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_three_consecutive_forced_inclusion")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_three_consecutive_forced_inclusion failed"

def test_end_of_sequencing_forced_inclusion(l1_client, beacon_client, l2_client_node1, env_vars):
    """
    Send forced inclusions before end of sequencing and include it int the chain after handover window
    """
    assert env_vars.max_blocks_per_batch <= 10, "max_blocks_per_batch should be <= 10"
    assert env_vars.preconf_min_txs == 1, "preconf_min_txs should be 1"
    assert env_vars.l2_private_key != env_vars.l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
    # check that forced inclusion list is empty
    forced_inclusion_store_is_empty(l1_client, env_vars.forced_inclusion_store_address)
    # wait for slot
    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, env_vars.preconf_whitelist_address, 19)
    try:
        # get current nonce of FI sender
        fi_account = Account.from_key(env_vars.l2_private_key)
        fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("FI sender nonce:", fi_sender_nonce)
        # send 1 forced inclusion
        send_forced_inclusion(0)
        # get chain info
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)
        batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        # wait for handower window
        wait_for_slot_beginning(beacon_client, 29)
        in_handover_block_number = l2_client_node1.eth.block_number
        print("In handover block number:", in_handover_block_number)
        assert block_number + 1 == in_handover_block_number, "Invalid block number in handover"
        # send transactions to create batch
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
        # wait for transactions to be included on L1
        wait_for_slot_beginning(beacon_client, 3)
        # check chain info
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        after_handover_block_number = l2_client_node1.eth.block_number
        print("After handover block number:", after_handover_block_number)
        after_handover_batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        print("After handover batch ID:", after_handover_batch_id)
        after_handover_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("After handover FI sender nonce:", after_handover_fi_sender_nonce)
        assert in_handover_block_number + env_vars.max_blocks_per_batch == after_handover_block_number, "Invalid block number after handover"
        # we should not have forced inclusions after handover
        assert fi_sender_nonce == after_handover_fi_sender_nonce, "Transaction not included after handover"
        assert batch_id + 2 == after_handover_batch_id, "Invalid batch ID after handover"
        # create new batch and forced inclusion
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
        # wait for transactions to be included on L1
        time.sleep(slot_duration_sec * 3)
        # check chain info
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        print("New batch ID:", new_batch_id)
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("New FI sender nonce:", new_fi_sender_nonce)
        # max_blocks_per_batch + forced inclusion block
        assert after_handover_block_number + env_vars.max_blocks_per_batch + 1 == new_block_number, "Invalid block number"
        # we should have forced inclusion
        assert fi_sender_nonce + 1 == new_fi_sender_nonce, "Transaction not included"
        # add 1 new batch with forced inclusion
        assert after_handover_batch_id + 2 == new_batch_id, "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_three_consecutive_forced_inclusion")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_three_consecutive_forced_inclusion failed"

def test_preconf_forced_inclusion_after_restart(l1_client, beacon_client, l2_client_node1, env_vars):
    """
    Restart the nodes, then add FI and produce transactions every 2 L2 slots to build batch.
    """
    assert env_vars.max_blocks_per_batch <= 10, "max_blocks_per_batch should be <= 10"
    assert env_vars.preconf_min_txs == 1, "preconf_min_txs should be 1"
    assert env_vars.l2_private_key != env_vars.l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    assert get_forced_inclusion_store_head(l1_client, env_vars.forced_inclusion_store_address) > 0, "Forced inclusion head should be greater than 0"

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)

    # Check that forced inclusion list is empty
    forced_inclusion_store_is_empty(l1_client, env_vars.forced_inclusion_store_address)

    # Wait for block 30 in epoch
    wait_for_slot_beginning(beacon_client, 1)

    try:
        # Restart nodes
        restart_catalyst_node(1)
        restart_catalyst_node(2)

        # Wait for nodes to warm up
        time.sleep(slot_duration_sec * 3)

        # Validate chain info
        print("Slot:", get_slot_in_epoch(beacon_client))
        fi_account = Account.from_key(env_vars.l2_private_key)
        fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("FI sender nonce:", fi_sender_nonce)
        batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        print("Batch ID:", batch_id)
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)

        # Send forced inclusion
        send_forced_inclusion(0)

        # Send transactions to create a batch
        spam_n_txs_wait_only_for_the_last(
            l2_client_node1,
            env_vars.l2_prefunded_priv_key,
            env_vars.max_blocks_per_batch,
            delay,
        )

        # Wait for transactions to be included on L1
        time.sleep(slot_duration_sec * 3)

        # Verify results
        print("Slot:", get_slot_in_epoch(beacon_client))
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("New FI sender nonce:", new_fi_sender_nonce)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client, env_vars.taiko_inbox_address)
        print("New batch ID:", new_batch_id)

        assert fi_sender_nonce + 1 == new_fi_sender_nonce, "FI transaction not included"
        assert block_number + env_vars.max_blocks_per_batch + 1 == new_block_number, "Invalid block number"
        assert batch_id + 2 == new_batch_id, "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_preconf_forced_inclusion_after_restart")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_preconf_forced_inclusion_after_restart failed"
