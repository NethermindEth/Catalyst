from web3 import Web3
import json
from utils import get_shasta_inbox_abi

with open("../pacaya/src/l1/abi/ITaikoInbox.json") as f:
    pacaya_abi = json.load(f)

def get_last_batch_id(l1_client, env_vars):
    core_state = get_core_state(l1_client, env_vars)
    last_batch_id = core_state[0] - 1
    return last_batch_id

def get_last_block_id(l1_client, env_vars):
    return 0 # not needed for shasta

def get_core_state(l1_client, env_vars):
    shasta_abi = get_shasta_inbox_abi()
    contract = l1_client.eth.contract(address=env_vars.taiko_inbox_address, abi=shasta_abi)
    result = contract.functions.getCoreState().call()
    return result