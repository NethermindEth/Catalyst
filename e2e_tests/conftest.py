import pytest
from web3 import Web3
from web3.beacon import Beacon
from eth_account import Account
import os
from dotenv import load_dotenv

load_dotenv()

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