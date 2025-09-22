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

max_blocks_per_batch = int(os.getenv("MAX_BLOCKS_PER_BATCH"))
if not max_blocks_per_batch:
    raise Exception("Environment variable MAX_BLOCKS_PER_BATCH not set")

preconf_heartbeat_ms = int(os.getenv("PRECONF_HEARTBEAT_MS"))
if not max_blocks_per_batch:
    raise Exception("Environment variable PRECONF_HEARTBEAT_MS not set")

container_name_node1 = os.getenv("CONTAINER_NAME_NODE1")
if not container_name_node1:
    raise Exception("Environment variable CONTAINER_NAME_NODE1 not set")

container_name_node2 = os.getenv("CONTAINER_NAME_NODE2")
if not container_name_node2:
    raise Exception("Environment variable CONTAINER_NAME_NODE2 not set")

def restart_container(container_name):
    result = subprocess.run(["docker", "restart", container_name], capture_output=True, text=True, check=True)
    print("Restarted container:", container_name)
    print(result.stdout)
    if result.stderr:
        print(result.stderr)
        assert False, "Error restarting container"

def test_first_preocnfirmation(l1_client, beacon_client, l2_client_node1):
    """
    Send three proposeBatch after node restart
    """
    assert l2_private_key != l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    slot_duration_sec = get_slot_duration_sec(beacon_client)
    # wait for block 30 in epoch
    sleep_until_slot_in_epoch(beacon_client, 1)
    slot = get_slot_in_epoch(beacon_client)
    print("Slot: ", slot)
    try:
        #restart nodes
        restart_container(container_name_node1)
        restart_container(container_name_node2)
        # wait for nodes warmup
        time.sleep(slot_duration_sec * 3)
        # get chain info
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)
        batch_id = get_last_batch_id(l1_client)
        # send transactions to create 3 batches
        delay = preconf_heartbeat_ms / 500
        print("delay", delay)
        spam_n_txs_no_wait(l2_client_node1, l2_prefunded_priv_key, 3 * max_blocks_per_batch, delay)
        # wait for transactions to be included on L1
        time.sleep(slot_duration_sec * 3)
        # verify
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client)
        print("New batch ID:", new_batch_id)
        assert block_number + 3 * max_blocks_per_batch == new_block_number, "Invalid block number"
        assert batch_id + 3 == new_batch_id, "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_three_consecutive_forced_inclusion")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_three_consecutive_forced_inclusion failed"

