import pytest
import requests
from web3 import Web3
import os
from dotenv import load_dotenv
import sys
from utils import *
import subprocess
import re
import time
from eth_account import Account
from taiko_inbox import get_last_batch_id

load_dotenv()

l2_prefunded_priv_key = os.getenv("TEST_L2_PREFUNDED_PRIVATE_KEY")
if not l2_prefunded_priv_key:
    raise Exception("Environment variable TEST_L2_PREFUNDED_PRIVATE_KEY not set")

#FI sender
l2_private_key = os.getenv("L2_PRIVATE_KEY")
if not l2_private_key:
    raise Exception("Environment variable L2_PRIVATE_KEY not set")

max_blocks_per_batch = int(os.getenv("MAX_BLOCKS_PER_BATCH"))
if not max_blocks_per_batch:
    raise Exception("Environment variable MAX_BLOCKS_PER_BATCH not set")

preconf_heartbeat_ms = int(os.getenv("PRECONF_HEARTBEAT_MS"))
if not max_blocks_per_batch:
    raise Exception("Environment variable PRECONF_HEARTBEAT_MS not set")

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


def test_three_consecutive_forced_inclusion(l1_client, beacon_client, l2_client_node1):
    """
    Send three consecutive forced inclusions. And include them in the chain
    """
    assert l2_private_key != l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    slot_duration_sec = get_slot_duration_sec(beacon_client)
    # wait for block 30 in epoch
    sleep_until_slot_in_epoch(beacon_client, 30)
    slot = get_slot_in_epoch(beacon_client)
    print("Slot: ", slot)
    try:
        # get current nonce of FI sender
        fi_account = Account.from_key(l2_private_key)
        fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("FI sender nonce:", fi_sender_nonce)
        # send 3 forced inclusion
        send_forced_inclusion(0)
        send_forced_inclusion(1)
        send_forced_inclusion(2)
        # get chain info
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)
        batch_id = get_last_batch_id(l1_client)
        # send transactions to create 4 batches
        delay = preconf_heartbeat_ms / 500
        print("delay", delay)
        spam_n_txs_no_wait(l2_client_node1, l2_prefunded_priv_key, max_blocks_per_batch, delay)
        # Sleep due to a node bug: the first gas history retrieval after restart takes too long
        # https://github.com/NethermindEth/Catalyst/issues/611
        time.sleep(slot_duration_sec)
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        assert fi_sender_nonce + 1 == new_fi_sender_nonce, "First fi transaction not included"
        spam_n_txs_no_wait(l2_client_node1, l2_prefunded_priv_key, 3 * max_blocks_per_batch, delay)
        # wait 2 l1 slots to include all propose batch transactions
        time.sleep(slot_duration_sec * 2)
        # verify
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client)
        new_fi_sender_nonce = l2_client_node1.eth.get_transaction_count(fi_account.address)
        print("New FI sender nonce:", new_fi_sender_nonce)
        assert block_number + 4 * max_blocks_per_batch + 3 == new_block_number, "Invalid block number"
        assert fi_sender_nonce + 3 == new_fi_sender_nonce, "Transaction not included"
        # 4 batches for blocks and 3 batches for forced inclusion
        assert batch_id + 7 == new_batch_id, "Invalid batch ID"


    except subprocess.CalledProcessError as e:
        print("Error running test_three_consecutive_forced_inclusion")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_three_consecutive_forced_inclusion failed"

