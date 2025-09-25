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
from chain_info import ChainInfo

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
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
    # check that forced inclusion list is empty
    forced_inclusion_store_is_empty(l1_client, env_vars.forced_inclusion_store_address)
    fi_account = Account.from_key(env_vars.l2_private_key)
    # Restart nodes for clean start
    restart_catalyst_node(1)
    restart_catalyst_node(2)
    # wait for block 30 in epoch
    wait_for_slot_beginning(beacon_client, 30)
    slot = get_slot_in_epoch(beacon_client)
    print("Slot: ", slot)
    try:
         # get chain info
        chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        # send 3 forced inclusion
        send_forced_inclusion(0)
        send_forced_inclusion(1)
        send_forced_inclusion(2)
        # spam transactions
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 4 * env_vars.max_blocks_per_batch, delay)
        # wait 2 l1 slots to include all propose batch transactions
        time.sleep(slot_duration_sec * 2)
        # verify
        new_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)

        assert chain_info.block_number + 4 * env_vars.max_blocks_per_batch + 3 == new_chain_info.block_number, "Invalid block number"
        assert chain_info.fi_sender_nonce + 3 == new_chain_info.fi_sender_nonce, "Transaction not included"
        # 4 batches for blocks and 3 batches for forced inclusion
        assert chain_info.batch_id + 7 == new_chain_info.batch_id, "Invalid batch ID"
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
    fi_account = Account.from_key(env_vars.l2_private_key)
    # wait for slot
    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, env_vars.preconf_whitelist_address, 19)
    try:
        # get chain info
        chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        # send 1 forced inclusion
        send_forced_inclusion(0)
        # wait for handower window
        wait_for_slot_beginning(beacon_client, 29)
        in_handover_block_number = l2_client_node1.eth.block_number
        print("In handover block number:", in_handover_block_number)
        # end_of_sequencing block added
        assert chain_info.block_number + 1 == in_handover_block_number, "Invalid block number in handover"
        # send transactions to create batch
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
        after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        # wait for transactions to be included on L1
        wait_for_slot_beginning(beacon_client, 3)
        # Verify reorg after L1 inclusion
        after_spam_chain_info.check_reorg(l2_client_node1)
        # check chain info
        after_handover_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        assert in_handover_block_number + env_vars.max_blocks_per_batch == after_handover_chain_info.block_number, "Invalid block number after handover"
        # we should not have forced inclusions after handover
        assert chain_info.fi_sender_nonce == after_handover_chain_info.fi_sender_nonce, "Transaction not included after handover"
        assert chain_info.batch_id + 2 == after_handover_chain_info.batch_id, "Invalid batch ID after handover"
        # create new batch and forced inclusion
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
        after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        # wait for transactions to be included on L1
        time.sleep(slot_duration_sec * 3)
        # Verify reorg after L1 inclusion
        after_spam_chain_info.check_reorg(l2_client_node1)
        # check chain info
        new_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        # max_blocks_per_batch + forced inclusion block
        assert after_handover_chain_info.block_number + env_vars.max_blocks_per_batch + 1 == new_chain_info.block_number, "Invalid block number"
        # we should have forced inclusion
        assert after_handover_chain_info.fi_sender_nonce + 1 == new_chain_info.fi_sender_nonce, "Transaction not included"
        # add 1 new batch with forced inclusion
        assert after_handover_chain_info.batch_id + 2 == new_chain_info.batch_id, "Invalid batch ID"
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
    fi_account = Account.from_key(env_vars.l2_private_key)

    # Wait for block 30 in epoch
    wait_for_slot_beginning(beacon_client, 1)

    try:
        # Restart nodes
        restart_catalyst_node(1)
        restart_catalyst_node(2)

        # Wait for nodes to warm up
        time.sleep(slot_duration_sec * 3)

        # Validate chain info
        chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)

        # Send forced inclusion
        send_forced_inclusion(0)

        # Send transactions to create a batch
        spam_n_txs_wait_only_for_the_last(
            l2_client_node1,
            env_vars.l2_prefunded_priv_key,
            env_vars.max_blocks_per_batch,
            delay,
        )

        # Get chain info
        before_l1_inclusion_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        assert chain_info.fi_sender_nonce + 1 == before_l1_inclusion_chain_info.fi_sender_nonce, "FI transaction not included"
        assert chain_info.block_number + env_vars.max_blocks_per_batch + 1 == before_l1_inclusion_chain_info.block_number, "Invalid block number"
        assert chain_info.batch_id == before_l1_inclusion_chain_info.batch_id, "Invalid batch ID"

        # Wait for transactions to be included on L1
        time.sleep(slot_duration_sec * 3)

        # Verify reorg after L1 inclusion
        before_l1_inclusion_chain_info.check_reorg(l2_client_node1)

        # Verify results
        new_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        assert chain_info.fi_sender_nonce + 1 == new_chain_info.fi_sender_nonce, "FI transaction not included"
        assert chain_info.block_number + env_vars.max_blocks_per_batch + 1 == new_chain_info.block_number, "Invalid block number"
        assert chain_info.batch_id + 2 == new_chain_info.batch_id, "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_preconf_forced_inclusion_after_restart")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_preconf_forced_inclusion_after_restart failed"

def test_recover_forced_inclusion_after_restart(l1_client, beacon_client, l2_client_node1, env_vars):
    """
    Test forced inclusion recovery after node restart
    """
    assert env_vars.max_blocks_per_batch <= 10, "max_blocks_per_batch should be <= 10"
    assert env_vars.preconf_min_txs == 1, "preconf_min_txs should be 1"
    assert env_vars.l2_private_key != env_vars.l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    # Check that forced inclusion list is empty
    forced_inclusion_store_is_empty(l1_client, env_vars.forced_inclusion_store_address)
    fi_account = Account.from_key(env_vars.l2_private_key)

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)

    # Restart nodes
    restart_catalyst_node(1)
    restart_catalyst_node(2)

    # Wait for block 1 in epoch
    wait_for_slot_beginning(beacon_client, 1)

    try:
        # Validate chain info
        start_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)

        # Send forced inclusion
        send_forced_inclusion(0)

        # send transactions but don't create batch
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch-1, delay)

        # Validate chain info
        chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        assert start_chain_info.fi_sender_nonce + 1 == chain_info.fi_sender_nonce, "FI transaction not included"
        assert start_chain_info.block_number + (env_vars.max_blocks_per_batch - 1) + 1 == chain_info.block_number, "Invalid block number"
        assert start_chain_info.batch_id == chain_info.batch_id, "Invalid batch ID"

        # Restart nodes
        restart_catalyst_node(1)
        restart_catalyst_node(2)

        # Wait for nodes to warm up
        time.sleep(slot_duration_sec * 5)

        # Validate chain info
        chain_info.check_reorg(l2_client_node1)
        chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars.taiko_inbox_address, beacon_client)
        assert start_chain_info.fi_sender_nonce + 1 == chain_info.fi_sender_nonce, "FI transaction not included after restart"
        assert start_chain_info.block_number + (env_vars.max_blocks_per_batch - 1) + 1 == chain_info.block_number, "Invalid block number after restart"
        assert start_chain_info.batch_id + 2 == chain_info.batch_id, "Invalid batch ID after restart"

    except subprocess.CalledProcessError as e:
        print("Error running test_preconf_forced_inclusion_after_restart")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_preconf_forced_inclusion_after_restart failed"