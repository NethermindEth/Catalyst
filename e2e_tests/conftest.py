import pytest
from web3 import Web3
from web3.beacon import Beacon
from eth_account import Account
import os
import time
from dotenv import load_dotenv
from utils import ensure_catalyst_node_running, spam_n_blocks, forced_inclusion_store_is_empty, check_empty_forced_inclusion_store, get_current_operator
from dataclasses import dataclass
from taiko_inbox import get_last_block_id

load_dotenv()

@dataclass
class EnvVars:
    """Centralized environment variables"""
    l2_prefunded_priv_key: str
    l2_prefunded_priv_key_2: str
    taiko_inbox_address: str
    preconf_whitelist_address: str
    forced_inclusion_store_address: str
    preconf_min_txs: int
    preconf_heartbeat_ms: int
    l2_private_key: str
    max_blocks_per_batch: int
    protocol: str

    @classmethod
    def from_env(cls):
        """Create EnvVars instance from environment variables"""
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

        forced_inclusion_store_address = os.getenv("FORCED_INCLUSION_STORE_ADDRESS")
        if not forced_inclusion_store_address:
            raise Exception("Environment variable FORCED_INCLUSION_STORE_ADDRESS not set")

        preconf_min_txs = os.getenv("PRECONF_MIN_TXS")
        if preconf_min_txs is None:
            raise Exception("PRECONF_MIN_TXS is not set")
        preconf_min_txs = int(preconf_min_txs)

        preconf_heartbeat_ms = int(os.getenv("PRECONF_HEARTBEAT_MS", "0"))
        if not preconf_heartbeat_ms:
            raise Exception("Environment variable PRECONF_HEARTBEAT_MS not set")

        l2_private_key = os.getenv("L2_PRIVATE_KEY")
        if not l2_private_key:
            raise Exception("Environment variable L2_PRIVATE_KEY not set")

        max_blocks_per_batch = int(os.getenv("MAX_BLOCKS_PER_BATCH", "0"))
        if not max_blocks_per_batch:
            raise Exception("Environment variable MAX_BLOCKS_PER_BATCH not set")

        protocol = os.getenv("PROTOCOL")
        if not protocol:
            raise Exception("Environment variable PROTOCOL not set")

        return cls(
            l2_prefunded_priv_key=l2_prefunded_priv_key,
            l2_prefunded_priv_key_2=l2_prefunded_priv_key_2,
            taiko_inbox_address=taiko_inbox_address,
            preconf_whitelist_address=preconf_whitelist_address,
            forced_inclusion_store_address=forced_inclusion_store_address,
            preconf_min_txs=preconf_min_txs,
            preconf_heartbeat_ms=preconf_heartbeat_ms,
            l2_private_key=l2_private_key,
            max_blocks_per_batch=max_blocks_per_batch,
            protocol=protocol,
        )

    def is_shasta(self):
        return self.protocol == "shasta"

    def is_pacaya(self):
        return self.protocol == "pacaya"

@pytest.fixture(scope="session")
def env_vars():
    """Centralized environment variables fixture"""
    return EnvVars.from_env()

@pytest.fixture(scope="session")
def l1_client():
    w3 = Web3(Web3.HTTPProvider(os.getenv("L1_RPC_URL")))
    return w3

@pytest.fixture(scope="session")
def l2_client_node1():
    w3 = Web3(Web3.HTTPProvider(os.getenv("L2_RPC_URL_NODE1")))
    return w3

@pytest.fixture(scope="session")
def l2_client_node2():
    w3 = Web3(Web3.HTTPProvider(os.getenv("L2_RPC_URL_NODE2")))
    return w3

@pytest.fixture(scope="session")
def beacon_client():
    beacon_rpc_url = os.getenv("BEACON_RPC_URL")
    if not beacon_rpc_url:
        raise Exception("Environment variable BEACON_RPC_URL not set")

    return Beacon(beacon_rpc_url)

@pytest.fixture(scope="session")
def forced_inclusion_parameters(l1_client, env_vars):
    assert env_vars.max_blocks_per_batch <= 10, "max_blocks_per_batch should be <= 10"
    assert env_vars.preconf_min_txs == 1, "preconf_min_txs should be 1"
    assert env_vars.l2_private_key != env_vars.l2_prefunded_priv_key, "l2_private_key should not be the same as l2_prefunded_priv_key"
    check_empty_forced_inclusion_store(l1_client, env_vars)

@pytest.fixture
def catalyst_node_teardown():
    """Fixture to ensure both catalyst nodes are running after test"""
    yield None
    print("Test teardown: ensuring both catalyst nodes are running")
    ensure_catalyst_node_running(1)
    ensure_catalyst_node_running(2)

@pytest.fixture
def forced_inclusion_teardown(l1_client, l2_client_node1, env_vars):
    """Fixture to ensure forced inclusion store is empty after test"""
    yield None
    print("Test teardown: ensuring forced inclusion store is empty")
    if not forced_inclusion_store_is_empty(l1_client, env_vars):
        print("Spamming blocks to ensure forced inclusion store is empty")
        spam_n_blocks(l2_client_node1, env_vars.l2_prefunded_priv_key, env_vars.max_blocks_per_batch, env_vars.preconf_min_txs)

@pytest.fixture(scope="session", autouse=True)
def global_setup(l1_client, l2_client_node1, l2_client_node2, env_vars):
    """Run once before all tests"""

    print("Wait for Geth sync with TaikoInbox")
    block_number_contract = get_last_block_id(l1_client, env_vars)

    while True:
        block_number_node1 = l2_client_node1.eth.block_number
        block_number_node2 = l2_client_node2.eth.block_number
        if block_number_contract <= block_number_node1 and block_number_contract <= block_number_node2:
            break

        print(
            f"Block Number Contract: {block_number_contract}, "
            f"Node1: {block_number_node1}, "
            f"Node2: {block_number_node2}"
        )
        print("Sleeping 10 sec to sync...")
        time.sleep(10)

    print("Wait for operator to be set in whitelist contract")
    empty_address = "0x0000000000000000000000000000000000000000"
    while True:
        current_operator = get_current_operator(l1_client, env_vars.preconf_whitelist_address)
        if current_operator != empty_address:
            print(f"Operator is set: {current_operator}")
            break

        print(f"Current operator is empty address, waiting...")
        time.sleep(10)

    yield
    print("Global teardown after all tests")
