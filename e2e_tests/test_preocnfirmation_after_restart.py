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

def test_preocnfirmation_after_restart(l1_client, beacon_client, l2_client_node1, env_vars):
    """
    We restart the nodes and then produce 30 transactions every 2 L2 slots. We expect to receive 30 L2 blocks and 3 batches at the end
    """
    slot_duration_sec = get_slot_duration_sec(beacon_client)
    slot = get_slot_in_epoch(beacon_client)
    print("Slot: ", slot)
    try:
        #restart nodes
        restart_catalyst_node(1)
        restart_catalyst_node(2)
        # wait for nodes warmup
        time.sleep(slot_duration_sec * 3)
        # get chain info
        block_number = l2_client_node1.eth.block_number
        print("Block number:", block_number)
        batch_id = get_last_batch_id(l1_client, env_vars)
        # send transactions to create 3 batches
        # produce 1 L2 block every 2 L2 slots
        delay = get_two_l2_slots_duration_sec(env_vars.preconf_heartbeat_ms)
        print("delay", delay)
        spam_n_txs_wait_only_for_the_last(l2_client_node1, env_vars.l2_prefunded_priv_key, 3 * env_vars.max_blocks_per_batch, delay)
        # wait for transactions to be included on L1
        wait_for_batch_proposed_event(l1_client, l1_client.eth.block_number, env_vars)
        # verify
        slot = get_slot_in_epoch(beacon_client)
        print("Slot: ", slot)
        new_block_number = l2_client_node1.eth.block_number
        print("New block number:", new_block_number)
        new_batch_id = get_last_batch_id(l1_client, env_vars)
        print("New batch ID:", new_batch_id)
        assert  new_block_number >= block_number + 3 * env_vars.max_blocks_per_batch, "Invalid block number"
        assert new_batch_id >= batch_id + 3 , "Invalid batch ID"
    except subprocess.CalledProcessError as e:
        print("Error running test_preocnfirmation_after_restart")
        print(e)
        print("stdout:", e.stdout)
        print("stderr:", e.stderr)
        assert False, "test_preocnfirmation_after_restart failed"

