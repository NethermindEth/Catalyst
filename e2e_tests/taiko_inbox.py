from web3 import Web3
import json
from utils import get_shasta_inbox_abi

with open("../pacaya/src/l1/abi/ITaikoInbox.json") as f:
    pacaya_abi = json.load(f)

def get_last_batch_id(l1_client, env_vars):
    if env_vars.is_pacaya():
        contract = l1_client.eth.contract(address=env_vars.taiko_inbox_address, abi=pacaya_abi)
        result = contract.functions.getStats2().call()
        last_batch_id = result[0]
        return last_batch_id
    else:
        core_state = get_core_state(l1_client, env_vars)
        last_batch_id = core_state[0] - 1
        return last_batch_id

def get_last_block_id(l1_client, env_vars):
    if env_vars.is_pacaya():
        batch_id = int(get_last_batch_id(l1_client, env_vars)) - 1
        contract = l1_client.eth.contract(address=env_vars.taiko_inbox_address, abi=pacaya_abi)
        result = contract.functions.getBatch(batch_id).call()
        last_block_id = result[1]
        return last_block_id
    else:
        core_state = get_core_state(l1_client, env_vars)
        last_block_id = core_state[1]
        #print(f"Last L2 block id from core state: {last_block_id}, next proposal id: {core_state[0]}")
        return last_block_id

def get_core_state(l1_client, env_vars):
    shasta_abi = get_shasta_inbox_abi()
    contract = l1_client.eth.contract(address=env_vars.taiko_inbox_address, abi=shasta_abi)
    result = contract.functions.getCoreState().call()
    return result