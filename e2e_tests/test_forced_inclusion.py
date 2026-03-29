from unittest import skip
from web3 import Web3
from utils import *
import subprocess
import re
import time
from eth_account import Account
from chain_info import ChainInfo
from taiko_inbox import get_last_block_id


def send_forced_inclusion(nonce_delta, env_vars):
    if env_vars.is_pacaya():
        image = "nethswitchboard/taiko-forced-inclusion-toolbox"
    else:
        image = "nethswitchboard/taiko-forced-inclusion-toolbox:shasta"
    cmd = [
        "docker", "run", "--network", "host", "--env-file", ".env", "--rm",
        image, "send",
        "--nonce-delta", str(nonce_delta)
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
    except subprocess.CalledProcessError as e:
        print(f"Error running sending forced inclusion")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False

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

def test_forced_inclusion(l1_client, beacon_client, l2_client_node1, env_vars, forced_inclusion_teardown):
    """
    This test runs the forced inclusion toolbox docker command and prints its output.
    """
    forced_inclusion_teardown

    check_empty_forced_inclusion_store(l1_client, env_vars)
    fi_account = Account.from_key(env_vars.l2_private_key)
    ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    get_last_block_id(l1_client, env_vars)

    #send forced inclusion
    forced_inclusion_tx_hash = send_forced_inclusion(0, env_vars)
    print(f"Extracted forced inclusion tx hash: {forced_inclusion_tx_hash}")

    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
    print("spam 41 transactions with delay", delay)
    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 41, delay)
    wait_for_batch_proposed_event(l1_client, l1_client.eth.block_number, env_vars)

    get_last_block_id(l1_client, env_vars)

    assert wait_for_tx_to_be_included(l2_client_node1, forced_inclusion_tx_hash), "Forced inclusion tx should be included in L2 Node 1"


def test_three_consecutive_forced_inclusion(l1_client, beacon_client, l2_client_node1, env_vars, forced_inclusion_teardown, forced_inclusion_parameters):
    """
    Send three consecutive forced inclusions. And include them in the chain
    """
    forced_inclusion_teardown

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)

    # Restart nodes for clean start
    restart_catalyst_node(1)
    restart_catalyst_node(2)
    time.sleep(3*slot_duration_sec)

    check_empty_forced_inclusion_store(l1_client, env_vars)

    # send 3 forced inclusion
    tx_1 = send_forced_inclusion(0, env_vars)
    tx_2 = send_forced_inclusion(1, env_vars)
    tx_3 = send_forced_inclusion(2, env_vars)
    # Synchronize transaction sending with slot time
    wait_for_next_slot(beacon_client)
    # spam transactions
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 4 * env_vars.max_blocks_per_batch, delay)

    # wait 2 l1 slots to include all propose batch transactions
    time.sleep(slot_duration_sec * 2)

    wait_for_tx_to_be_included(l2_client_node1, tx_1)
    wait_for_tx_to_be_included(l2_client_node1, tx_2)
    wait_for_tx_to_be_included(l2_client_node1, tx_3)
    wait_for_forced_inclusion_store_to_be_empty(l1_client, env_vars)

@skip("Skipping end of sequencing forced inclusion test, cannot run with empty blocks production")
def test_end_of_sequencing_forced_inclusion(l1_client, beacon_client, l2_client_node1, env_vars, forced_inclusion_teardown, forced_inclusion_parameters):
    """
    Send forced inclusions before end of sequencing and include it int the chain after handover window
    """
    forced_inclusion_teardown

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
    fi_account = Account.from_key(env_vars.l2_private_key)
    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, env_vars.preconf_whitelist_address, 19)

    # get chain info
    chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    # send 1 forced inclusion
    forced_inclusion_tx_hash = send_forced_inclusion(0, env_vars)
    # wait for handower window
    wait_for_slot_beginning(beacon_client, 25)

    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)
    # send transactions to create batch
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
    after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    # wait for transactions to be included on L1
    wait_for_slot_beginning(beacon_client, 3)
    # Verify reorg after L1 inclusion
    after_spam_chain_info.check_reorg(l2_client_node1)
    # check chain info
    after_handover_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    # we should not have forced inclusions after handover
    assert chain_info.fi_sender_nonce == after_handover_chain_info.fi_sender_nonce, "Transaction not included after handover"
    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)


    # create new batch and forced inclusion
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
    after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    # wait for transactions to be included on L1
    time.sleep(slot_duration_sec * 3)
    # Verify reorg after L1 inclusion
    after_spam_chain_info.check_reorg(l2_client_node1)

    wait_for_tx_to_be_included(l2_client_node1, forced_inclusion_tx_hash)
    wait_for_forced_inclusion_store_to_be_empty(l1_client, env_vars)

def test_preconf_forced_inclusion_after_restart(l1_client, beacon_client, l2_client_node1, env_vars, forced_inclusion_teardown, forced_inclusion_parameters):
    """
    Restart the nodes, then add FI and produce transactions every 2 L2 slots to build batch.
    """
    forced_inclusion_teardown

    assert get_forced_inclusion_store_head(l1_client, env_vars) > 0, "Forced inclusion head should be greater than 0"

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
    fi_account = Account.from_key(env_vars.l2_private_key)

    wait_for_slot_beginning(beacon_client, 1)

    # Restart nodes
    restart_catalyst_node(1)
    restart_catalyst_node(2)

    # Wait for nodes to warm up
    time.sleep(slot_duration_sec * 3)

    # Validate chain info
    chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)

    # Send forced inclusion
    forced_inclusion_tx_hash = send_forced_inclusion(0, env_vars)

    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)

    # Send transactions to create a batch
    spam_n_txs_wait_only_for_the_last(
        l2_client_node1,
        env_vars.l2_prefunded_priv_key,
        env_vars.max_blocks_per_batch,
        delay,
    )

    # Get chain info
    before_l1_inclusion_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)

    # Wait for transactions to be included on L1
    time.sleep(slot_duration_sec * 3)

    # Verify reorg after L1 inclusion
    before_l1_inclusion_chain_info.check_reorg(l2_client_node1)
    wait_for_tx_to_be_included(l2_client_node1, forced_inclusion_tx_hash)
    wait_for_forced_inclusion_store_to_be_empty(l1_client, env_vars)

def test_recover_forced_inclusion_after_restart(l1_client, beacon_client, l2_client_node1, env_vars, forced_inclusion_teardown, forced_inclusion_parameters):
    """
    Test forced inclusion recovery after node restart
    """
    forced_inclusion_teardown

    fi_account = Account.from_key(env_vars.l2_private_key)
    slot_duration_sec = get_slot_duration_sec(beacon_client)

    # wait_for_slot_beginning(beacon_client, 1)

    start_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    # start_block = l1_client.eth.block_number

    forced_inclusion_tx_hash = send_forced_inclusion(0, env_vars)

    wait_for_new_block(l2_client_node1, start_chain_info.block_number)

    # Restart nodes
    restart_catalyst_node(1)
    restart_catalyst_node(2)

    wait_for_forced_inclusion_store_to_be_empty(l1_client, env_vars)
    chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    assert start_chain_info.fi_sender_nonce + 1 == chain_info.fi_sender_nonce, "FI transaction not included after restart"
    start_chain_info.check_reorg(l2_client_node1)

@skip("Skipping test_verify_forced_inclusion_after_previous_operator_stop, needs refactor with empty blocks production")
def test_verify_forced_inclusion_after_previous_operator_stop(l1_client, beacon_client, l2_client_node1, env_vars, catalyst_node_teardown, forced_inclusion_teardown, forced_inclusion_parameters):
    """
    Test forced inclusion after previous operator stop
    """
    # Start all nodes after test
    catalyst_node_teardown
    forced_inclusion_teardown
    fi_account = Account.from_key(env_vars.l2_private_key)

    slot_duration_sec = get_slot_duration_sec(beacon_client)
    delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)

    # Wait for block 5 in epoch
    wait_for_epoch_with_operator_switch_and_slot(beacon_client, l1_client, env_vars.preconf_whitelist_address, 1)
    node_number = get_current_operator_number(l1_client, env_vars.l2_prefunded_priv_key, env_vars.preconf_whitelist_address)

    op1_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)

    # Send 2 forced inclusions
    send_forced_inclusion(0, env_vars)
    send_forced_inclusion(1)

    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)

    # send transactions but don't create batch
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch-1, delay)

    op1_stop_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    assert op1_chain_info.fi_sender_nonce + 1 == op1_stop_chain_info.fi_sender_nonce, "FI transaction not included"

    # Stop current operator
    stop_catalyst_node(node_number)

    # wait for handower window
    wait_for_slot_beginning(beacon_client, 25)
    in_handover_block_number = l2_client_node1.eth.block_number
    print("In handover block number:", in_handover_block_number)

    # end_of_sequencing block not added as node is stopped
    assert op1_stop_chain_info.block_number == in_handover_block_number, "Invalid block number in handover"

    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)

    # send transactions to create batch
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
    after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)

    # wait for new epoch
    wait_for_slot_beginning(beacon_client, 0)

    # we started verifier but result not ready yet
    new_epoch_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)

    # Validate chain info
    after_spam_chain_info.check_reorg(l2_client_node1)
    assert op1_stop_chain_info.fi_sender_nonce == new_epoch_chain_info.fi_sender_nonce, "FI transaction not included"

    # wait for Verification
    wait_for_slot_beginning(beacon_client, 5)

    # All preconf blocks should be included in L1
    op1_stop_chain_info.check_reorg(l2_client_node1)
    after_spam_chain_info.check_reorg(l2_client_node1)
    new_epoch_chain_info.check_reorg(l2_client_node1)
    after_inclusion_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    assert new_epoch_chain_info.fi_sender_nonce == after_inclusion_chain_info.fi_sender_nonce, "FI transaction not included"

    # Synchronize transaction sending with L1 slot time
    wait_for_next_slot(beacon_client)
    # send transactions to create batch with FI
    spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, delay)
    after_spam_chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    assert after_inclusion_chain_info.fi_sender_nonce + 1 == after_spam_chain_info.fi_sender_nonce, "FI transaction not included"

    # wait for transactions to be included on L1
    time.sleep(slot_duration_sec * 3)

    # Validate chain info
    after_spam_chain_info.check_reorg(l2_client_node1)
    chain_info = ChainInfo.from_chain(fi_account.address, l2_client_node1, l1_client, env_vars, beacon_client)
    assert after_spam_chain_info.fi_sender_nonce == chain_info.fi_sender_nonce, "FI transaction not included"
