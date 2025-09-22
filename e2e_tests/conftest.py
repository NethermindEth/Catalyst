import pytest
from web3 import Web3
from web3.beacon import Beacon
from eth_account import Account
import os
from dotenv import load_dotenv
from utils import ensure_catalyst_node_running
from dataclasses import dataclass

load_dotenv()

@dataclass
class EnvVars:
    """Centralized environment variables"""
    l2_prefunded_priv_key: str
    l2_prefunded_priv_key_2: str
    taiko_inbox_address: str
    preconf_whitelist_address: str
    preconf_min_txs: int
    preconf_heartbeat_ms: int
    l2_private_key: str
    max_blocks_per_batch: int
    container_name_node1: str
    container_name_node2: str

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

        container_name_node1 = os.getenv("CONTAINER_NAME_NODE1")
        if not container_name_node1:
            raise Exception("Environment variable CONTAINER_NAME_NODE1 not set")

        container_name_node2 = os.getenv("CONTAINER_NAME_NODE2")
        if not container_name_node2:
            raise Exception("Environment variable CONTAINER_NAME_NODE2 not set")

        return cls(
            l2_prefunded_priv_key=l2_prefunded_priv_key,
            l2_prefunded_priv_key_2=l2_prefunded_priv_key_2,
            taiko_inbox_address=taiko_inbox_address,
            preconf_whitelist_address=preconf_whitelist_address,
            preconf_min_txs=preconf_min_txs,
            preconf_heartbeat_ms=preconf_heartbeat_ms,
            l2_private_key=l2_private_key,
            max_blocks_per_batch=max_blocks_per_batch,
            container_name_node1=container_name_node1,
            container_name_node2=container_name_node2
        )

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

@pytest.fixture
def catalyst_node_teardown():
    """Fixture to ensure both catalyst nodes are running after test"""
    yield None
    print("Test teardown: ensuring both catalyst nodes are running")
    ensure_catalyst_node_running(1)
    ensure_catalyst_node_running(2)